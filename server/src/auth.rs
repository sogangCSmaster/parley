//! 주최자 인증. 계정은 운영자가 `HOST_ACCOUNTS` 환경변수로 등록한다(DB 없음).
//! 세션은 프로세스 메모리에 두므로 서버를 재시작하면 다시 로그인해야 한다.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use axum::http::HeaderMap;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const COOKIE_NAME: &str = "tolkmeet_session";
const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// (클라이언트 IP, 아이디)별로 이 시간 안에 이만큼 실패하면 시간이 지날 때까지 로그인을 막는다.
/// IP별로 세므로 아이디를 아는 공격자가 다른 곳의 주최자를 잠글 수 없다.
const MAX_FAILURES: u32 = 10;
/// 여러 IP에 나눠 시도하는 공격을 막는 아이디별 상한. 정상 사용자가 닿지 않도록 넉넉하게 둔다.
const MAX_USER_FAILURES: u32 = 100;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// 실패 기록 맵의 최대 크기. 없는 아이디도 기록하므로 무한히 커지지 않게 막는다.
const MAX_TRACKED: usize = 10_000;
/// 동시에 돌리는 argon2 검증 수. 로그인 요청으로 CPU를 다 쓰게 만들 수 없게 한다.
const VERIFY_CONCURRENCY: usize = 4;
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

/// in_flight는 아직 검증 중인 시도다. 결과가 나오기 전에도 한도에 넣어야 동시 요청으로 한도를 넘길 수 없다.
struct Attempts {
    count: u32,
    in_flight: u32,
    window_start: Instant,
}

impl Attempts {
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.window_start) >= FAILURE_WINDOW
    }

    fn used(&self, now: Instant) -> u32 {
        let count = if self.expired(now) { 0 } else { self.count };
        count + self.in_flight
    }
}

struct Limiter<K> {
    max: u32,
    cap: usize,
    map: HashMap<K, Attempts>,
}

impl<K: Eq + Hash + Clone> Limiter<K> {
    fn new(max: u32, cap: usize) -> Self {
        Self {
            max,
            cap,
            map: HashMap::new(),
        }
    }

    fn is_full(&self, key: &K, now: Instant) -> bool {
        self.map.get(key).is_some_and(|a| a.used(now) >= self.max)
    }

    fn reserve(&mut self, key: K, now: Instant) {
        if !self.map.contains_key(&key) && self.map.len() >= self.cap {
            self.make_room(now);
        }
        let a = self.map.entry(key).or_insert(Attempts {
            count: 0,
            in_flight: 0,
            window_start: now,
        });
        if a.expired(now) {
            a.count = 0;
            a.window_start = now;
        }
        a.in_flight += 1;
    }

    fn finish(&mut self, key: &K, failed: bool) {
        let Some(a) = self.map.get_mut(key) else {
            return;
        };
        a.in_flight = a.in_flight.saturating_sub(1);
        a.count = if failed { a.count + 1 } else { 0 };
        if a.count == 0 && a.in_flight == 0 {
            self.map.remove(key);
        }
    }

    fn sweep(&mut self, now: Instant) {
        self.map.retain(|_, a| a.in_flight > 0 || !a.expired(now));
    }

    /// 실패가 가장 적고 오래된 기록부터 버린다. 무작위 아이디를 대량으로 보내
    /// 실패가 쌓인 기록을 밀어내는 방식으로 한도를 우회하지 못하게 하려는 것이다.
    /// 검증 중인 기록은 버리지 않는다(동시 요청 수만큼만 상한을 넘을 수 있다).
    fn make_room(&mut self, now: Instant) {
        self.sweep(now);
        if self.map.len() < self.cap {
            return;
        }
        let victim = self
            .map
            .iter()
            .filter(|(_, a)| a.in_flight == 0)
            .min_by_key(|(_, a)| (a.count, a.window_start))
            .map(|(k, _)| k.clone());
        if let Some(k) = victim {
            self.map.remove(&k);
        }
    }
}

/// 두 한도를 한 잠금 아래에서 확인·예약해야 동시 요청 사이에 틈이 생기지 않는다.
struct Throttle {
    by_client: Limiter<(IpAddr, String)>,
    by_user: Limiter<String>,
}

impl Throttle {
    fn new(cap: usize) -> Self {
        Self {
            by_client: Limiter::new(MAX_FAILURES, cap),
            by_user: Limiter::new(MAX_USER_FAILURES, cap),
        }
    }

