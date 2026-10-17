//! 계정 HTTP API: 가입·이메일 인증·로그인(비밀번호·2차 인증·패스키·Google)·비밀번호 재설정·계정 설정.
//! 모든 요청 본문은 JSON(Content-Type: application/json)만 받으므로 다른 사이트의 단순 폼 전송(CSRF)은
//! 여기 닿지 못한다(세션 쿠키는 SameSite=Lax).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::auth::{
    self, LoginError, hash_password, is_valid_username, new_token, normalize_username,
};
use crate::db::{Db, OptionalExtension, now_secs, params};
use crate::limit::KeyedLimiter;
use crate::mail::{Copy, Lang, Mailer};
use crate::totp;
use crate::webauthn::{AuthenticationResponse, RegistrationResponse, WebAuthn};

const CODE_TTL: i64 = 15 * 60;
const RESET_TTL: i64 = 30 * 60;
const MAX_CODE_ATTEMPTS: i64 = 5;
const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 1024;
const OAUTH_TTL: Duration = Duration::from_secs(10 * 60);
const OAUTH_COOKIE: &str = "tolkmeet_oauth";

pub struct Accounts {
    pub mailer: Mailer,
    pub webauthn: WebAuthn,
    pub google: Option<Google>,
    /// 앱 주소(메일 속 링크, Google 되돌아올 주소, 패스키 출처 확인).
    pub origin: String,
    /// 가입·메일 다시 보내기·재설정 요청: 망(IPv6 /48)마다, IP마다, 주소마다.
    by_net: KeyedLimiter,
    by_ip: KeyedLimiter,
    by_email: KeyedLimiter,
    /// 초대 메일: 보내는 사람마다 하루 100통, 받는 사람마다 하루 5통.
    invites_by_user: KeyedLimiter,
    invites_by_rcpt: KeyedLimiter,
    /// 로그인한 뒤의 2차 인증 코드 확인(켜기·끄기·복구 코드): 사용자마다 15분에 10번, 하루 30번.
    totp_by_user: KeyedLimiter,
    totp_daily: KeyedLimiter,
    /// 로그인 때의 2차 인증 코드: (사용자, 클라이언트)마다 15분에 10번·하루 30번, 사용자 전체로 하루 100번.
    /// 로그인할 때마다 새 티켓이 나오므로 티켓마다 세는 것만으로는 비밀번호를 아는 사람이 계속 맞혀 볼 수 있다.
    /// 클라이언트마다 따로 세서, 비밀번호를 아는 다른 사람이 본인의 로그인까지 막기는 어렵게 한다.
    mfa_by_client: KeyedLimiter,
    mfa_daily_by_client: KeyedLimiter,
    mfa_daily: KeyedLimiter,
    /// 로그인 전 대기 상태를 만드는 요청(패스키 로그인 옵션·Google 시작): 클라이언트마다 1분에 30번.
    /// 한 곳에서 대기 상태를 잔뜩 만들어 다른 사람의 진행 중인 로그인을 밀어내지 못하게.
    sign_in_starts: KeyedLimiter,
    /// Google 로그인 진행 중: state → (nonce, 연결할 사용자, 화면 언어, 시작 시각)
    oauth: Mutex<HashMap<String, OAuthPending>>,
}

/// (nonce, 연결할 사용자, 화면 언어, 시작 시각)
type OAuthPending = (String, Option<i64>, String, Instant);

pub struct Google {
    pub client_id: String,
    pub client_secret: String,
    http: reqwest::Client,
}

impl Google {
    pub fn from_env() -> Option<Self> {
        let id = std::env::var("GOOGLE_CLIENT_ID")
            .ok()
            .filter(|v| !v.trim().is_empty())?;
        let secret = std::env::var("GOOGLE_CLIENT_SECRET")
            .ok()
            .filter(|v| !v.trim().is_empty())?;
        Some(Self {
            client_id: id.trim().to_owned(),
            client_secret: secret.trim().to_owned(),
            http: reqwest::Client::new(),
        })
    }
}

impl Accounts {
    pub fn new(mailer: Mailer, origin: String, google: Option<Google>) -> Self {
        Self {
            mailer,
            webauthn: WebAuthn::new(&origin),
            google,
            origin,
            by_net: KeyedLimiter::new(200, Duration::from_secs(3600)),
            by_ip: KeyedLimiter::new(20, Duration::from_secs(3600)),
            sign_in_starts: KeyedLimiter::new(30, Duration::from_secs(60)),
            by_email: KeyedLimiter::new(5, Duration::from_secs(3600)),
            invites_by_user: KeyedLimiter::new(100, Duration::from_secs(86_400)),
            invites_by_rcpt: KeyedLimiter::new(5, Duration::from_secs(86_400)),
            totp_by_user: KeyedLimiter::new(10, Duration::from_secs(15 * 60)),
            totp_daily: KeyedLimiter::new(30, Duration::from_secs(86_400)),
            mfa_by_client: KeyedLimiter::new(10, Duration::from_secs(15 * 60)),
            mfa_daily_by_client: KeyedLimiter::new(30, Duration::from_secs(86_400)),
            mfa_daily: KeyedLimiter::new(100, Duration::from_secs(86_400)),
            oauth: Mutex::default(),
        }
    }

    /// 초대 메일 한 통을 보내도 되는지(보내는 사람·받는 사람 한도).
    pub fn may_invite(&self, user_id: i64, to: &str) -> bool {
        // 보내는 사람 한도를 먼저 본다: 받는 주소(아무 값이나 넣을 수 있음)는 그 한도를 넘지 않은 요청만 맵에 쌓인다.
        self.invites_by_user.check(&user_id.to_string()) && self.invites_by_rcpt.check(to)
    }

    /// 메일을 보내는 요청의 빈도 제한. 넘으면 조용히 보내지 않는다(응답은 같게 해 계정 존재를 숨긴다).
    /// IPv6는 /64로 묶는다(주소를 바꿔 가며 피하지 못하게).
    // IP 한도를 먼저 본다(&&): 아무 값이나 넣을 수 있는 주소 키는 IP 한도를 넘지 않은 요청만 맵에 쌓인다.
    fn may_mail(&self, ip: IpAddr, email: &str) -> bool {
        // 터널 하나로 /48(/64 65,536개)을 받을 수 있으므로 더 넓은 단위를 먼저 본다.
        self.by_net.check(&net48(ip)) && self.by_ip.check(&auth::client_key(ip).to_string()) && self.by_email.check(email)
    }

    /// 로그인 전 대기 상태를 하나 더 만들어도 되는지.
    fn may_start_sign_in(&self, ip: IpAddr) -> bool {
        self.sign_in_starts.check(&auth::client_key(ip).to_string())
    }

    /// 로그인 중 이 클라이언트가 이 사용자의 2차 인증 코드를 한 번 더 맞혀 봐도 되는지.
    fn may_try_login_mfa(&self, user_id: i64, ip: IpAddr) -> bool {
        let key = format!("{user_id}:{}", auth::client_key(ip));
        self.mfa_by_client.check(&key)
            && self.mfa_daily_by_client.check(&key)
            && self.mfa_daily.check(&user_id.to_string())
    }

    /// 로그인한 사용자가 2차 인증 코드를 한 번 더 확인해 봐도 되는지.
    pub fn may_try_totp(&self, user_id: i64) -> bool {
        let key = user_id.to_string();
        self.totp_by_user.check(&key) && self.totp_daily.check(&key)
    }
}

/// 빈도 제한의 넓은 단위: IPv6는 /48(보통 한 사이트·터널에 주는 크기), IPv4는 그대로.
fn net48(ip: IpAddr) -> String {
    match auth::client_key(ip) {
        IpAddr::V6(v6) => std::net::Ipv6Addr::from_bits(v6.to_bits() & (u128::MAX << 80)).to_string(),
        v4 => v4.to_string(),
    }
}

// ── 공통 ──────────────────────────────────────────────

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "code": code }))).into_response()
}

fn client(state: &AppState, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    auth::client_ip(peer.ip(), headers, state.trusted_proxy)
}

fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

fn with_session(
    state: &AppState,
    token: &str,
    status: StatusCode,
    body: Option<Value>,
) -> Response {
    let cookie = [(
        header::SET_COOKIE,
        auth::session_cookie(token, state.cookie_secure),
    )];
    match body {
        Some(b) => (status, cookie, Json(b)).into_response(),
        None => (status, cookie).into_response(),
    }
}

fn valid_email(email: &str) -> bool {
    is_valid_username(email)
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
}

fn valid_password(p: &str) -> bool {
    (MIN_PASSWORD..=MAX_PASSWORD).contains(&p.chars().count())
}

fn clean_locale(l: &str) -> &'static str {
    match l {
        "ko" => "ko",
        "ja" => "ja",
        _ => "en",
    }
}

