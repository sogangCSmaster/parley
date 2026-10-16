//! 입장한 참가자에게 주는 ICE 서버 목록(STUN + 시간제한 TURN 자격).
//!
//! TURN은 coturn의 `use-auth-secret`(TURN REST API) 방식을 쓴다. 서버와 coturn이 비밀값 하나를 나눠 갖고,
//! 참가자마다 `username = "<만료 unix 초>:<참가자 id>"`, `credential = base64(HMAC-SHA1(secret, username))`를
//! 만들어 준다. coturn은 같은 계산으로 검증하고 만료가 지나면 거절하므로 자격이 새어도 오래 못 쓴다.
//! 자격은 Welcome으로만 나가므로 입장이 확정된 참가자만 받는다.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha1::Sha1;

use crate::protocol::IceServer;

/// coturn은 할당을 갱신(약 10분마다)할 때도 같은 자격을 검사한다. 만료가 지나면 회의 도중 릴레이가 끊기므로
/// 한 번의 회의보다 넉넉하게 잡는다(주최자 로그인 세션과 같은 12시간).
pub const TURN_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// coturn `static-auth-secret`. 짧으면 HMAC 키를 추측당할 수 있다.
const MIN_SECRET_LEN: usize = 32;

#[derive(Default)]
pub struct Ice {
    stun: Vec<IceServer>,
    turn: Option<Turn>,
}

struct Turn {
    urls: Vec<String>,
    secret: String,
}

impl Ice {
    /// `ICE_SERVERS`(자격 없는 STUN 등, JSON 배열), `TURN_URLS`(공백 구분), `TURN_SECRET`.
    pub fn new(
        ice_servers: Option<&str>,
        turn_urls: Option<&str>,
        turn_secret: Option<&str>,
    ) -> Result<Self, String> {
        let stun = match ice_servers {
            Some(raw) => serde_json::from_str::<Vec<StaticServer>>(raw)
                .map_err(|e| format!("ICE_SERVERS must be a JSON array of {{\"urls\": …}}: {e}"))?
                .into_iter()
                .map(StaticServer::into_server)
                .collect::<Result<_, _>>()?,
            None => vec![IceServer {
                urls: vec!["stun:stun.l.google.com:19302".into()],
                username: None,
                credential: None,
            }],
        };

        let urls: Vec<String> = turn_urls
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let secret = turn_secret.map(str::trim).unwrap_or_default();
        let turn = match (urls.is_empty(), secret.is_empty()) {
            (true, true) => None,
            (false, false) => {
                if secret.len() < MIN_SECRET_LEN {
                    return Err(format!(
                        "TURN_SECRET must be at least {MIN_SECRET_LEN} characters"
                    ));
                }
                if let Some(bad) = urls
                    .iter()
                    .find(|u| !(u.starts_with("turn:") || u.starts_with("turns:")))
                {
                    return Err(format!(
                        "TURN_URLS entries must start with turn: or turns: ({bad})"
                    ));
                }
                Some(Turn {
                    urls,
                    secret: secret.to_owned(),
                })
            }
            _ => return Err("TURN_URLS and TURN_SECRET must be set together".into()),
        };
        Ok(Self { stun, turn })
    }

    pub fn has_turn(&self) -> bool {
        self.turn.is_some()
    }

    /// 참가자 한 명에게 줄 목록. 자격에 참가자 id를 넣어 coturn 로그에서 누구의 릴레이인지 알 수 있게 한다.
    pub fn servers_for(&self, peer: &str, now: SystemTime) -> Vec<IceServer> {
        let mut servers = self.stun.clone();
        if let Some(turn) = &self.turn {
            let expiry = (now + TURN_TTL)
                .duration_since(UNIX_EPOCH)
                .expect("clock before 1970")
                .as_secs();
            let username = format!("{expiry}:{peer}");
            let credential = sign(&turn.secret, &username);
            servers.push(IceServer {
                urls: turn.urls.clone(),
                username: Some(username),
                credential: Some(credential),
            });
        }
        servers
    }
}

/// `ICE_SERVERS` 항목. 브라우저 형식 그대로 `"urls": "stun:…"`도 받는다.
#[derive(Deserialize)]
struct StaticServer {
    urls: Urls,
    username: Option<serde_json::Value>,
    credential: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Urls {
    One(String),
    Many(Vec<String>),
}

impl StaticServer {
    fn into_server(self) -> Result<IceServer, String> {
        // 정적 자격은 모든 참가자에게 같은 값이 나가 회의가 끝나도 계속 쓸 수 있다. TURN_SECRET으로 대신한다.
        if self.username.is_some() || self.credential.is_some() {
            return Err(
                "ICE_SERVERS must not contain static `username`/`credential`; \
                        use TURN_URLS + TURN_SECRET for time-limited TURN credentials"
                    .into(),
            );
        }
        let urls = match self.urls {
            Urls::One(u) => vec![u],
            Urls::Many(us) => us,
        };
        Ok(IceServer {
            urls,
            username: None,
            credential: None,
        })
    }
}

fn sign(secret: &str, username: &str) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(username.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn credential_matches_coturn_rest_scheme() {
        // printf '1700043200:p1' | openssl dgst -sha1 -hmac "$SECRET" -binary | base64
        let ice = Ice::new(
            Some("[]"),
            Some("turn:t.example:3478 turns:t.example:5349"),
            Some(SECRET),
        )
        .unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_700_043_200) - TURN_TTL;
        let servers = ice.servers_for("p1", now);
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers[0].urls,
            ["turn:t.example:3478", "turns:t.example:5349"]
        );
        assert_eq!(servers[0].username.as_deref(), Some("1700043200:p1"));
        assert_eq!(
            servers[0].credential.as_deref(),
            Some("riGNzwgvaYjtcAcc17l0cFZs28o=")
        );
    }

    #[test]
    fn stun_only_by_default() {
        let servers = Ice::new(None, None, None)
            .unwrap()
            .servers_for("p1", SystemTime::now());
        assert_eq!(servers.len(), 1);
        assert!(servers[0].username.is_none() && servers[0].credential.is_none());
    }

    #[test]
    fn accepts_single_url_string() {
        let ice = Ice::new(Some(r#"[{"urls":"stun:s.example:3478"}]"#), None, None).unwrap();
        assert_eq!(
            ice.servers_for("p", SystemTime::now())[0].urls,
            ["stun:s.example:3478"]
        );
    }

    #[test]
    fn rejects_unsafe_configs() {
        for (servers, urls, secret) in [
            (
                Some(r#"[{"urls":"turn:t:3478","username":"u","credential":"c"}]"#),
                None,
                None,
            ),
            (None, Some("turn:t:3478"), None),
            (None, None, Some(SECRET)),
            (None, Some("turn:t:3478"), Some("short")),
            (None, Some("stun:t:3478"), Some(SECRET)),
        ] {
            assert!(
                Ice::new(servers, urls, secret).is_err(),
                "{servers:?} {urls:?} {secret:?}"
            );
        }
    }
}
