//! `/ws` 연결 하나 = 참가자 하나. 첫 메시지는 반드시 `join`이어야 한다.
//! 권한 판단(주최자 여부, 입장 여부)은 모두 `Rooms`가 하고, 여기서는 메시지를 라우팅만 한다.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::auth::Auth;
use crate::protocol::{ClientMsg, ErrorCode, PeerId, ServerMsg};
use crate::room::{self, JoinOutcome, JoinRequest, Rooms};

/// SDP는 수 KB 수준이다. 이보다 큰 메시지는 거부한다.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const OUTBOX_CAPACITY: usize = 64;
const PING_INTERVAL: Duration = Duration::from_secs(20);

pub async fn handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(rooms): State<Arc<Rooms>>,
    State(auth): State<Arc<Auth>>,
) -> Response {
    // 다른 사이트의 페이지가 사용자의 쿠키로 연결하는 것(cross-site WebSocket hijacking)을 막는다.
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let account = auth.user_from_headers(&headers);
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, rooms, account))
}

/// 브라우저가 보낸 Origin의 host:port가 요청 Host와 같아야 한다. Origin이 없으면 거부.
fn same_origin(headers: &HeaderMap) -> bool {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    match (origin, host) {
        (Some(origin), Some(host)) => origin
            .split_once("://")
            .is_some_and(|(_, authority)| authority == host),
        _ => false,
    }
}

async fn run(socket: WebSocket, rooms: Arc<Rooms>, account: Option<String>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(OUTBOX_CAPACITY);

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        loop {
            let msg = tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => {
                        let json = serde_json::to_string(&msg).expect("ServerMsg is always serializable");
                        Message::Text(Utf8Bytes::from(json))
                    }
                    // 모든 송신자가 사라짐 = 연결 종료
                    None => break,
                },
                _ = ping.tick() => Message::Ping(Default::default()),
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // 입장했거나 대기 중인 (방 코드, 참가자 id). 실제 권한은 매 요청마다 Rooms가 확인한다.
    let mut session: Option<(String, PeerId)> = None;

    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(text) => text,
            Message::Close(_) => break,
            _ => continue,
        };
        let Ok(msg) = serde_json::from_str::<ClientMsg>(&text) else {
            reply(&tx, ErrorCode::BadRequest);
            continue;
        };

        let result = match (msg, &session) {
            (
                ClientMsg::Join {
                    room,
                    name,
                    lang,
                    partner_lang,
                    media,
                },
                None,
            ) => match (room::normalize_name(&name), room::is_valid_room_code(&room)) {
                (Some(name), true)
                    if room::is_valid_lang(&lang) && room::is_valid_lang(&partner_lang) =>
                {
                    let req = JoinRequest {
                        name,
                        lang,
                        partner_lang,
                        media,
                    };
                    rooms
                        .join(&room, req, account.as_deref(), tx.clone())
                        .map(|outcome| {
                            tracing::info!(room = %room, ?outcome, "join");
                            let (JoinOutcome::Joined(id) | JoinOutcome::Waiting(id)) = outcome;
                            session = Some((room, id));
                        })
                }
                _ => Err(ErrorCode::BadRequest),
            },
            (ClientMsg::Join { .. }, Some(_)) => Err(ErrorCode::BadRequest),
            (_, None) => Err(ErrorCode::NotJoined),
            (ClientMsg::Lang { lang, partner_lang }, Some((room, id))) => {
                if room::is_valid_lang(&lang) && room::is_valid_lang(&partner_lang) {
                    rooms.update(room, id, |p| {
                        p.lang = lang;
                        p.partner_lang = partner_lang;
                    });
                    Ok(())
                } else {
                    Err(ErrorCode::BadRequest)
                }
            }
            (ClientMsg::Media { media }, Some((room, id))) => {
                rooms.update(room, id, |p| p.media = media);
                Ok(())
            }
            (ClientMsg::Signal { to, data }, Some((room, id))) => {
                rooms.relay_signal(room, id, &to, data)
            }
            (ClientMsg::Admit { id: guest }, Some((room, id))) => rooms.admit(room, id, &guest),
            (ClientMsg::Deny { id: guest }, Some((room, id))) => rooms.deny(room, id, &guest),
            (ClientMsg::Settings { settings }, Some((room, id))) => {
                rooms.set_settings(room, id, settings)
            }
        };
        if let Err(code) = result {
            reply(&tx, code);
        }
    }

    if let Some((room, id)) = session {
        tracing::info!(room = %room, peer = %id, "left");
        rooms.leave(&room, &id);
    }
    drop(tx);
    let _ = writer.await;
}

fn reply(tx: &mpsc::Sender<ServerMsg>, code: ErrorCode) {
    let _ = tx.try_send(ServerMsg::Error {
        code,
        message: format!("{code:?}"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(origin: Option<&str>, host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(o) = origin {
            h.insert(header::ORIGIN, o.parse().unwrap());
        }
        h
    }

    #[test]
    fn websocket_requires_same_origin() {
        assert!(same_origin(&headers(
            Some("http://localhost:5173"),
            "localhost:5173"
        )));
        assert!(same_origin(&headers(
            Some("https://meet.example.com"),
            "meet.example.com"
        )));
        assert!(!same_origin(&headers(
            Some("https://evil.example"),
            "meet.example.com"
        )));
        assert!(!same_origin(&headers(
            Some("https://meet.example.com.evil.example"),
            "meet.example.com"
        )));
        assert!(!same_origin(&headers(None, "meet.example.com")));
    }
}