fn clean_name(n: &str) -> String {
    n.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect()
}

/// argon2 해싱은 블로킹 스레드에서.
async fn hash_async(password: &str) -> Option<String> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .ok()
        .and_then(Result::ok)
}

/// 유출된 비밀번호 목록(Have I Been Pwned)에 있는지. 비밀번호의 SHA-1 앞 5자만 보내고(k-익명성)
/// 받은 목록에서 나머지를 맞춰 본다. 응답이 없거나 늦으면(3초) 막지 않는다.
async fn breached(password: &str) -> bool {
    use sha1::{Digest, Sha1};
    let hex: String = Sha1::digest(password.as_bytes())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    let (prefix, suffix) = hex.split_at(5);
    // 연결을 다시 쓰게 하나만 만든다(요청마다 TLS 연결을 새로 맺지 않게).
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let res = HTTP
        .get_or_init(reqwest::Client::new)
        .get(format!("https://api.pwnedpasswords.com/range/{prefix}"))
        .header("Add-Padding", "true")
        .timeout(Duration::from_secs(3))
        .send()
        .await;
    let Ok(res) = res else { return false };
    let Ok(body) = res.text().await else {
        return false;
    };
    body.lines().any(|l| {
        l.split_once(':')
            .is_some_and(|(s, n)| s.eq_ignore_ascii_case(suffix) && n.trim() != "0")
    })
}

fn six_digit_code() -> String {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).expect("OS RNG unavailable");
    format!("{:06}", u32::from_le_bytes(b) % 1_000_000)
}

/// 코드·링크 토큰은 DB에 해시로만(사용자 id를 섞어 다른 계정의 코드와 겹치지 않게).
fn code_hash(user_id: i64, code: &str) -> String {
    auth::token_hash(&format!("{user_id}:{code}"))
}

struct UserRow {
    id: i64,
    locale: String,
    verified: bool,
    has_password: bool,
}

fn user_by_email(c: &rusqlite::Connection, email: &str) -> rusqlite::Result<Option<UserRow>> {
    c.query_row(
        "SELECT id, email, name, locale, email_verified, password_hash IS NOT NULL FROM users WHERE email = ?1",
        params![email],
        |r| {
            Ok(UserRow {
                id: r.get(0)?,
                locale: r.get(3)?,
                verified: r.get::<_, i64>(4)? != 0,
                has_password: r.get::<_, i64>(5)? != 0,
            })
        },
    )
    .optional()
}

/// 메일 코드를 새로 만든다. `payload`는 코드와 함께 확인할 값(가입 비밀번호 해시, 바꿀 새 이메일).
/// 인증 코드는 가입 요청마다 따로 남긴다(다른 사람이 같은 주소로 가입해도 원래 코드가 지워지지 않게).
async fn new_email_code(
    db: &Db,
    user_id: i64,
    kind: &'static str,
    payload: Option<String>,
) -> Result<String, rusqlite::Error> {
    let code = if kind == "reset" {
        new_token()
    } else {
        six_digit_code()
    };
    let hash = code_hash(user_id, &code);
    let ttl = if kind == "reset" { RESET_TTL } else { CODE_TTL };
    let now = now_secs();
    db.call(move |c| {
        if kind == "verify" {
            // 만료된 것과 오래된 것만 지우고 최근 5개는 남긴다.
            c.execute(
                "DELETE FROM email_codes WHERE user_id = ?1 AND kind = ?2 AND (expires_at <= ?3 OR id NOT IN
                   (SELECT id FROM email_codes WHERE user_id = ?1 AND kind = ?2 ORDER BY id DESC LIMIT 4))",
                params![user_id, kind, now],
            )?;
        } else {
            c.execute("DELETE FROM email_codes WHERE user_id = ?1 AND kind = ?2", params![user_id, kind])?;
        }
        c.execute(
            "INSERT INTO email_codes (user_id, kind, code_hash, expires_at, created_at, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![user_id, kind, hash, now + ttl, now, payload],
        )?;
        Ok(())
    })
    .await?;
    Ok(code)
}

fn send_verify_code(state: &AppState, email: String, locale: &str, code: &str) {
    let url = format!(
        "{}/verify?email={}&code={code}",
        state.accounts.origin,
        urlencode(&email)
    );
    state.accounts.mailer.send(
        Copy {
            lang: Lang::parse(locale),
        }
        .verify_code(&email, code, &url),
    );
}

// ── 가입·인증 ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct SignupReq {
    email: String,
    password: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    locale: String,
}

/// 가입. 이미 있는 주소여도 같은 응답(202)을 준다. 인증된 계정이면 대신 비밀번호 재설정 메일을 보낸다.
/// 비밀번호는 인증 코드와 함께 보관했다가, 코드와 그 비밀번호가 함께 맞을 때 계정에 넣는다
/// (같은 주소로 다른 사람이 가입해도 원래 가입자의 비밀번호를 바꿀 수 없게).
pub async fn signup(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<SignupReq>,
) -> Response {
    let email = normalize_username(&req.email);
    if !valid_email(&email) {
        return error(StatusCode::BAD_REQUEST, "email");
    }
    if !valid_password(&req.password) {
        return error(StatusCode::BAD_REQUEST, "password");
    }
    if breached(&req.password).await {
        return error(StatusCode::BAD_REQUEST, "breached");
    }
    let ip = client(&state, peer, &headers);
    if !state.accounts.may_mail(ip, &email) {
        return error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let locale = clean_locale(&req.locale).to_owned();
    let name = clean_name(&req.name);
    let Some(hash) = hash_async(&req.password).await else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    let db = state.auth.db().clone();
    let e = email.clone();
    let l = locale.clone();
    let result = db
        .call(move |c| match user_by_email(c, &e)? {
            Some(u) if u.verified => Ok((u.id, true, u.locale)),
            Some(u) => Ok((u.id, false, u.locale)),
            None => {
                c.execute(
                    "INSERT INTO users (email, name, display_name, locale, created_at) VALUES (?1, ?2, ?2, ?3, ?4)",
                    params![e, name, l, now_secs()],
                )?;
                Ok((c.last_insert_rowid(), false, l))
            }
        })
        .await;
    let Ok((user_id, existing, user_locale)) = result else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    if existing {
        if let Ok(token) = new_email_code(&db, user_id, "reset", None).await {
            let url = format!(
                "{}/reset?token={token}&uid={user_id}",
                state.accounts.origin
            );
            state.accounts.mailer.send(
                Copy {
                    lang: Lang::parse(&user_locale),
                }
                .reset_link(&email, &url),
            );
        }
    } else if let Ok(code) = new_email_code(&db, user_id, "verify", Some(hash)).await {
        send_verify_code(&state, email, &locale, &code);
    }
    StatusCode::ACCEPTED.into_response()
}

#[derive(Deserialize)]
pub struct VerifyReq {
    email: String,
    code: String,
    /// 가입할 때 넣은 비밀번호. 코드와 함께 맞아야 한다.
    password: String,
}

/// 이메일 인증. 코드가 맞고, 그 코드를 만든 가입의 비밀번호와 지금 넣은 비밀번호가 같으면
/// 그 비밀번호로 계정을 열고 바로 로그인시킨다. 틀린 시도는 코드마다 5번까지.
pub async fn verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<VerifyReq>,
) -> Response {
    let email = normalize_username(&req.email);
    let code = req.code.trim().to_owned();
    let db = state.auth.db().clone();
    let now = now_secs();
    let found = db
        .call(move |c| {
            let Some(u) = user_by_email(c, &email)? else { return Ok(None) };
            if u.verified {
                return Ok(None);
            }
            let want = code_hash(u.id, &code);
            let row: Option<(i64, Option<String>)> = c
                .query_row(
                    "SELECT id, payload FROM email_codes WHERE user_id = ?1 AND kind = 'verify' AND code_hash = ?2
                       AND expires_at > ?3 AND attempts < ?4",
                    params![u.id, want, now, MAX_CODE_ATTEMPTS],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if row.is_none() {
                // 틀린 코드: 이 계정의 살아 있는 코드 모두에 시도를 센다(코드 맞히기 제한).
                c.execute(
                    "UPDATE email_codes SET attempts = attempts + 1 WHERE user_id = ?1 AND kind = 'verify'",
                    params![u.id],
                )?;
            }
            Ok(row.map(|(id, payload)| (u.id, id, payload)))
        })
        .await;
    let Ok(Some((user_id, code_id, Some(hash)))) = found else {
        return error(StatusCode::BAD_REQUEST, "code");
    };
    let password = req.password.clone();
    let h = hash.clone();
    let matches = tokio::task::spawn_blocking(move || auth::verify_password(&password, &h))
        .await
        .unwrap_or(false);
    if !matches {
        let _ = db
            .call(move |c| {
                c.execute(
                    "UPDATE email_codes SET attempts = attempts + 1 WHERE id = ?1",
                    params![code_id],
                )
            })
            .await;
        return error(StatusCode::BAD_REQUEST, "password");
    }
    let opened = db
        .call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE users SET email_verified = 1, password_hash = ?2, password_changed_at = ?3 WHERE id = ?1 AND email_verified = 0",
                params![user_id, hash, now],
            )?;
            tx.execute("DELETE FROM email_codes WHERE user_id = ?1 AND kind = 'verify'", params![user_id])?;
            tx.commit()
        })
        .await;
    if opened.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    }
    let token = state
        .auth
        .create_session(user_id, &user_agent(&headers))
        .await;
    with_session(&state, &token, StatusCode::NO_CONTENT, None)
}

