//! `/ws` 연결 하나 = 참가자 하나. 첫 메시지는 반드시 `join`이어야 한다.
//! 권한 판단(주최자 여부, 입장 여부)은 모두 `Rooms`가 하고, 여기서는 메시지를 라우팅만 한다.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Notify, mpsc};

use crate::auth::{self, Auth};
use crate::pipeline::{self, Pipeline, SpeakerHandle};
use crate::protocol::{ClientMsg, ErrorCode, PeerId, ServerMsg};
use crate::providers::live::SttTokens;
use crate::room::{self, JoinOutcome, JoinRequest, Outbox, Rooms};

/// SDP는 수 KB 수준이다. 이보다 큰 메시지는 거부한다.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const OUTBOX_CAPACITY: usize = 64;
const PING_INTERVAL: Duration = Duration::from_secs(20);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// STT 중간 결과는 약 0.5초마다 온다. 이보다 훨씬 잦으면 버린다(번역 비용 폭주 방지).
const STT_PER_SEC: f64 = 5.0;
const STT_BURST: f64 = 10.0;
/// 음성 인식 세션은 3분 넘게 조용하거나 언어가 바뀔 때 새로 연다. 평소 몇 분에 1개면 충분하다.
const STT_TOKENS_PER_SEC: f64 = 1.0 / 30.0;
const STT_TOKEN_BURST: f64 = 4.0;

pub async fn handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(rooms): State<Arc<Rooms>>,
    State(auth): State<Arc<Auth>>,
    State(pipeline): State<Arc<Pipeline>>,
    State(stt): State<Option<Arc<dyn SttTokens>>>,
) -> Response {
    // 다른 사이트의 페이지가 사용자의 쿠키로 연결하는 것(cross-site WebSocket hijacking)을 막는다.
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token =
        auth::token_from_headers(&headers).filter(|token| auth.session_user(token).is_some());
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, rooms, pipeline, stt, auth, token))
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

