//! 주최자 인증. 계정은 운영자가 `HOST_ACCOUNTS` 환경변수로 등록한다(DB 없음).
//! 세션은 프로세스 메모리에 두므로 서버를 재시작하면 다시 로그인해야 한다.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use axum::http::HeaderMap;

pub const COOKIE_NAME: &str = "parley_session";
const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// 계정별로 이 시간 안에 이만큼 실패하면 시간이 지날 때까지 로그인을 막는다.
const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// argon2는 비싸므로 비정상적으로 긴 입력은 해싱 전에 거른다.
const MAX_PASSWORD_BYTES: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum LoginError {
    Invalid,
    Locked,
}

struct Session {
    username: String,
    expires: Instant,
}

struct Failures {
    count: u32,
    window_start: Instant,
}

pub struct Auth {
    accounts: HashMap<String, String>,
    sessions: Mutex<HashMap<String, Session>>,
    failures: Mutex<HashMap<String, Failures>>,
    /// 없는 계정도 같은 시간이 걸리도록 비교에 쓰는 해시(계정 존재 여부를 응답 시간으로 알 수 없게).
    dummy_hash: String,
}

impl Auth {
    pub fn new(accounts: HashMap<String, String>) -> Self {
        let dummy_hash = hash_password("dummy-password-for-timing").expect("argon2 hashing works");
        Self {
            accounts,
            sessions: Mutex::default(),
            failures: Mutex::default(),
            dummy_hash,
        }
    }

    pub fn has_accounts(&self) -> bool {
        !self.accounts.is_empty()
    }

    /// 성공하면 새 세션 토큰을 돌려준다. 비밀번호 검증은 블로킹 스레드에서 돌려 런타임을 막지 않는다.
    pub async fn login(&self, username: &str, password: &str) -> Result<String, LoginError> {
        if self.is_locked(username) {
            return Err(LoginError::Locked);
        }
        let known = self.accounts.get(username);
        let hash = known.unwrap_or(&self.dummy_hash).clone();
        let password = password.to_owned();
        let verified = password.len() <= MAX_PASSWORD_BYTES
            && tokio::task::spawn_blocking(move || verify_password(&password, &hash))
                .await
                .unwrap_or(false);

        if !(verified && known.is_some()) {
            if known.is_some() {
                self.record_failure(username);
            }
            return Err(LoginError::Invalid);
        }

        self.failures.lock().unwrap().remove(username);
        let token = new_token();
        let session = Session {
            username: username.to_owned(),
            expires: Instant::now() + SESSION_TTL,
        };
        self.sessions.lock().unwrap().insert(token.clone(), session);
        Ok(token)
    }

    pub fn session_user(&self, token: &str) -> Option<String> {
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get(token) {
            Some(s) if s.expires > Instant::now() => Some(s.username.clone()),
            Some(_) => {
                sessions.remove(token);
                None
            }
            None => None,
        }
    }

    pub fn user_from_headers(&self, headers: &HeaderMap) -> Option<String> {
        token_from_headers(headers).and_then(|t| self.session_user(&t))
    }

    pub fn logout(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }

    /// 만료된 세션과 오래된 실패 기록을 정리한다(주기 작업).
    pub fn sweep(&self) {
        let now = Instant::now();
        self.sessions.lock().unwrap().retain(|_, s| s.expires > now);
        self.failures
            .lock()
            .unwrap()
            .retain(|_, f| now.duration_since(f.window_start) < FAILURE_WINDOW);
    }

    fn is_locked(&self, username: &str) -> bool {
        let failures = self.failures.lock().unwrap();
        failures
            .get(username)
            .is_some_and(|f| f.count >= MAX_FAILURES && f.window_start.elapsed() < FAILURE_WINDOW)
    }

    fn record_failure(&self, username: &str) {
        let mut failures = self.failures.lock().unwrap();
        let now = Instant::now();
        let entry = failures.entry(username.to_owned()).or_insert(Failures {
            count: 0,
            window_start: now,
        });
        if now.duration_since(entry.window_start) >= FAILURE_WINDOW {
            *entry = Failures {
                count: 0,
                window_start: now,
            };
        }
        entry.count += 1;
    }
}

/// `HOST_ACCOUNTS` 형식: 공백·줄바꿈으로 구분한 `아이디:argon2 PHC 해시` 목록.
/// PHC 문자열에는 쉼표가 들어가므로 구분자로 공백을 쓴다.
pub fn parse_accounts(raw: &str) -> Result<HashMap<String, String>, String> {
    let mut accounts = HashMap::new();
    for entry in raw.split_whitespace() {
        let (user, hash) = entry
            .split_once(':')
            .ok_or_else(|| format!("expected user:hash, got `{entry}`"))?;
        let valid_user = (1..=32).contains(&user.len())
            && user
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if !valid_user {
            return Err(format!("invalid username `{user}`"));
        }
        PasswordHash::new(hash).map_err(|e| format!("invalid hash for `{user}`: {e}"))?;
        accounts.insert(user.to_owned(), hash.to_owned());
    }
    Ok(accounts)
}

pub fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS RNG unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            pair.trim()
                .strip_prefix(COOKIE_NAME)?
                .strip_prefix('=')
                .map(str::to_owned)
        })
}

/// HttpOnly(스크립트 접근 차단) + SameSite=Lax(타 사이트 POST·WebSocket에 쿠키 미전송).
pub fn session_cookie(token: &str, secure: bool) -> String {
    let max_age = SESSION_TTL.as_secs();
    let secure = if secure { "; Secure" } else { "" };
    format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

pub fn clear_cookie(secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth_with(user: &str, password: &str) -> Auth {
        let raw = format!("{user}:{}", hash_password(password).unwrap());
        Auth::new(parse_accounts(&raw).unwrap())
    }

    #[tokio::test]
    async fn login_succeeds_with_correct_password_and_creates_session() {
        let auth = auth_with("host", "correct horse");
        let token = auth.login("host", "correct horse").await.unwrap();
        assert_eq!(auth.session_user(&token).as_deref(), Some("host"));

        auth.logout(&token);
        assert_eq!(auth.session_user(&token), None);
    }

    #[tokio::test]
    async fn login_rejects_wrong_password_and_unknown_user() {
        let auth = auth_with("host", "correct horse");
        assert_eq!(
            auth.login("host", "wrong").await.unwrap_err(),
            LoginError::Invalid
        );
        assert_eq!(
            auth.login("nobody", "correct horse").await.unwrap_err(),
            LoginError::Invalid
        );
        assert_eq!(auth.session_user("not-a-token"), None);
    }

    #[tokio::test]
    async fn repeated_failures_lock_the_account_even_for_correct_password() {
        let auth = auth_with("host", "correct horse");
        for _ in 0..MAX_FAILURES {
            let _ = auth.login("host", "wrong").await;
        }
        assert_eq!(
            auth.login("host", "correct horse").await.unwrap_err(),
            LoginError::Locked
        );
    }

    #[test]
    fn parses_accounts_and_rejects_malformed_entries() {
        let hash = hash_password("pw").unwrap();
        let accounts = parse_accounts(&format!("alice:{hash}\n bob:{hash}")).unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(parse_accounts("alice").is_err());
        assert!(parse_accounts("alice:not-a-hash").is_err());
        assert!(parse_accounts(&format!("bad user:{hash}")).is_err());
    }

    #[test]
    fn reads_session_token_from_cookie_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "theme=dark; parley_session=abc123; other=1".parse().unwrap(),
        );
        assert_eq!(token_from_headers(&headers).as_deref(), Some("abc123"));
    }
}