#[derive(Deserialize)]
pub struct EmailReq {
    email: String,
}

/// 인증 코드 다시 보내기. 가장 최근 가입 요청의 비밀번호를 그대로 이어 쓴다. 계정이 없거나 이미 인증됐어도 같은 응답.
pub async fn resend(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<EmailReq>,
) -> Response {
    let email = normalize_username(&req.email);
    let ip = client(&state, peer, &headers);
    if !state.accounts.may_mail(ip, &email) {
        return error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let db = state.auth.db().clone();
    let e = email.clone();
    let latest = db
        .call(move |c| {
            let Some(u) = user_by_email(c, &e)? else { return Ok(None) };
            if u.verified {
                return Ok(None);
            }
            let payload: Option<String> = c
                .query_row(
                    "SELECT payload FROM email_codes WHERE user_id = ?1 AND kind = 'verify' ORDER BY id DESC LIMIT 1",
                    params![u.id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            Ok(payload.map(|p| (u.id, u.locale, p)))
        })
        .await;
    if let Ok(Some((uid, locale, payload))) = latest
        && let Ok(code) = new_email_code(&db, uid, "verify", Some(payload)).await
    {
        send_verify_code(&state, email, &locale, &code);
    }
    StatusCode::NO_CONTENT.into_response()
}

// ── 로그인 ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginReq {
    #[serde(alias = "username")]
    email: String,
    password: String,
}

/// "이 기기는 30일 동안 2차 인증을 묻지 않기" 쿠키.
const TRUST_COOKIE: &str = "tolkmeet_trust";
const TRUST_TTL: i64 = 30 * 86_400;

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|p| {
            p.trim()
                .strip_prefix(name)?
                .strip_prefix('=')
                .map(str::to_owned)
        })
}