async fn run(
    socket: WebSocket,
    rooms: Arc<Rooms>,
    pipeline: Arc<Pipeline>,
    stt: Option<Arc<dyn SttTokens>>,
    auth: Arc<Auth>,
    token: Option<String>,
) {
    let (mut sink, mut stream) = socket.split();
    let (sender, mut rx) = mpsc::channel::<ServerMsg>(OUTBOX_CAPACITY);
    let disconnect = Arc::new(Notify::new());
    let tx = Outbox::new(sender, disconnect.clone());

    let mut writer = tokio::spawn(async move {
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
            if !matches!(
                tokio::time::timeout(WRITE_TIMEOUT, sink.send(msg)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
    });

    // 입장했거나 대기 중인 (방 코드, 참가자 id). 실제 권한은 매 요청마다 Rooms가 확인한다.
    let mut session: Option<(String, PeerId)> = None;
    // 첫 STT 메시지에서 띄운다. 연결이 끝나 버려지면 진행 중 번역도 멈춘다.
    let mut speaker: Option<SpeakerHandle> = None;
    let mut stt_limit = TokenBucket::new(STT_PER_SEC, STT_BURST);
    let mut stt_token_limit = TokenBucket::new(STT_TOKENS_PER_SEC, STT_TOKEN_BURST);
    // 연결이 끝나면 진행 중인 발급도 취소한다. 받을 사람이 없는 토큰을 만들지 않게.
    let mut stt_tasks = tokio::task::JoinSet::new();

    let connected_at = tokio::time::Instant::now();
    let mut last_seen = connected_at;
    let mut session_check = tokio::time::interval(PING_INTERVAL);
    let mut writer_finished = false;
    loop {
        let deadline = if session.is_some() {
            last_seen + IDLE_TIMEOUT
        } else {
            connected_at + JOIN_TIMEOUT
        };
        let msg = tokio::select! {
            biased;
            _ = disconnect.notified() => break,
            _ = &mut writer => {
                writer_finished = true;
                break;
            }
            _ = session_check.tick() => {
                if token.as_ref().is_some_and(|token| auth.session_user(token).is_none()) {
                    break;
                }
                continue;
            }
            _ = tokio::time::sleep_until(deadline) => break,
            msg = stream.next() => match msg {
                Some(Ok(msg)) => msg,
                _ => break,
            },
        };
        last_seen = tokio::time::Instant::now();
        // 업그레이드 때의 계정 복사본을 권한으로 쓰지 않는다. 로그아웃·만료가 즉시 반영되어야 한다.
        let account = token.as_ref().and_then(|token| auth.session_user(token));
        if token.is_some() && account.is_none() {
            break;
        }
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
            // 입장 여부·번역 사용 여부는 파이프라인이 자막을 보낼 때마다 확인한다.
            (ClientMsg::Stt { seg, text, is_final }, Some((room, id))) => {
                if text.chars().count() > pipeline::MAX_STT_CHARS {
                    Err(ErrorCode::BadRequest)
                } else {
                    if stt_limit.take(Instant::now()) {
                        speaker
                            .get_or_insert_with(|| pipeline.speaker(room.clone(), id.clone()))
                            .push(seg, text, is_final);
                    }
                    Ok(())
                }
            }
            (ClientMsg::SttToken { id: req }, Some((room, id))) => {
                while stt_tasks.try_join_next().is_some() {}
                match (&stt, rooms.stt_lang(room, id)) {
                    (Some(issuer), Some(lang)) if stt_token_limit.take(Instant::now()) => {
                        // 발급은 Gemini 왕복(수백 ms)이라 연결 루프를 막지 않도록 따로 돌린다.
                        let issue = issuer.issue(&lang);
                        let tx = tx.clone();
                        stt_tasks.spawn(async move {
                            let msg = match issue.await {
                                Ok(grant) => ServerMsg::SttToken {
                                    id: req,
                                    token: grant.token,
                                    model: grant.model,
                                },
                                Err(err) => {
                                    tracing::warn!(%err, "stt token failed");
                                    ServerMsg::SttTokenDenied { id: req, retry: true }
                                }
                            };
                            tx.send(msg);
                        });
                    }
                    (Some(_), Some(_)) => tx.send(ServerMsg::SttTokenDenied { id: req, retry: true }),
                    // 키가 없거나, 입장 전이거나, 번역이 꺼진 방
                    _ => tx.send(ServerMsg::SttTokenDenied { id: req, retry: false }),
                }
                Ok(())
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
    stt_tasks.abort_all();
    drop(tx);
    writer.abort();
    if !writer_finished {
        let _ = writer.await;
    }
}

/// 초당 `rate`개, 최대 `burst`개까지 허용한다.
struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    at: Instant,
}

impl TokenBucket {
    fn new(rate: f64, burst: f64) -> Self {
        TokenBucket {
            rate,
            burst,
            tokens: burst,
            at: Instant::now(),
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

fn reply(tx: &Outbox, code: ErrorCode) {
    tx.send(ServerMsg::Error {
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
    fn stt_rate_is_limited() {
        let t0 = Instant::now();
        let mut bucket = TokenBucket::new(5.0, 10.0);
        assert_eq!((0..20).filter(|_| bucket.take(t0)).count(), 10);
        assert!(bucket.take(t0 + Duration::from_millis(200)));
        assert!(!bucket.take(t0 + Duration::from_millis(250)));
        assert_eq!(
            (0..20).filter(|_| bucket.take(t0 + Duration::from_secs(10))).count(),
            10
        );
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

    struct TestServer {
        addr: std::net::SocketAddr,
        rooms: Arc<Rooms>,
        auth: Arc<Auth>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    type TestSocket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    impl TestServer {
        async fn start() -> Self {
            Self::start_with(Some(Arc::new(
                crate::providers::live::testing::FakeTokens::default(),
            )))
            .await
        }

        async fn start_with(stt: Option<Arc<dyn SttTokens>>) -> Self {
            let rooms = Arc::new(Rooms::default());
            let auth = Arc::new(Auth::new(std::collections::HashMap::from([(
                "host".into(),
                auth::hash_password("test password").unwrap(),
            )])));
            let state = crate::AppState {
                pipeline: Arc::new(Pipeline::new(rooms.clone(), None)),
                stt,
                rooms: rooms.clone(),
                auth: auth.clone(),
                cookie_secure: false,
                trusted_proxy: false,
            };
            let app = axum::Router::new()
                .route("/ws", axum::routing::get(handler))
                .with_state(state);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self {
                addr,
                rooms,
                auth,
                task,
            }
        }

        async fn connect(&self, token: &str) -> TestSocket {
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut req = format!("ws://{}/ws", self.addr)
                .into_client_request()
                .unwrap();
            req.headers_mut().insert(
                header::ORIGIN,
                format!("http://{}", self.addr).parse().unwrap(),
            );
            req.headers_mut().insert(
                header::COOKIE,
                format!("{}={token}", auth::COOKIE_NAME).parse().unwrap(),
            );
            tokio_tungstenite::connect_async(req).await.unwrap().0
        }

        async fn token(&self) -> String {
            self.auth
                .login(self.addr.ip(), "host", "test password")
                .await
                .unwrap()
        }

        fn room(&self) -> String {
            self.rooms
                .create("host", crate::protocol::RoomSettings { translation: true })
                .unwrap()
        }
    }

    async fn send(socket: &mut TestSocket, msg: ClientMsg) {
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&msg).unwrap().into(),
            ))
            .await
            .unwrap();
    }

    async fn next_message(socket: &mut TestSocket) -> Option<ServerMsg> {
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(msg)) = socket.next().await {
                if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                    return Some(serde_json::from_str(&text).unwrap());
                }
            }
            None
        })
        .await
        .expect("socket should respond or close")
    }

    async fn join(socket: &mut TestSocket, room: &str) {
        send(
            socket,
            ClientMsg::Join {
                room: room.into(),
                name: "Host".into(),
                lang: "en".into(),
                partner_lang: "ko".into(),
                media: crate::protocol::MediaState::default(),
            },
        )
        .await;
    }

    async fn welcomed(server: &TestServer, room: &str) -> TestSocket {
        let token = server.token().await;
        let mut socket = server.connect(&token).await;
        join(&mut socket, room).await;
        assert!(matches!(
            next_message(&mut socket).await,
            Some(ServerMsg::Welcome { .. })
        ));
        socket
    }

    #[tokio::test]
    async fn stt_tokens_go_only_to_admitted_participants_in_translated_rooms() {
        let server = TestServer::start().await;
        let room = server.room();
        let mut host = welcomed(&server, &room).await;
        send(&mut host, ClientMsg::SttToken { id: 1 }).await;
        match next_message(&mut host).await {
            Some(ServerMsg::SttToken { id, token, model }) => {
                assert_eq!(id, 1);
                assert_eq!(token, "auth_tokens/fake-en", "issued for the participant's language");
                assert_eq!(model, "models/gemini-3.5-transcribe-live");
            }
            other => panic!("expected stt_token, got {other:?}"),
        }

        // 대기 중인 게스트(쿠키 없음)는 받지 못한다.
        let mut guest = server.connect("no-session").await;
        join(&mut guest, &room).await;
        assert!(matches!(next_message(&mut guest).await, Some(ServerMsg::Waiting)));
        send(&mut guest, ClientMsg::SttToken { id: 2 }).await;
        assert!(matches!(
            next_message(&mut guest).await,
            Some(ServerMsg::SttTokenDenied { retry: false, .. })
        ));

        // 번역을 끄면 주최자도 받지 못한다.
        send(
            &mut host,
            ClientMsg::Settings {
                settings: crate::protocol::RoomSettings { translation: false },
            },
        )
        .await;
        loop {
            match next_message(&mut host).await {
                Some(ServerMsg::Settings { .. }) => break,
                Some(_) => continue,
                None => panic!("closed"),
            }
        }
        send(&mut host, ClientMsg::SttToken { id: 1 }).await;
        assert!(matches!(
            next_message(&mut host).await,
            Some(ServerMsg::SttTokenDenied { retry: false, .. })
        ));
    }

    #[tokio::test]
    async fn stt_tokens_are_rate_limited_and_failures_are_retryable() {
        let server = TestServer::start().await;
        let room = server.room();
        let mut socket = welcomed(&server, &room).await;
        for _ in 0..STT_TOKEN_BURST as usize {
            send(&mut socket, ClientMsg::SttToken { id: 3 }).await;
            assert!(matches!(
                next_message(&mut socket).await,
                Some(ServerMsg::SttToken { .. })
            ));
        }
        send(&mut socket, ClientMsg::SttToken { id: 3 }).await;
        assert!(matches!(
            next_message(&mut socket).await,
            Some(ServerMsg::SttTokenDenied { retry: true, .. })
        ));

        let failing = crate::providers::live::testing::FakeTokens {
            fail: true,
            ..Default::default()
        };
        let server = TestServer::start_with(Some(Arc::new(failing))).await;
        let room = server.room();
        let mut socket = welcomed(&server, &room).await;
        send(&mut socket, ClientMsg::SttToken { id: 3 }).await;
        assert!(matches!(
            next_message(&mut socket).await,
            Some(ServerMsg::SttTokenDenied { retry: true, .. })
        ));

        let server = TestServer::start_with(None).await;
        let room = server.room();
        let mut socket = welcomed(&server, &room).await;
        send(&mut socket, ClientMsg::SttToken { id: 3 }).await;
        assert!(matches!(
            next_message(&mut socket).await,
            Some(ServerMsg::SttTokenDenied { retry: false, .. })
        ));
    }

    #[tokio::test]
    async fn logout_revokes_both_pending_host_join_and_existing_host_commands() {
        let server = TestServer::start().await;
        let room = server.room();
        let token = server.token().await;
        let mut socket = server.connect(&token).await;
        server.auth.logout(&token);
        join(&mut socket, &room).await;
        assert!(next_message(&mut socket).await.is_none());

        let token = server.token().await;
        let mut socket = server.connect(&token).await;
        join(&mut socket, &room).await;
        assert!(matches!(
            next_message(&mut socket).await,
            Some(ServerMsg::Welcome { host: true, .. })
        ));
        server.auth.logout(&token);
        send(
            &mut socket,
            ClientMsg::Settings {
                settings: crate::protocol::RoomSettings { translation: false },
            },
        )
        .await;
        assert!(next_message(&mut socket).await.is_none());
        assert!(server.rooms.settings(&room).unwrap().translation);
    }

    #[tokio::test]
    async fn stalled_client_is_disconnected_and_its_room_slot_is_released() {
        let server = TestServer::start().await;
        let room = server.room();
        let token = server.token().await;
        let mut socket = server.connect(&token).await;
        join(&mut socket, &room).await;
        assert!(matches!(
            next_message(&mut socket).await,
            Some(ServerMsg::Welcome { .. })
        ));
        tokio::time::pause();
        tokio::time::advance(IDLE_TIMEOUT + Duration::from_secs(1)).await;
        // Allow the server's expired read deadline to run before reading (and replying to) pings.
        tokio::task::yield_now().await;
        tokio::time::resume();
        assert!(next_message(&mut socket).await.is_none());
        server
            .rooms
            .sweep(std::time::Instant::now() + room::ROOM_IDLE_TTL + Duration::from_secs(1));
        assert!(server.rooms.settings(&room).is_none());
    }

    #[tokio::test]
    async fn overflow_closes_the_connection_instead_of_silently_losing_room_state() {
        let server = TestServer::start().await;
        let room = server.room();
        let token = server.token().await;
        let mut socket = server.connect(&token).await;
        join(&mut socket, &room).await;
        let Some(ServerMsg::Welcome { you, .. }) = next_message(&mut socket).await else {
            panic!("expected welcome");
        };
        // No await here: fill the outbox before the writer can drain it.
        for _ in 0..=OUTBOX_CAPACITY {
            server
                .rooms
                .set_settings(
                    &room,
                    &you,
                    crate::protocol::RoomSettings { translation: false },
                )
                .unwrap();
        }
        while next_message(&mut socket).await.is_some() {}
        server
            .rooms
            .sweep(std::time::Instant::now() + room::ROOM_IDLE_TTL + Duration::from_secs(1));
        assert!(server.rooms.settings(&room).is_none());
    }
}