    fn try_reserve(&mut self, client: IpAddr, username: &str, now: Instant) -> bool {
        let client_key = (client, username.to_owned());
        if self.by_client.is_full(&client_key, now)
            || self.by_user.is_full(&username.to_owned(), now)
        {
            return false;
        }
        self.by_client.reserve(client_key, now);
        self.by_user.reserve(username.to_owned(), now);
        true
    }

    fn finish(&mut self, client: IpAddr, username: &str, failed: bool) {
        self.by_client
            .finish(&(client, username.to_owned()), failed);
        self.by_user.finish(&username.to_owned(), failed);
    }

    fn sweep(&mut self, now: Instant) {
        self.by_client.sweep(now);
        self.by_user.sweep(now);
    }
}

/// 예약한 시도 슬롯. 요청이 중간에 취소돼도(클라이언트 연결 끊김) Drop에서 반납해
/// in_flight가 새어 계정이 영구히 잠기는 일을 막는다. 결과 없이 끝나면 실패로 센다.
struct Reservation<'a> {
    throttle: &'a Mutex<Throttle>,
    client: IpAddr,
    username: &'a str,
    succeeded: bool,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.throttle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .finish(self.client, self.username, !self.succeeded);
    }
}

pub struct Auth {
    accounts: HashMap<String, String>,
    sessions: Mutex<HashMap<String, Session>>,
    throttle: Mutex<Throttle>,
    verify_slots: Arc<Semaphore>,
    /// 없는 계정도 같은 시간이 걸리도록 비교에 쓰는 해시(계정 존재 여부를 응답 시간으로 알 수 없게).
    dummy_hash: String,
}

impl Auth {
    pub fn new(accounts: HashMap<String, String>) -> Self {
        let dummy_hash = hash_password("dummy-password-for-timing").expect("argon2 hashing works");
        Self {
            accounts,
            sessions: Mutex::default(),
            throttle: Mutex::new(Throttle::new(MAX_TRACKED)),
            verify_slots: Arc::new(Semaphore::new(VERIFY_CONCURRENCY)),
            dummy_hash,
        }
    }

    pub fn has_accounts(&self) -> bool {
        !self.accounts.is_empty()
    }

    /// 성공하면 새 세션 토큰을 돌려준다. 비밀번호 검증은 블로킹 스레드에서 돌려 런타임을 막지 않는다.
    /// 없는 아이디도 같은 방식으로 실패를 세므로 429 응답으로 계정 존재 여부를 알 수 없다.
    pub async fn login(
        &self,
        client: IpAddr,
        username: &str,
        password: &str,
    ) -> Result<String, LoginError> {
        // 형식에 맞지 않는 아이디는 어떤 계정과도 일치할 수 없다(형식은 공개 정보).
        // 해싱도 기록도 하지 않아 큰 키가 기록 맵에 쌓이지 않는다.
        if !is_valid_username(username) {
            return Err(LoginError::Invalid);
        }
        // 대기열도 제한한다. 검증 슬롯이 없으면 시도 기록을 소비하지 않고 재시도를 요청한다.
        let permit = self
            .verify_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| LoginError::Locked)?;
        let client = client_key(client);
        if !self
            .throttle
            .lock()
            .unwrap()
            .try_reserve(client, username, Instant::now())
        {
            return Err(LoginError::Locked);
        }
        let mut reservation = Reservation {
            throttle: &self.throttle,
            client,
            username,
            succeeded: false,
        };
        if !self.verify(username, password, permit).await {
            return Err(LoginError::Invalid);
        }
        reservation.succeeded = true;
        drop(reservation);

        let token = new_token();
        let session = Session {
            username: username.to_owned(),
            expires: Instant::now() + SESSION_TTL,
        };
        self.sessions.lock().unwrap().insert(token.clone(), session);
        Ok(token)
    }

    async fn verify(&self, username: &str, password: &str, permit: OwnedSemaphorePermit) -> bool {
        if password.len() > MAX_PASSWORD_BYTES {
            return false;
        }
        let known = self.accounts.get(username);
        let hash = known.unwrap_or(&self.dummy_hash).clone();
        let password = password.to_owned();
        let verified = tokio::task::spawn_blocking(move || {
            // HTTP 요청이 취소돼도 spawn_blocking은 계속되므로 실제 작업이 슬롯을 소유한다.
            let _permit = permit;
            verify_password(&password, &hash)
        })
        .await
        .unwrap_or(false);
        verified && known.is_some()
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
        self.throttle.lock().unwrap().sweep(now);
    }
}