/// 이 브라우저가 그 사용자의 믿는 기기인지.
async fn trusted_device(db: &Db, headers: &HeaderMap, user_id: i64) -> bool {
    let Some(token) = cookie_value(headers, TRUST_COOKIE) else {
        return false;
    };
    let hash = auth::token_hash(&token);
    let now = now_secs();
    db.call(move |c| {
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM trusted_devices WHERE token_hash = ?1 AND user_id = ?2 AND expires_at > ?3)",
            params![hash, user_id, now],
            |r| r.get::<_, bool>(0),
        )
    })
    .await
    .unwrap_or(false)
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> Response {
    let ip = client(&state, peer, &headers);
    match state.auth.login(ip, &req.email, &req.password).await {
        Ok(token) => with_session(&state, &token, StatusCode::NO_CONTENT, None),
        Err(LoginError::Invalid) => error(StatusCode::UNAUTHORIZED, "invalid"),
        Err(LoginError::Locked) => error(StatusCode::TOO_MANY_REQUESTS, "locked"),
        Err(LoginError::Unverified) => error(StatusCode::FORBIDDEN, "unverified"),
        Err(LoginError::MfaRequired(ticket)) => {
            // 전에 "이 기기는 묻지 않기"를 고른 브라우저면 2차 인증을 건너뛴다.
            if let Some(uid) = state.auth.mfa_ticket_user(&ticket)
                && trusted_device(state.auth.db(), &headers, uid).await
            {
                state.auth.take_mfa_ticket(&ticket);
                let token = state.auth.create_session(uid, &user_agent(&headers)).await;
                return with_session(&state, &token, StatusCode::NO_CONTENT, None);
            }
            Json(json!({ "mfa": ticket })).into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct MfaReq {
    ticket: String,
    /// 인증 앱의 6자리 코드 또는 복구 코드(xxxx-xxxx)
    code: String,
    /// 이 기기는 30일 동안 다시 묻지 않기
    #[serde(default)]
    remember: bool,
}

/// 비밀번호 다음의 2차 인증 코드. 티켓 하나에 5번까지(넘으면 처음부터 다시 로그인).
pub async fn login_mfa(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<MfaReq>,
) -> Response {
    let Some(user_id) = state.auth.mfa_ticket_user(&req.ticket) else {
        return error(StatusCode::UNAUTHORIZED, "expired");
    };
    let ip = client(&state, peer, &headers);
    if !state.auth.count_mfa_attempt(&req.ticket) || !state.accounts.may_try_login_mfa(user_id, ip) {
        return error(StatusCode::TOO_MANY_REQUESTS, "locked");
    }
    let code = req.code.clone();
    let ok = state
        .auth
        .db()
        .call(move |c| Ok(check_totp(c, user_id, &code)? || use_recovery_code(c, user_id, &code)?))
        .await
        .unwrap_or(false);
    if !ok {
        return error(StatusCode::UNAUTHORIZED, "code");
    }
    state.auth.take_mfa_ticket(&req.ticket);
    let token = state
        .auth
        .create_session(user_id, &user_agent(&headers))
        .await;
    let mut cookies = vec![(
        header::SET_COOKIE,
        auth::session_cookie(&token, state.cookie_secure),
    )];
    if req.remember {
        let trust = new_token();
        let hash = auth::token_hash(&trust);
        let expires = now_secs() + TRUST_TTL;
        let saved = state
            .auth
            .db()
            .call(move |c| {
                c.execute(
                    "INSERT INTO trusted_devices (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
                    params![hash, user_id, expires],
                )
            })
            .await;
        if saved.is_ok() {
            let secure = if state.cookie_secure { "; Secure" } else { "" };
            cookies.push((
                header::SET_COOKIE,
                format!("{TRUST_COOKIE}={trust}; Path=/api; HttpOnly; SameSite=Lax; Max-Age={TRUST_TTL}{secure}"),
            ));
        }
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    for (k, v) in cookies {
        if let Ok(v) = v.parse() {
            res.headers_mut().append(k, v);
        }
    }
    res
}

/// 복구 코드(한 번만 쓸 수 있음). 휴대폰을 잃어버렸을 때 2차 인증 대신.
fn use_recovery_code(c: &rusqlite::Connection, user_id: i64, code: &str) -> rusqlite::Result<bool> {
    let normalized: String = code
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if normalized.len() != 10 {
        return Ok(false);
    }
    let hash = code_hash(user_id, &normalized);
    let n = c.execute(
        "UPDATE recovery_codes SET used_at = ?3 WHERE user_id = ?1 AND code_hash = ?2 AND used_at IS NULL",
        params![user_id, hash, now_secs()],
    )?;
    Ok(n == 1)
}

/// 복구 코드 10개를 새로 만든다(예전 것은 모두 버린다). 화면에는 이때 한 번만 보여 준다.
fn new_recovery_codes(c: &rusqlite::Connection, user_id: i64) -> rusqlite::Result<Vec<String>> {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    c.execute(
        "DELETE FROM recovery_codes WHERE user_id = ?1",
        params![user_id],
    )?;
    let mut out = Vec::new();
    for _ in 0..10 {
        let mut bytes = [0u8; 10];
        getrandom::fill(&mut bytes).expect("OS RNG unavailable");
        let raw: String = bytes
            .iter()
            .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
            .collect();
        c.execute(
            "INSERT INTO recovery_codes (user_id, code_hash) VALUES (?1, ?2)",
            params![user_id, code_hash(user_id, &raw)],
        )?;
        out.push(format!("{}-{}", &raw[..5], &raw[5..]));
    }
    Ok(out)
}

/// 켜져 있는 2차 인증 코드를 확인하고, 맞으면 쓴 시간 칸을 기록한다(같은 코드 재사용 방지).
fn check_totp(c: &rusqlite::Connection, user_id: i64, code: &str) -> rusqlite::Result<bool> {
    let row: Option<(Option<String>, i64)> = c
        .query_row(
            "SELECT totp_secret, totp_last_step FROM users WHERE id = ?1 AND totp_enabled = 1",
            params![user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((Some(secret), last)) = row else {
        return Ok(false);
    };
    match totp::verify(&secret, code, now_secs(), last) {
        Some(step) => {
            c.execute(
                "UPDATE users SET totp_last_step = ?2 WHERE id = ?1",
                params![user_id, step],
            )?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::token_from_headers(&headers) {
        state.auth.logout(&token);
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, auth::clear_cookie(state.cookie_secure))],
    )
        .into_response()
}

// ── 비밀번호 재설정 ───────────────────────────────────

/// 재설정 메일 요청. 계정이 없어도 같은 응답.
pub async fn forgot(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<EmailReq>,
) -> Response {
    let email = normalize_username(&req.email);
    let ip = client(&state, peer, &headers);
    if !state.accounts.may_mail(ip, &email) {
        return error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let db = state.auth.db().clone();
    let e = email.clone();
    if let Ok(Some(u)) = db.call(move |c| user_by_email(c, &e)).await
        && let Ok(token) = new_email_code(&db, u.id, "reset", None).await
    {
        let url = format!("{}/reset?token={token}&uid={}", state.accounts.origin, u.id);
        state.accounts.mailer.send(
            Copy {
                lang: Lang::parse(&u.locale),
            }
            .reset_link(&email, &url),
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub struct ResetReq {
    uid: i64,
    token: String,
    password: String,
}

/// 재설정 링크로 새 비밀번호. 메일함을 가진 것이 확인됐으므로 인증 완료로 바꾸고, 다른 세션은 모두 끊는다.
/// 2차 인증을 켠 계정은 바로 로그인시키지 않고 코드를 묻는다(메일함만 가진 사람이 들어오지 못하게).
pub async fn reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ResetReq>,
) -> Response {
    if !valid_password(&req.password) {
        return error(StatusCode::BAD_REQUEST, "password");
    }
    let want = code_hash(req.uid, &req.token);
    let uid = req.uid;
    let now = now_secs();
    // 비싼 일(유출 확인 요청·argon2)은 링크가 맞을 때만 한다(엉터리 요청으로 서버를 바쁘게 하지 못하게).
    let check = want.clone();
    let valid = state
        .auth
        .db()
        .call(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM email_codes WHERE user_id = ?1 AND kind = 'reset' AND code_hash = ?2 AND expires_at > ?3)",
                params![uid, check, now],
                |r| r.get::<_, bool>(0),
            )
        })
        .await
        .unwrap_or(false);
    if !valid {
        return error(StatusCode::BAD_REQUEST, "token");
    }
    if breached(&req.password).await {
        return error(StatusCode::BAD_REQUEST, "breached");
    }
    let Some(hash) = hash_async(&req.password).await else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    let result = state
        .auth
        .db()
        .call(move |c| {
            let found: Option<i64> = c
                .query_row(
                    "SELECT id FROM email_codes WHERE user_id = ?1 AND kind = 'reset' AND code_hash = ?2 AND expires_at > ?3",
                    params![uid, want, now],
                    |r| r.get(0),
                )
                .optional()?;
            if found.is_none() {
                return Ok(None);
            }
            c.execute("DELETE FROM email_codes WHERE user_id = ?1 AND kind = 'reset'", params![uid])?;
            c.execute(
                "UPDATE users SET password_hash = ?2, email_verified = 1, password_changed_at = ?3 WHERE id = ?1",
                params![uid, hash, now],
            )?;
            // 비밀번호를 바꿨으니 믿는 기기 기록도 지운다.
            c.execute("DELETE FROM trusted_devices WHERE user_id = ?1", params![uid])?;
            c.query_row("SELECT totp_enabled FROM users WHERE id = ?1", params![uid], |r| r.get::<_, bool>(0)).map(Some)
        })
        .await;
    let totp = match result {
        Ok(Some(totp)) => totp,
        Ok(None) => return error(StatusCode::BAD_REQUEST, "token"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    };
    state.auth.revoke_user_sessions(uid, None).await;
    if totp {
        return Json(json!({ "mfa": state.auth.new_mfa_ticket(uid) })).into_response();
    }
    let token = state.auth.create_session(uid, &user_agent(&headers)).await;
    with_session(&state, &token, StatusCode::NO_CONTENT, None)
}

// ── 내 정보·계정 설정 ─────────────────────────────────

/// 비로그인도 정상 응답(200, user: null)이다. 게스트가 매번 401 콘솔 오류를 보지 않도록.
pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let token = auth::token_from_headers(&headers);
    let user = match token.as_deref().and_then(|t| state.auth.session_user_id(t)) {
        Some(id) => {
            state.auth.touch(token.as_deref().unwrap_or("")).await;
            user_json(state.auth.db(), id, token.as_deref()).await
        }
        None => Value::Null,
    };
    Json(json!({
        "user": user,
        "features": {
            "google": state.accounts.google.is_some(),
            "billing": state.billing.is_some(),
            "devBilling": state.billing.is_none() && state.dev_mode,
        },
    }))
}

pub async fn user_json(db: &Db, id: i64, current_token: Option<&str>) -> Value {
    let current = current_token.map(auth::token_hash);
    db.call(move |c| {
        let user = c
            .query_row(
                "SELECT id, email, name, locale, email_verified, password_hash IS NOT NULL, google_sub IS NOT NULL,
                        totp_enabled, plan, plan_renews_at, plan_canceled, operator, created_at,
                        display_name, prefs, password_changed_at
                 FROM users WHERE id = ?1",
                params![id],
                |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "email": r.get::<_, String>(1)?,
                        "name": r.get::<_, String>(2)?,
                        "locale": r.get::<_, String>(3)?,
                        "verified": r.get::<_, i64>(4)? != 0,
                        "hasPassword": r.get::<_, i64>(5)? != 0,
                        "google": r.get::<_, i64>(6)? != 0,
                        "totp": r.get::<_, i64>(7)? != 0,
                        "plan": r.get::<_, String>(8)?,
                        "planRenewsAt": r.get::<_, Option<i64>>(9)?,
                        "planCanceled": r.get::<_, i64>(10)? != 0,
                        "operator": r.get::<_, i64>(11)? != 0,
                        "createdAt": r.get::<_, i64>(12)?,
                        "displayName": r.get::<_, String>(13)?,
                        "prefs": serde_json::from_str::<Value>(&r.get::<_, String>(14)?).unwrap_or(json!({})),
                        "passwordChangedAt": r.get::<_, Option<i64>>(15)?,
                    }))
                },
            )
            .optional()?;
        let Some(mut user) = user else { return Ok(Value::Null) };
        let mut stmt = c.prepare(
            "SELECT id, name, created_at, last_used_at FROM passkeys WHERE user_id = ?1 ORDER BY created_at",
        )?;
        let keys: Vec<Value> = stmt
            .query_map(params![id], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "name": r.get::<_, String>(1)?,
                    "createdAt": r.get::<_, i64>(2)?,
                    "lastUsedAt": r.get::<_, Option<i64>>(3)?,
                }))
            })?
            .collect::<rusqlite::Result<_>>()?;
        user["passkeys"] = Value::Array(keys);
        // 로그인된 기기: 세션 id는 토큰 해시의 앞 16자(본인에게만 보인다).
        let mut stmt = c.prepare(
            "SELECT token_hash, user_agent, created_at, last_seen_at FROM sessions WHERE user_id = ?1 ORDER BY COALESCE(last_seen_at, created_at) DESC",
        )?;
        let devices: Vec<Value> = stmt
            .query_map(params![id], |r| {
                let hash: String = r.get(0)?;
                Ok(json!({
                    "id": hash[..16].to_owned(),
                    "current": current.as_deref() == Some(hash.as_str()),
                    "userAgent": r.get::<_, String>(1)?,
                    "createdAt": r.get::<_, i64>(2)?,
                    "lastSeenAt": r.get::<_, Option<i64>>(3)?,
                }))
            })?
            .collect::<rusqlite::Result<_>>()?;
        user["devices"] = Value::Array(devices);
        let left: i64 = c.query_row(
            "SELECT count(*) FROM recovery_codes WHERE user_id = ?1 AND used_at IS NULL",
            params![id],
            |r| r.get(0),
        )?;
        user["recoveryLeft"] = json!(left);
        Ok(user)
    })
    .await
    .unwrap_or(Value::Null)
}

/// 로그인한 사용자 id. 없으면 401 응답.
#[allow(clippy::result_large_err)]
pub fn require_user(state: &AppState, headers: &HeaderMap) -> Result<i64, Response> {
    state
        .auth
        .user_id_from_headers(headers)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "login"))
}

/// 민감한 작업(비밀번호·이메일 변경, 계정 삭제, 2차 인증·패스키 설정) 전에 본인인지 다시 확인한다.
/// 비밀번호가 있으면 비밀번호로, 없으면(Google로만 가입) 최근 10분 안에 한 로그인이어야 한다.
/// 2차 인증 코드는 여기 쓰지 않는다: 세션만 가진 사람이 2차 인증을 스스로 켜고 그 코드로 통과하지 못하게.
async fn reauth(
    state: &AppState,
    uid: i64,
    headers: &HeaderMap,
    ip: IpAddr,
    password: &str,
) -> Result<(), Response> {
    let db = state.auth.db().clone();
    let has_password: bool = db
        .call(move |c| {
            c.query_row(
                "SELECT password_hash IS NOT NULL FROM users WHERE id = ?1",
                params![uid],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(true);
    if has_password {
        return match state.auth.check_password(ip, uid, password).await {
            Ok(()) => Ok(()),
            Err(LoginError::Locked) => Err(error(StatusCode::TOO_MANY_REQUESTS, "locked")),
            Err(_) => Err(error(StatusCode::UNAUTHORIZED, "current")),
        };
    }
    let hash = auth::token_from_headers(headers)
        .map(|t| auth::token_hash(&t))
        .unwrap_or_default();
    let created: i64 = db
        .call(move |c| {
            c.query_row(
                "SELECT created_at FROM sessions WHERE token_hash = ?1",
                params![hash],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(0);
    if now_secs() - created > FRESH_LOGIN {
        return Err(error(StatusCode::UNAUTHORIZED, "reauth"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileReq {
    name: Option<String>,
    display_name: Option<String>,
    locale: Option<String>,
    prefs: Option<Value>,
}

/// 기본 설정은 아는 키만, 정해진 값만 남긴다.
fn clean_prefs(v: &Value) -> Value {
    let mut out = serde_json::Map::new();
    let lang = |k: &str| v[k].as_str().filter(|l| room_lang(l)).map(|l| json!(l));
    if let Some(l) = lang("lang") {
        out.insert("lang".into(), l);
    }
    if let Some(l) = lang("partnerLang") {
        out.insert("partnerLang".into(), l);
    }
    if let Some(t) = v["theme"]
        .as_str()
        .filter(|t| matches!(*t, "system" | "light" | "dark"))
    {
        out.insert("theme".into(), json!(t));
    }
    if let Some(t) = v["captionSize"]
        .as_str()
        .filter(|t| matches!(*t, "s" | "m" | "l"))
    {
        out.insert("captionSize".into(), json!(t));
    }
    if let Some(b) = v["savePrompt"].as_bool() {
        out.insert("savePrompt".into(), json!(b));
    }
    Value::Object(out)
}

fn room_lang(l: &str) -> bool {
    crate::room::is_valid_lang(l)
}

pub async fn update_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ProfileReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let name = req.name.as_deref().map(clean_name);
    let display = req.display_name.as_deref().map(clean_name);
    let locale = req.locale.as_deref().map(|l| clean_locale(l).to_owned());
    let prefs = req.prefs.as_ref().map(clean_prefs);
    let db = state.auth.db().clone();
    let res = db
        .call(move |c| {
            if let Some(n) = name {
                c.execute("UPDATE users SET name = ?2 WHERE id = ?1", params![uid, n])?;
            }
            if let Some(n) = display {
                c.execute(
                    "UPDATE users SET display_name = ?2 WHERE id = ?1",
                    params![uid, n],
                )?;
            }
            if let Some(l) = locale {
                c.execute(
                    "UPDATE users SET locale = ?2 WHERE id = ?1",
                    params![uid, l],
                )?;
            }
            if let Some(p) = prefs {
                // 보낸 키만 바꾼다(나머지는 그대로).
                let old: String =
                    c.query_row("SELECT prefs FROM users WHERE id = ?1", params![uid], |r| {
                        r.get(0)
                    })?;
                let mut merged: Value = serde_json::from_str(&old).unwrap_or(json!({}));
                if let (Some(m), Some(new)) = (merged.as_object_mut(), p.as_object()) {
                    for (k, v) in new {
                        m.insert(k.clone(), v.clone());
                    }
                }
                c.execute(
                    "UPDATE users SET prefs = ?2 WHERE id = ?1",
                    params![uid, merged.to_string()],
                )?;
            }
            Ok(())
        })
        .await;
    let token = auth::token_from_headers(&headers);
    match res {
        Ok(()) => Json(user_json(&db, uid, token.as_deref()).await).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

#[derive(Deserialize)]
pub struct EmailChangeReq {
    email: String,
    /// 비밀번호가 있는 계정: 지금 비밀번호(없는 계정은 방금 한 로그인으로 확인)
    #[serde(default)]
    password: String,
}

/// 비밀번호도 2차 인증도 없는 계정(Google로만 가입)은 최근 10분 안에 로그인한 세션이어야
/// 민감한 변경을 할 수 있다(오래 열어 둔 남의 컴퓨터에서 계정을 넘기지 못하게).
const FRESH_LOGIN: i64 = 10 * 60;

/// 이메일 바꾸기 1단계: 새 주소로 코드를 보낸다(비밀번호가 있는 계정은 비밀번호 확인).
pub async fn email_change(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<EmailChangeReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let email = normalize_username(&req.email);
    if !valid_email(&email) {
        return error(StatusCode::BAD_REQUEST, "email");
    }
    let ip = client(&state, peer, &headers);
    if !state.accounts.may_mail(ip, &email) {
        return error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let db = state.auth.db().clone();
    let e = email.clone();
    let info = db
        .call(move |c| {
            let taken: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE email = ?1)",
                params![e],
                |r| r.get(0),
            )?;
            let locale: String = c.query_row(
                "SELECT locale FROM users WHERE id = ?1",
                params![uid],
                |r| r.get(0),
            )?;
            Ok((taken, locale))
        })
        .await;
    let Ok((taken, locale)) = info else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    if let Err(r) = reauth(&state, uid, &headers, ip, &req.password).await {
        return r;
    }
    if taken {
        return error(StatusCode::CONFLICT, "taken");
    }
    match new_email_code(&db, uid, "email", Some(email.clone())).await {
        Ok(code) => {
            state.accounts.mailer.send(
                Copy {
                    lang: Lang::parse(&locale),
                }
                .email_change_code(&email, &code),
            );
            StatusCode::ACCEPTED.into_response()
        }
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

#[derive(Deserialize)]
pub struct CodeOnly {
    code: String,
}

/// 이메일 바꾸기 2단계: 새 주소로 받은 코드가 맞으면 바꾼다.
pub async fn email_change_verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CodeOnly>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let want = code_hash(uid, req.code.trim());
    let now = now_secs();
    let db = state.auth.db().clone();
    let res = db
        .call(move |c| {
            let row: Option<(i64, Option<String>)> = c
                .query_row(
                    "SELECT id, payload FROM email_codes WHERE user_id = ?1 AND kind = 'email' AND code_hash = ?2
                       AND expires_at > ?3 AND attempts < ?4",
                    params![uid, want, now, MAX_CODE_ATTEMPTS],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((_, Some(email))) = row else {
                c.execute("UPDATE email_codes SET attempts = attempts + 1 WHERE user_id = ?1 AND kind = 'email'", params![uid])?;
                return Ok(Err("code"));
            };
            c.execute("DELETE FROM email_codes WHERE user_id = ?1 AND kind = 'email'", params![uid])?;
            // 그사이 다른 계정이 그 주소를 가져갔으면 실패.
            let (old, locale): (String, String) =
                c.query_row("SELECT email, locale FROM users WHERE id = ?1", params![uid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            match c.execute("UPDATE users SET email = ?2, email_verified = 1 WHERE id = ?1", params![uid, email]) {
                Ok(_) => {
                    c.execute("DELETE FROM trusted_devices WHERE user_id = ?1", params![uid])?;
                    Ok(Ok((old, email, locale)))
                }
                Err(_) => Ok(Err("taken")),
            }
        })
        .await;
    let token = auth::token_from_headers(&headers);
    match res {
        Ok(Ok((old, new, locale))) => {
            // 예전 주소로 알린다(본인이 아니면 바로 알아차리게). 다른 기기의 로그인은 끊는다.
            state.accounts.mailer.send(
                Copy {
                    lang: Lang::parse(&locale),
                }
                .email_changed_notice(&old, &new),
            );
            state.auth.revoke_user_sessions(uid, token.as_deref()).await;
            Json(user_json(&db, uid, token.as_deref()).await).into_response()
        }
        Ok(Err(code)) => error(StatusCode::BAD_REQUEST, code),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

/// 로그인된 기기 하나를 로그아웃시킨다(id = 세션 토큰 해시 앞 16자).
pub async fn revoke_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return error(StatusCode::BAD_REQUEST, "id");
    }
    if state.auth.revoke_session_prefix(uid, &id).await {
        forget_trusted_devices(state.auth.db(), uid).await;
        StatusCode::NO_CONTENT.into_response()
    } else {
        error(StatusCode::NOT_FOUND, "missing")
    }
}

#[derive(Deserialize)]
pub struct PasswordReq {
    #[serde(default)]
    current: String,
    new: String,
}

/// 비밀번호 바꾸기(없으면 새로 만들기). 바꾸면 지금 세션 말고는 모두 끊는다.
pub async fn change_password(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<PasswordReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if !valid_password(&req.new) {
        return error(StatusCode::BAD_REQUEST, "password");
    }
    let db = state.auth.db().clone();
    if let Err(r) = reauth(
        &state,
        uid,
        &headers,
        client(&state, peer, &headers),
        &req.current,
    )
    .await
    {
        return r;
    }
    if breached(&req.new).await {
        return error(StatusCode::BAD_REQUEST, "breached");
    }
    let Some(hash) = hash_async(&req.new).await else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    let now = now_secs();
    if db
        .call(move |c| {
            c.execute(
                "UPDATE users SET password_hash = ?2, password_changed_at = ?3 WHERE id = ?1",
                params![uid, hash, now],
            )?;
            c.execute(
                "DELETE FROM trusted_devices WHERE user_id = ?1",
                params![uid],
            )
        })
        .await
        .is_err()
    {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    }
    let token = auth::token_from_headers(&headers);
    state.auth.revoke_user_sessions(uid, token.as_deref()).await;
    StatusCode::NO_CONTENT.into_response()
}

/// 지금 기기 말고 다른 곳의 로그인을 모두 끊는다.
pub async fn revoke_other_sessions(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let token = auth::token_from_headers(&headers);
    state.auth.revoke_user_sessions(uid, token.as_deref()).await;
    forget_trusted_devices(state.auth.db(), uid).await;
    StatusCode::NO_CONTENT.into_response()
}

/// 2차 인증을 건너뛰던 기기 기록을 모두 지운다(다른 기기를 로그아웃시킬 때 함께).
async fn forget_trusted_devices(db: &Db, uid: i64) {
    let _ = db
        .call(move |c| {
            c.execute(
                "DELETE FROM trusted_devices WHERE user_id = ?1",
                params![uid],
            )
        })
        .await;
}

#[derive(Deserialize)]
pub struct DeleteReq {
    #[serde(default)]
    password: String,
    /// 비밀번호가 없는 계정(Google 가입)은 이메일 주소를 다시 입력해 확인한다.
    #[serde(default)]
    confirm: String,
}

/// 계정 삭제: 회의·사용량·패스키·세션까지 함께 지운다(외래키 CASCADE).
pub async fn delete_account(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<DeleteReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let db = state.auth.db().clone();
    let Ok(Some((email, has_password, subscription))) = db
        .call(move |c| {
            c.query_row(
                "SELECT email, password_hash IS NOT NULL, paddle_subscription_id FROM users WHERE id = ?1",
                params![uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, Option<String>>(2)?)),
            )
            .optional()
        })
        .await
    else {
        return error(StatusCode::NOT_FOUND, "missing");
    };
    if let Err(r) = reauth(
        &state,
        uid,
        &headers,
        client(&state, peer, &headers),
        &req.password,
    )
    .await
    {
        return r;
    }
    // 실수로 누르지 않게 비밀번호가 없는 계정은 이메일 주소를 다시 쓰게 한다.
    if !has_password && normalize_username(&req.confirm) != email {
        return error(StatusCode::BAD_REQUEST, "confirm");
    }
    // 결제 중인 구독은 먼저 해지한다(실패하면 지우지 않는다: 지운 뒤에도 결제가 계속되면 안 된다).
    if let (Some(billing), Some(sub)) = (&state.billing, subscription)
        && billing.cancel_now(&sub).await.is_err()
    {
        return error(StatusCode::BAD_GATEWAY, "billing");
    }
    state.auth.revoke_user_sessions(uid, None).await;
    if db
        .call(move |c| c.execute("DELETE FROM users WHERE id = ?1", params![uid]))
        .await
        .is_err()
    {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, auth::clear_cookie(state.cookie_secure))],
    )
        .into_response()
}

/// 내 데이터 내보내기(JSON 파일). 비밀번호 해시·2차 인증 키·세션은 넣지 않는다.
pub async fn export(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let db = state.auth.db().clone();
    let user = user_json(&db, uid, None).await;
    let meetings = db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT code, title, kind, lang_a, lang_b, starts_at, duration_min, guests, created_at, started_at, ended_at, translated_seconds
                 FROM meetings WHERE owner_id = ?1 ORDER BY created_at",
            )?;
            stmt.query_map(params![uid], |r| {
                Ok(json!({
                    "code": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?, "kind": r.get::<_, String>(2)?,
                    "langA": r.get::<_, String>(3)?, "langB": r.get::<_, String>(4)?, "startsAt": r.get::<_, Option<i64>>(5)?,
                    "durationMin": r.get::<_, i64>(6)?, "guests": serde_json::from_str::<Value>(&r.get::<_, String>(7)?).unwrap_or(Value::Null),
                    "createdAt": r.get::<_, i64>(8)?, "startedAt": r.get::<_, Option<i64>>(9)?, "endedAt": r.get::<_, Option<i64>>(10)?,
                    "translatedSeconds": r.get::<_, i64>(11)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .unwrap_or_default();
    let body = json!({ "exportedAt": now_secs(), "account": user, "meetings": meetings });
    (
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"tolkmeet-data.json\"".to_owned(),
            ),
        ],
        serde_json::to_string_pretty(&body).unwrap_or_default(),
    )
        .into_response()
}

// ── 2차 인증(TOTP) ────────────────────────────────────

/// 새 키를 만들어 저장만 하고(아직 꺼짐), 인증 앱에 넣을 QR·키를 준다.
#[derive(Deserialize, Default)]
pub struct ReauthReq {
    #[serde(default)]
    password: String,
    /// Google 연결에서만: 메일 등에 쓸 화면 언어
    #[serde(default)]
    locale: String,
}

pub async fn totp_setup(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ReauthReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if let Err(r) = reauth(
        &state,
        uid,
        &headers,
        client(&state, peer, &headers),
        &req.password,
    )
    .await
    {
        return r;
    }
    let secret = totp::new_secret();
    let s = secret.clone();
    let email = state
        .auth
        .db()
        .call(move |c| {
            let enabled: bool = c.query_row(
                "SELECT totp_enabled FROM users WHERE id = ?1",
                params![uid],
                |r| r.get(0),
            )?;
            if enabled {
                return Ok(None);
            }
            c.execute(
                "UPDATE users SET totp_secret = ?2 WHERE id = ?1",
                params![uid, s],
            )?;
            c.query_row("SELECT email FROM users WHERE id = ?1", params![uid], |r| {
                r.get::<_, String>(0)
            })
            .map(Some)
        })
        .await;
    match email {
        Ok(Some(email)) => {
            let uri = totp::otpauth_uri(&secret, &email);
            Json(json!({ "secret": secret, "uri": uri, "qr": totp::qr_svg(&uri) })).into_response()
        }
        Ok(None) => error(StatusCode::CONFLICT, "enabled"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

#[derive(Deserialize)]
pub struct CodeReq {
    code: String,
}

/// 앱에 넣은 뒤 나온 첫 코드로 확인해야 켜진다. 켜지면 복구 코드 10개를 한 번만 보여 준다.
pub async fn totp_enable(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CodeReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    // 6자리 코드를 무차별로 맞혀 보지 못하게 다른 인증 앱 확인과 같은 한도를 둔다.
    if !state.accounts.may_try_totp(uid) {
        return error(StatusCode::TOO_MANY_REQUESTS, "locked");
    }
    let code = req.code.clone();
    let codes = state
        .auth
        .db()
        .call(move |c| {
            // 켜는 중인(아직 꺼진) 비밀 키에만. 이미 켜져 있으면 복구 코드 다시 만들기를 쓴다.
            let secret: Option<String> = c
                .query_row(
                    "SELECT totp_secret FROM users WHERE id = ?1 AND totp_enabled = 0",
                    params![uid],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let Some(secret) = secret else {
                return Ok(None);
            };
            match totp::verify(&secret, &code, now_secs(), 0) {
                Some(step) => {
                    c.execute(
                        "UPDATE users SET totp_enabled = 1, totp_last_step = ?2 WHERE id = ?1",
                        params![uid, step],
                    )?;
                    new_recovery_codes(c, uid).map(Some)
                }
                None => Ok(None),
            }
        })
        .await
        .ok()
        .flatten();
    match codes {
        Some(codes) => Json(json!({ "recoveryCodes": codes })).into_response(),
        None => error(StatusCode::BAD_REQUEST, "code"),
    }
}

/// 복구 코드를 새로 만든다(지금 인증 앱 코드로 확인). 예전 코드는 쓸 수 없게 된다.
pub async fn totp_recovery(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CodeReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if !state.accounts.may_try_totp(uid) {
        return error(StatusCode::TOO_MANY_REQUESTS, "locked");
    }
    let code = req.code.clone();
    let codes = state
        .auth
        .db()
        .call(move |c| {
            if !check_totp(c, uid, &code)? {
                return Ok(None);
            }
            new_recovery_codes(c, uid).map(Some)
        })
        .await
        .ok()
        .flatten();
    match codes {
        Some(codes) => Json(json!({ "recoveryCodes": codes })).into_response(),
        None => error(StatusCode::BAD_REQUEST, "code"),
    }
}

/// 끄려면 지금 코드를 한 번 더 넣는다(세션만 훔친 사람이 끄지 못하게).
pub async fn totp_disable(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CodeReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if !state.accounts.may_try_totp(uid) {
        return error(StatusCode::TOO_MANY_REQUESTS, "locked");
    }
    let code = req.code.clone();
    let ok = state
        .auth
        .db()
        .call(move |c| {
            if !check_totp(c, uid, &code)? {
                return Ok(false);
            }
            c.execute(
                "DELETE FROM recovery_codes WHERE user_id = ?1",
                params![uid],
            )?;
            c.execute(
                "DELETE FROM trusted_devices WHERE user_id = ?1",
                params![uid],
            )?;
            c.execute(
                "UPDATE users SET totp_enabled = 0, totp_secret = NULL WHERE id = ?1",
                params![uid],
            )?;
            Ok(true)
        })
        .await
        .unwrap_or(false);
    if ok {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error(StatusCode::BAD_REQUEST, "code")
    }
}

// ── 패스키 ───────────────────────────────────────────

pub async fn passkey_register_options(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ReauthReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if let Err(r) = reauth(
        &state,
        uid,
        &headers,
        client(&state, peer, &headers),
        &req.password,
    )
    .await
    {
        return r;
    }
    let info = state
        .auth
        .db()
        .call(move |c| {
            let (email, name): (String, String) = c.query_row(
                "SELECT email, name FROM users WHERE id = ?1",
                params![uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut stmt = c.prepare("SELECT id FROM passkeys WHERE user_id = ?1")?;
            let ids = stmt
                .query_map(params![uid], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((email, name, ids))
        })
        .await;
    match info {
        Ok((email, name, ids)) => Json(
            state
                .accounts
                .webauthn
                .registration_options(uid, &email, &name, &ids),
        )
        .into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

#[derive(Deserialize)]
pub struct PasskeyRegisterReq {
    credential: RegistrationResponse,
    #[serde(default)]
    name: String,
}

pub async fn passkey_register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PasskeyRegisterReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let (owner, cred) = match state.accounts.webauthn.finish_registration(&req.credential) {
        Ok(v) => v,
        Err(why) => {
            tracing::info!(why, "passkey registration rejected");
            return error(StatusCode::BAD_REQUEST, "passkey");
        }
    };
    // 챌린지를 받은 사용자와 지금 세션의 사용자가 같아야 한다.
    if owner != uid {
        return error(StatusCode::FORBIDDEN, "passkey");
    }
    let name: String = clean_name(&req.name);
    let now = now_secs();
    let res = state
        .auth
        .db()
        .call(move |c| {
            c.execute(
                "INSERT INTO passkeys (id, user_id, public_key, sign_count, name, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![cred.id, uid, cred.public_key, cred.sign_count, name, now],
            )
        })
        .await;
    match res {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => error(StatusCode::CONFLICT, "exists"),
    }
}

pub async fn passkey_login_options(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !state.accounts.may_start_sign_in(client(&state, peer, &headers)) {
        return error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    Json(state.accounts.webauthn.login_options()).into_response()
}

#[derive(Deserialize)]
pub struct PasskeyLoginReq {
    credential: AuthenticationResponse,
}

/// 패스키 로그인. 패스키는 기기 잠금(생체·PIN)까지 거치므로 2차 인증을 따로 묻지 않는다.
pub async fn passkey_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PasskeyLoginReq>,
) -> Response {
    let db = state.auth.db().clone();
    let id = req.credential.id.clone();
    let stored: Option<(i64, Vec<u8>, u32)> = db
        .call(move |c| {
            c.query_row(
                "SELECT user_id, public_key, sign_count FROM passkeys WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    let result = state
        .accounts
        .webauthn
        .finish_login(&req.credential, |_| stored);
    let (uid, count) = match result {
        Ok(v) => v,
        Err(why) => {
            tracing::info!(why, "passkey login rejected");
            return error(StatusCode::UNAUTHORIZED, "passkey");
        }
    };
    let id = req.credential.id.clone();
    let now = now_secs();
    let _ = db
        .call(move |c| {
            c.execute(
                "UPDATE passkeys SET sign_count = ?2, last_used_at = ?3 WHERE id = ?1",
                params![id, count, now],
            )?;
            // 패스키로 들어왔다면 메일 인증 전 계정은 있을 수 없지만(등록은 로그인 뒤에만) 혹시 몰라 확인 상태를 맞춘다.
            c.execute(
                "UPDATE users SET email_verified = 1 WHERE id = ?1",
                params![uid],
            )
        })
        .await;
    let token = state.auth.create_session(uid, &user_agent(&headers)).await;
    with_session(&state, &token, StatusCode::NO_CONTENT, None)
}

pub async fn passkey_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    // 남의 패스키는 지울 수 없다(사용자 조건).
    let n = state
        .auth
        .db()
        .call(move |c| {
            c.execute(
                "DELETE FROM passkeys WHERE id = ?1 AND user_id = ?2",
                params![id, uid],
            )
        })
        .await
        .unwrap_or(0);
    if n == 1 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error(StatusCode::NOT_FOUND, "missing")
    }
}

// ── Google 로그인(OIDC 인가 코드 흐름) ───────────────

#[derive(Deserialize)]
pub struct GoogleStart {
    #[serde(default)]
    locale: String,
}

pub async fn google_start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<GoogleStart>,
) -> Response {
    if !state.accounts.may_start_sign_in(client(&state, peer, &headers)) {
        return Redirect::to("/login?error=google").into_response();
    }
    let Some((url, cookie)) = google_begin(&state, None, &q.locale) else {
        return Redirect::to("/login?error=google").into_response();
    };
    ([(header::SET_COOKIE, cookie)], Redirect::to(&url)).into_response()
}

/// 로그인한 계정에 Google 계정을 연결한다. 비밀번호 없이 들어올 길이 하나 더 생기므로
/// 다른 민감한 작업처럼 다시 확인하고, 이미 연결돼 있으면 덮어쓰지 않는다(먼저 연결을 끊어야 한다).
pub async fn google_link(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ReauthReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if let Err(r) = reauth(&state, uid, &headers, client(&state, peer, &headers), &req.password).await {
        return r;
    }
    let linked: bool = state
        .auth
        .db()
        .call(move |c| {
            c.query_row(
                "SELECT google_sub IS NOT NULL FROM users WHERE id = ?1",
                params![uid],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(true);
    if linked {
        return error(StatusCode::CONFLICT, "linked");
    }
    let locale = req.locale.clone();
    let Some((url, cookie)) = google_begin(&state, Some(uid), &locale) else {
        return error(StatusCode::NOT_FOUND, "google");
    };
    ([(header::SET_COOKIE, cookie)], Json(json!({ "url": url }))).into_response()
}

/// Google 로그인을 시작한다: state·nonce를 기억하고 (이동할 주소, 이 브라우저에 묶을 쿠키)를 돌려준다.
fn google_begin(state: &AppState, link: Option<i64>, locale: &str) -> Option<(String, String)> {
    let google = state.accounts.google.as_ref()?;
    let st = new_token();
    let nonce = new_token();
    {
        let mut map = state.accounts.oauth.lock().unwrap();
        map.retain(|_, (_, _, _, at)| at.elapsed() < OAUTH_TTL);
        // 로그인하지 않은 요청도 하나씩 쌓으므로 상한을 두고, 넘으면 가장 오래된 것을 버린다.
        if map.len() >= 10_000
            && let Some(oldest) = map.iter().min_by_key(|(_, (_, _, _, at))| *at).map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
        map.insert(
            st.clone(),
            (nonce.clone(), link, clean_locale(locale).to_owned(), Instant::now()),
        );
    }
    let redirect = format!("{}/api/auth/google/callback", state.accounts.origin);
    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&scope=openid%20email%20profile&client_id={}&redirect_uri={}&state={st}&nonce={nonce}&prompt=select_account",
        urlencode(&google.client_id),
        urlencode(&redirect),
    );
    // state를 이 브라우저에도 묶어 둔다(다른 사람의 로그인 응답을 끼워 넣는 로그인 CSRF 방지).
    let secure = if state.cookie_secure { "; Secure" } else { "" };
    let cookie = format!(
        "{OAUTH_COOKIE}={st}; Path=/api/auth/google; HttpOnly; SameSite=Lax; Max-Age=600{secure}"
    );
    Some((url, cookie))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Deserialize)]
pub struct GoogleCallback {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct IdClaims {
    iss: String,
    aud: String,
    sub: String,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    nonce: Option<String>,
    exp: i64,
}

pub async fn google_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GoogleCallback>,
) -> Response {
    let fail = || Redirect::to("/login?error=google").into_response();
    let (Some(google), Some(code), Some(st)) = (&state.accounts.google, q.code, q.state) else {
        return fail();
    };
    let cookie_state = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|p| {
            p.trim()
                .strip_prefix(OAUTH_COOKIE)?
                .strip_prefix('=')
                .map(str::to_owned)
        });
    if cookie_state.as_deref() != Some(st.as_str()) {
        return fail();
    }
    let Some((nonce, link, locale, _)) = state
        .accounts
        .oauth
        .lock()
        .unwrap()
        .remove(&st)
        .filter(|(_, _, _, at)| at.elapsed() < OAUTH_TTL)
    else {
        return fail();
    };
    let redirect = format!("{}/api/auth/google/callback", state.accounts.origin);
    let res = google
        .http
        .post("https://oauth2.googleapis.com/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!(
            "code={}&client_id={}&client_secret={}&redirect_uri={}&grant_type=authorization_code",
            urlencode(&code),
            urlencode(&google.client_id),
            urlencode(&google.client_secret),
            urlencode(&redirect),
        ))
        .timeout(Duration::from_secs(15))
        .send()
        .await;
    let Ok(res) = res else { return fail() };
    let Ok(body) = res.json::<Value>().await else {
        return fail();
    };
    // 토큰 엔드포인트에서 TLS로 직접 받은 ID 토큰이므로 서명 대신 내용(발급자·대상·만료·nonce)을 확인한다(OIDC Core 3.1.3.7).
    let Some(claims) = body["id_token"].as_str().and_then(decode_jwt_payload) else {
        return fail();
    };
    if !matches!(
        claims.iss.as_str(),
        "https://accounts.google.com" | "accounts.google.com"
    ) || claims.aud != google.client_id
        || claims.exp <= now_secs()
        || claims.nonce.as_deref() != Some(nonce.as_str())
    {
        return fail();
    }
    let email: String = match &claims.email {
        Some(e) if claims.email_verified == Some(true) => normalize_username(e),
        _ => return fail(),
    };
    if !valid_email(&email) {
        return fail();
    }
    let sub = claims.sub.clone();
    let name = clean_name(claims.name.as_deref().unwrap_or(""));
    let db = state.auth.db().clone();
    let result = db
        .call(move |c| google_account(c, &sub, &email, &name, &locale, link))
        .await;
    let (uid, reset_sessions) = match result {
        Ok(Some(v)) => v,
        _ => return fail(),
    };
    if reset_sessions {
        state.auth.revoke_user_sessions(uid, None).await;
    }
    let clear =
        format!("{OAUTH_COOKIE}=; Path=/api/auth/google; HttpOnly; SameSite=Lax; Max-Age=0");
    if link.is_some() {
        return (
            [(header::SET_COOKIE, clear)],
            Redirect::to("/account/security"),
        )
            .into_response();
    }
    // 2차 인증을 켠 계정은 Google로 들어와도 코드를 한 번 더 묻는다(믿는 기기면 건너뜀).
    let totp: bool = state
        .auth
        .db()
        .call(move |c| {
            c.query_row(
                "SELECT totp_enabled FROM users WHERE id = ?1",
                params![uid],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(true);
    if totp && !trusted_device(state.auth.db(), &headers, uid).await {
        let ticket = state.auth.new_mfa_ticket(uid);
        return (
            [(header::SET_COOKIE, clear)],
            Redirect::to(&format!("/login?mfa={ticket}")),
        )
            .into_response();
    }
    let token = state.auth.create_session(uid, &user_agent(&headers)).await;
    (
        [
            (
                header::SET_COOKIE,
                auth::session_cookie(&token, state.cookie_secure),
            ),
            (header::SET_COOKIE, clear),
        ],
        Redirect::to("/"),
    )
        .into_response()
}

/// Google 계정을 우리 계정에 잇는다. 돌려주는 값: (사용자, 기존 세션을 끊어야 하는지).
/// - 이미 연결된 계정 → 그 계정
/// - 같은 이메일의 인증된 계정 → 연결(Google이 메일 소유를 확인)
/// - 같은 이메일의 인증 전 계정 → 연결하고 그 비밀번호는 지운다(남이 먼저 만들어 둔 계정을 넘겨받는 공격 방지)
/// - 연결 요청(로그인 상태에서) → 그 사용자에게 연결. 다른 계정에 이미 쓰인 Google 계정이면 실패
fn google_account(
    c: &rusqlite::Connection,
    sub: &str,
    email: &str,
    name: &str,
    locale: &str,
    link: Option<i64>,
) -> rusqlite::Result<Option<(i64, bool)>> {
    let by_sub: Option<i64> = c
        .query_row(
            "SELECT id FROM users WHERE google_sub = ?1",
            params![sub],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(uid) = link {
        return match by_sub {
            Some(other) if other != uid => Ok(None),
            Some(_) => Ok(Some((uid, false))),
            None => {
                // 이미 다른 Google 계정이 연결돼 있으면 덮어쓰지 않는다.
                let changed = c.execute(
                    "UPDATE users SET google_sub = ?2 WHERE id = ?1 AND google_sub IS NULL",
                    params![uid, sub],
                )?;
                Ok((changed == 1).then_some((uid, false)))
            }
        };
    }
    if let Some(uid) = by_sub {
        return Ok(Some((uid, false)));
    }
    match user_by_email(c, email)? {
        Some(u) if u.verified => {
            c.execute(
                "UPDATE users SET google_sub = ?2 WHERE id = ?1",
                params![u.id, sub],
            )?;
            Ok(Some((u.id, false)))
        }
        Some(u) => {
            c.execute(
                "UPDATE users SET google_sub = ?2, email_verified = 1, password_hash = NULL WHERE id = ?1",
                params![u.id, sub],
            )?;
            let _ = u.has_password;
            Ok(Some((u.id, true)))
        }
        None => {
            c.execute(
                "INSERT INTO users (email, name, google_sub, email_verified, locale, created_at) VALUES (?1, ?2, ?3, 1, ?4, ?5)",
                params![email, name, sub, locale, now_secs()],
            )?;
            Ok(Some((c.last_insert_rowid(), false)))
        }
    }
}

fn decode_jwt_payload(jwt: &str) -> Option<IdClaims> {
    use base64::Engine;
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 연결된 Google 계정을 끊는다. 다른 로그인 수단(비밀번호·패스키)이 있어야 한다.
pub async fn google_unlink(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let ok = state
        .auth
        .db()
        .call(move |c| {
            let (has_password, keys): (bool, i64) = c.query_row(
                "SELECT password_hash IS NOT NULL, (SELECT count(*) FROM passkeys WHERE user_id = ?1) FROM users WHERE id = ?1",
                params![uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if !has_password && keys == 0 {
                return Ok(false);
            }
            c.execute("UPDATE users SET google_sub = NULL WHERE id = ?1", params![uid])?;
            Ok(true)
        })
        .await
        .unwrap_or(false);
    if ok {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error(StatusCode::CONFLICT, "last_method")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with(email: &str, verified: bool) -> (Db, i64) {
        let db = Db::memory();
        let id = db
            .blocking(|c| {
                c.execute(
                    "INSERT INTO users (email, password_hash, email_verified, created_at) VALUES (?1, 'x', ?2, 1)",
                    params![email, verified as i64],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        (db, id)
    }

    #[test]
    fn google_takes_over_unverified_account_and_drops_its_password() {
        let (db, id) = db_with("a@b.co", false);
        let (uid, reset) = db
            .blocking(|c| google_account(c, "sub1", "a@b.co", "", "en", None))
            .unwrap()
            .unwrap();
        assert_eq!((uid, reset), (id, true));
        let hash: Option<String> = db
            .blocking(|c| {
                c.query_row(
                    "SELECT password_hash FROM users WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(hash, None);
    }

    #[test]
    fn google_links_verified_account_and_refuses_to_steal_another_link() {
        let (db, id) = db_with("a@b.co", true);
        assert_eq!(
            db.blocking(|c| google_account(c, "sub1", "a@b.co", "", "en", None))
                .unwrap(),
            Some((id, false))
        );
        let other = db
            .blocking(|c| {
                c.execute(
                    "INSERT INTO users (email, email_verified, created_at) VALUES ('z@b.co', 1, 1)",
                    [],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        assert_eq!(
            db.blocking(|c| google_account(c, "sub1", "x@y.co", "", "en", Some(other)))
                .unwrap(),
            None
        );
    }

    #[test]
    fn email_and_password_rules() {
        assert!(valid_email("a@b.co"));
        assert!(!valid_email("a@b"));
        assert!(!valid_email("@b.co"));
        assert!(!valid_password("short"));
        assert!(valid_password("long enough"));
    }
}