/// 실패 기록에 쓰는 클라이언트 키. IPv6는 보통 /64 단위로 한 사용자에게 주어지므로
/// 주소를 바꿔 가며 한도를 피하지 못하게 /64로 묶는다.
fn client_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & (u128::MAX << 64))),
        },
        v4 => v4,
    }
}

/// 요청한 클라이언트의 IP. `X-Forwarded-For`는 누구나 보낼 수 있으므로 신뢰하는 프록시 뒤에서만
/// 읽고, 그 프록시가 덧붙인 마지막 값만 쓴다(앞쪽 값은 클라이언트가 조작할 수 있다).
pub fn client_ip(peer: IpAddr, headers: &HeaderMap, trust_proxy: bool) -> IpAddr {
    if !trust_proxy {
        return peer;
    }
    headers
        .get_all("x-forwarded-for")
        .iter()
        .next_back()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|ip| ip.trim().parse().ok())
        .unwrap_or(peer)
}

fn is_valid_username(user: &str) -> bool {
    (1..=32).contains(&user.len())
        && user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// `HOST_ACCOUNTS` 형식: 공백·줄바꿈으로 구분한 `아이디:argon2 PHC 해시` 목록.
/// PHC 문자열에는 쉼표가 들어가므로 구분자로 공백을 쓴다.
pub fn parse_accounts(raw: &str) -> Result<HashMap<String, String>, String> {
    let mut accounts = HashMap::new();
    for entry in raw.split_whitespace() {
        let (user, hash) = entry
            .split_once(':')
            .ok_or_else(|| format!("expected user:hash, got `{entry}`"))?;
        if !is_valid_username(user) {
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
    use std::net::Ipv4Addr;
    use std::sync::Arc;

    use super::*;

    const IP_A: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
    const IP_B: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));

    fn auth_with(user: &str, password: &str) -> Auth {
        let raw = format!("{user}:{}", hash_password(password).unwrap());
        Auth::new(parse_accounts(&raw).unwrap())
    }

    #[tokio::test]
    async fn login_succeeds_with_correct_password_and_creates_session() {
        let auth = auth_with("host", "correct horse");
        let token = auth.login(IP_A, "host", "correct horse").await.unwrap();
        assert_eq!(auth.session_user(&token).as_deref(), Some("host"));

        auth.logout(&token);
        assert_eq!(auth.session_user(&token), None);
    }

    #[tokio::test]
    async fn login_rejects_wrong_password_and_unknown_user() {
        let auth = auth_with("host", "correct horse");
        assert_eq!(
            auth.login(IP_A, "host", "wrong").await.unwrap_err(),
            LoginError::Invalid
        );
        assert_eq!(
            auth.login(IP_A, "nobody", "correct horse")
                .await
                .unwrap_err(),
            LoginError::Invalid
        );
        assert_eq!(auth.session_user("not-a-token"), None);
    }

    #[tokio::test]
    async fn repeated_failures_lock_the_account_even_for_correct_password() {
        let auth = auth_with("host", "correct horse");
        for _ in 0..MAX_FAILURES {
            let _ = auth.login(IP_A, "host", "wrong").await;
        }
        assert_eq!(
            auth.login(IP_A, "host", "correct horse").await.unwrap_err(),
            LoginError::Locked
        );
    }

    #[tokio::test]
    async fn parallel_attempts_cannot_exceed_the_failure_limit() {
        let auth = Arc::new(auth_with("host", "correct horse"));
        let attempts = (0..MAX_FAILURES * 3).map(|_| {
            let auth = auth.clone();
            tokio::spawn(async move { auth.login(IP_A, "host", "wrong").await })
        });
        let results = futures_util::future::join_all(attempts).await;
        let verified = results
            .iter()
            .filter(|r| matches!(r, Ok(Err(LoginError::Invalid))))
            .count();
        assert!((1..=MAX_FAILURES as usize).contains(&verified));
        for _ in verified..MAX_FAILURES as usize {
            assert_eq!(
                auth.login(IP_A, "host", "wrong").await,
                Err(LoginError::Invalid)
            );
        }
        assert_eq!(
            auth.login(IP_A, "host", "correct horse").await.unwrap_err(),
            LoginError::Locked
        );
    }

    #[tokio::test]
    async fn unknown_username_locks_out_like_a_real_account() {
        let auth = auth_with("host", "correct horse");
        for user in ["host", "nobody"] {
            for _ in 0..MAX_FAILURES {
                assert_eq!(
                    auth.login(IP_A, user, "wrong").await.unwrap_err(),
                    LoginError::Invalid
                );
            }
            assert_eq!(
                auth.login(IP_A, user, "wrong").await.unwrap_err(),
                LoginError::Locked
            );
        }
    }

    #[tokio::test]
    async fn lockout_is_per_client_ip() {
        let auth = auth_with("host", "correct horse");
        for _ in 0..MAX_FAILURES {
            let _ = auth.login(IP_A, "host", "wrong").await;
        }
        assert_eq!(
            auth.login(IP_A, "host", "correct horse").await.unwrap_err(),
            LoginError::Locked
        );
        assert!(auth.login(IP_B, "host", "correct horse").await.is_ok());
    }

    #[tokio::test]
    async fn per_username_ceiling_stops_attempts_spread_across_ips() {
        let auth = auth_with("host", "correct horse");
        for i in 0..MAX_USER_FAILURES / MAX_FAILURES {
            let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, i as u8));
            for _ in 0..MAX_FAILURES {
                assert_eq!(
                    auth.login(ip, "host", "wrong").await,
                    Err(LoginError::Invalid)
                );
            }
        }
        assert_eq!(
            auth.login(IP_B, "host", "correct horse").await.unwrap_err(),
            LoginError::Locked
        );
    }

    #[tokio::test]
    async fn busy_verification_rejects_without_recording_a_password_failure() {
        let auth = auth_with("host", "correct horse");
        let slots = auth
            .verify_slots
            .clone()
            .acquire_many_owned(VERIFY_CONCURRENCY as u32)
            .await
            .unwrap();
        assert_eq!(
            auth.login(IP_A, "host", "correct horse").await,
            Err(LoginError::Locked)
        );
        assert!(auth.throttle.lock().unwrap().by_client.map.is_empty());
        drop(slots);
        assert!(auth.login(IP_A, "host", "correct horse").await.is_ok());
    }

    #[test]
    fn cancelled_login_keeps_its_slot_until_the_blocking_job_finishes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let auth = Arc::new(auth_with("host", "correct horse"));
            let (release, hold) = std::sync::mpsc::channel();
            let (started, ready) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                hold.recv().unwrap();
            });
            ready.await.unwrap();
            let pending = tokio::spawn({
                let auth = auth.clone();
                async move { auth.login(IP_A, "host", "wrong").await }
            });
            while auth.verify_slots.available_permits() == VERIFY_CONCURRENCY {
                tokio::task::yield_now().await;
            }
            pending.abort();
            let _ = pending.await;
            let remaining = auth.verify_slots.available_permits();
            release.send(()).unwrap();
            blocker.await.unwrap();
            assert_eq!(remaining, VERIFY_CONCURRENCY - 1);
            tokio::time::timeout(Duration::from_secs(5), async {
                while auth.verify_slots.available_permits() != VERIFY_CONCURRENCY {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
    }

    #[test]
    fn eviction_keeps_records_with_the_most_failures() {
        let now = Instant::now();
        let mut limiter = Limiter::new(3, 2);
        for _ in 0..3 {
            limiter.reserve("target", now);
            limiter.finish(&"target", true);
        }
        for key in ["spam-1", "spam-2", "spam-3"] {
            limiter.reserve(key, now);
            limiter.finish(&key, true);
        }
        assert!(limiter.map.len() <= 2);
        assert!(limiter.is_full(&"target", now));
    }

    #[test]
    fn client_ip_uses_forwarded_header_only_behind_trusted_proxy() {
        let peer = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 203.0.113.1".parse().unwrap());
        assert_eq!(client_ip(peer, &headers, false), peer);
        assert_eq!(client_ip(peer, &headers, true), IP_A);
        assert_eq!(client_ip(peer, &HeaderMap::new(), true), peer);
    }

    #[test]
    fn ipv6_clients_are_grouped_by_64_prefix() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::2".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(client_key(a), client_key(b));
        assert_ne!(client_key(a), client_key(c));
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
            "theme=dark; tolkmeet_session=abc123; other=1"
                .parse()
                .unwrap(),
        );
        assert_eq!(token_from_headers(&headers).as_deref(), Some("abc123"));
    }
}
