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
    /// 가입·메일 다시 보내기·재설정 요청: IP마다, 주소마다.
    by_ip: KeyedLimiter,
    by_email: KeyedLimiter,
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
            by_ip: KeyedLimiter::new(20, Duration::from_secs(3600)),
            by_email: KeyedLimiter::new(5, Duration::from_secs(3600)),
            oauth: Mutex::default(),
        }
    }

    /// 메일을 보내는 요청의 빈도 제한. 넘으면 조용히 보내지 않는다(응답은 같게 해 계정 존재를 숨긴다).
    fn may_mail(&self, ip: IpAddr, email: &str) -> bool {
        self.by_ip.check(&ip.to_string()) && self.by_email.check(email)
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

async fn new_email_code(
    db: &Db,
    user_id: i64,
    kind: &'static str,
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
        c.execute("DELETE FROM email_codes WHERE user_id = ?1 AND kind = ?2", params![user_id, kind])?;
        c.execute(
            "INSERT INTO email_codes (user_id, kind, code_hash, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user_id, kind, hash, now + ttl, now],
        )?;
        Ok(())
    })
    .await?;
    Ok(code)
}

fn send_verify_code(state: &AppState, email: String, locale: &str, code: &str) {
    state.accounts.mailer.send(
        Copy {
            lang: Lang::parse(locale),
        }
        .verify_code(&email, code),
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
    let ip = client(&state, peer, &headers);
    if !state.accounts.may_mail(ip, &email) {
        return error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let locale = clean_locale(&req.locale).to_owned();
    let name = clean_name(&req.name);
    let password = req.password.clone();
    let Ok(hash) = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .unwrap_or(Err(String::new()))
    else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    let db = state.auth.db().clone();
    let e = email.clone();
    let l = locale.clone();
    let result = db
        .call(move |c| {
            match user_by_email(c, &e)? {
                Some(u) if u.verified => Ok((u.id, true, u.locale)),
                Some(u) => {
                    // 인증 전 계정은 새 가입 정보로 덮는다(아직 아무도 로그인할 수 없는 계정).
                    c.execute(
                        "UPDATE users SET password_hash = ?2, name = ?3, locale = ?4 WHERE id = ?1",
                        params![u.id, hash, name, l],
                    )?;
                    Ok((u.id, false, l))
                }
                None => {
                    c.execute(
                        "INSERT INTO users (email, name, password_hash, locale, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![e, name, hash, l, now_secs()],
                    )?;
                    Ok((c.last_insert_rowid(), false, l))
                }
            }
        })
        .await;
    let Ok((user_id, existing, user_locale)) = result else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    if existing {
        if let Ok(token) = new_email_code(&db, user_id, "reset").await {
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
    } else if let Ok(code) = new_email_code(&db, user_id, "verify").await {
        send_verify_code(&state, email, &user_locale, &code);
    }
    StatusCode::ACCEPTED.into_response()
}

#[derive(Deserialize)]
pub struct VerifyReq {
    email: String,
    code: String,
}

/// 이메일 인증 코드 확인. 맞으면 인증 완료로 바꾸고 바로 로그인시킨다.
pub async fn verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<VerifyReq>,
) -> Response {
    let email = normalize_username(&req.email);
    let code = req.code.trim().to_owned();
    let db = state.auth.db().clone();
    let now = now_secs();
    let result = db
        .call(move |c| {
            let Some(u) = user_by_email(c, &email)? else { return Ok(None) };
            let row: Option<(i64, String, i64, i64)> = c
                .query_row(
                    "SELECT id, code_hash, expires_at, attempts FROM email_codes WHERE user_id = ?1 AND kind = 'verify'",
                    params![u.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((id, hash, expires, attempts)) = row else { return Ok(None) };
            if expires <= now || attempts >= MAX_CODE_ATTEMPTS {
                return Ok(None);
            }
            if hash != code_hash(u.id, &code) {
                c.execute("UPDATE email_codes SET attempts = attempts + 1 WHERE id = ?1", params![id])?;
                return Ok(None);
            }
            c.execute("DELETE FROM email_codes WHERE user_id = ?1 AND kind = 'verify'", params![u.id])?;
            c.execute("UPDATE users SET email_verified = 1 WHERE id = ?1", params![u.id])?;
            Ok(Some(u.id))
        })
        .await;
    match result {
        Ok(Some(user_id)) => {
            let token = state
                .auth
                .create_session(user_id, &user_agent(&headers))
                .await;
            with_session(&state, &token, StatusCode::NO_CONTENT, None)
        }
        Ok(None) => error(StatusCode::BAD_REQUEST, "code"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

#[derive(Deserialize)]
pub struct EmailReq {
    email: String,
}

/// 인증 코드 다시 보내기. 계정이 없거나 이미 인증됐어도 같은 응답.
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
    if let Ok(Some(u)) = db.call(move |c| user_by_email(c, &e)).await
        && !u.verified
        && let Ok(code) = new_email_code(&db, u.id, "verify").await
    {
        send_verify_code(&state, email, &u.locale, &code);
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
        Err(LoginError::Unverified) => {
            // 비밀번호가 맞은 인증 전 계정: 새 코드를 보내고 인증 화면으로 보낸다.
            let email = normalize_username(&req.email);
            let db = state.auth.db().clone();
            let e = email.clone();
            if state.accounts.may_mail(ip, &email)
                && let Ok(Some(u)) = db.call(move |c| user_by_email(c, &e)).await
                && let Ok(code) = new_email_code(&db, u.id, "verify").await
            {
                send_verify_code(&state, email, &u.locale, &code);
            }
            error(StatusCode::FORBIDDEN, "unverified")
        }
        Err(LoginError::MfaRequired(ticket)) => Json(json!({ "mfa": ticket })).into_response(),
    }
}

#[derive(Deserialize)]
pub struct MfaReq {
    ticket: String,
    code: String,
}

/// 비밀번호 다음의 2차 인증 코드. 티켓 하나에 5번까지(넘으면 처음부터 다시 로그인).
pub async fn login_mfa(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MfaReq>,
) -> Response {
    let Some(user_id) = state.auth.mfa_ticket_user(&req.ticket) else {
        return error(StatusCode::UNAUTHORIZED, "expired");
    };
    if !state.auth.count_mfa_attempt(&req.ticket) {
        return error(StatusCode::TOO_MANY_REQUESTS, "locked");
    }
    let code = req.code.clone();
    let ok = state
        .auth
        .db()
        .call(move |c| check_totp(c, user_id, &code))
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
    with_session(&state, &token, StatusCode::NO_CONTENT, None)
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
        && let Ok(token) = new_email_code(&db, u.id, "reset").await
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
pub async fn reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ResetReq>,
) -> Response {
    if !valid_password(&req.password) {
        return error(StatusCode::BAD_REQUEST, "password");
    }
    let password = req.password.clone();
    let Ok(hash) = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .unwrap_or(Err(String::new()))
    else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    let want = code_hash(req.uid, &req.token);
    let uid = req.uid;
    let now = now_secs();
    let ok = state
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
                return Ok(false);
            }
            c.execute("DELETE FROM email_codes WHERE user_id = ?1 AND kind = 'reset'", params![uid])?;
            c.execute(
                "UPDATE users SET password_hash = ?2, email_verified = 1 WHERE id = ?1",
                params![uid, hash],
            )?;
            Ok(true)
        })
        .await
        .unwrap_or(false);
    if !ok {
        return error(StatusCode::BAD_REQUEST, "token");
    }
    state.auth.revoke_user_sessions(uid, None).await;
    let token = state.auth.create_session(uid, &user_agent(&headers)).await;
    with_session(&state, &token, StatusCode::NO_CONTENT, None)
}

// ── 내 정보·계정 설정 ─────────────────────────────────

/// 비로그인도 정상 응답(200, user: null)이다. 게스트가 매번 401 콘솔 오류를 보지 않도록.
pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let user = match state.auth.user_id_from_headers(&headers) {
        Some(id) => user_json(state.auth.db(), id).await,
        None => Value::Null,
    };
    Json(json!({
        "user": user,
        "features": {
            "google": state.accounts.google.is_some(),
            "billing": state.billing.is_some(),
        },
    }))
}

pub async fn user_json(db: &Db, id: i64) -> Value {
    db.call(move |c| {
        let user = c
            .query_row(
                "SELECT id, email, name, locale, email_verified, password_hash IS NOT NULL, google_sub IS NOT NULL,
                        totp_enabled, plan, plan_renews_at, plan_canceled, operator, created_at
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
                    }))
                },
            )
            .optional()?;
        let Some(mut user) = user else { return Ok(Value::Null) };
        let mut stmt = c.prepare("SELECT id, name, created_at, last_used_at FROM passkeys WHERE user_id = ?1 ORDER BY created_at")?;
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
        let sessions: i64 = c.query_row("SELECT count(*) FROM sessions WHERE user_id = ?1", params![id], |r| r.get(0))?;
        user["sessions"] = json!(sessions);
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

#[derive(Deserialize)]
pub struct ProfileReq {
    name: Option<String>,
    locale: Option<String>,
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
    let locale = req.locale.as_deref().map(|l| clean_locale(l).to_owned());
    let db = state.auth.db().clone();
    let res = db
        .call(move |c| {
            if let Some(n) = name {
                c.execute("UPDATE users SET name = ?2 WHERE id = ?1", params![uid, n])?;
            }
            if let Some(l) = locale {
                c.execute(
                    "UPDATE users SET locale = ?2 WHERE id = ?1",
                    params![uid, l],
                )?;
            }
            Ok(())
        })
        .await;
    match res {
        Ok(()) => Json(user_json(&db, uid).await).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
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
        match state
            .auth
            .check_password(client(&state, peer, &headers), uid, &req.current)
            .await
        {
            Ok(()) => {}
            Err(LoginError::Locked) => return error(StatusCode::TOO_MANY_REQUESTS, "locked"),
            Err(_) => return error(StatusCode::UNAUTHORIZED, "current"),
        }
    }
    let new = req.new.clone();
    let Ok(hash) = tokio::task::spawn_blocking(move || hash_password(&new))
        .await
        .unwrap_or(Err(String::new()))
    else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    };
    if db
        .call(move |c| {
            c.execute(
                "UPDATE users SET password_hash = ?2 WHERE id = ?1",
                params![uid, hash],
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
    StatusCode::NO_CONTENT.into_response()
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
    if has_password {
        match state
            .auth
            .check_password(client(&state, peer, &headers), uid, &req.password)
            .await
        {
            Ok(()) => {}
            Err(LoginError::Locked) => return error(StatusCode::TOO_MANY_REQUESTS, "locked"),
            Err(_) => return error(StatusCode::UNAUTHORIZED, "current"),
        }
    } else if normalize_username(&req.confirm) != email {
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
    let user = user_json(&db, uid).await;
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
pub async fn totp_setup(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
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

/// 앱에 넣은 뒤 나온 첫 코드로 확인해야 켜진다.
pub async fn totp_enable(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CodeReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let code = req.code.clone();
    let ok = state
        .auth
        .db()
        .call(move |c| {
            let secret: Option<String> = c.query_row(
                "SELECT totp_secret FROM users WHERE id = ?1",
                params![uid],
                |r| r.get(0),
            )?;
            let Some(secret) = secret else {
                return Ok(false);
            };
            match totp::verify(&secret, &code, now_secs(), 0) {
                Some(step) => {
                    c.execute(
                        "UPDATE users SET totp_enabled = 1, totp_last_step = ?2 WHERE id = ?1",
                        params![uid, step],
                    )?;
                    Ok(true)
                }
                None => Ok(false),
            }
        })
        .await
        .unwrap_or(false);
    if ok {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error(StatusCode::BAD_REQUEST, "code")
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
    let code = req.code.clone();
    let ok = state
        .auth
        .db()
        .call(move |c| {
            if !check_totp(c, uid, &code)? {
                return Ok(false);
            }
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
    headers: HeaderMap,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
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

pub async fn passkey_login_options(State(state): State<AppState>) -> Response {
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
    headers: HeaderMap,
    Query(q): Query<GoogleStart>,
) -> Response {
    let Some(google) = &state.accounts.google else {
        return Redirect::to("/login?error=google").into_response();
    };
    let link = state.auth.user_id_from_headers(&headers);
    let st = new_token();
    let nonce = new_token();
    {
        let mut map = state.accounts.oauth.lock().unwrap();
        map.retain(|_, (_, _, _, at)| at.elapsed() < OAUTH_TTL);
        map.insert(
            st.clone(),
            (
                nonce.clone(),
                link,
                clean_locale(&q.locale).to_owned(),
                Instant::now(),
            ),
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
    ([(header::SET_COOKIE, cookie)], Redirect::to(&url)).into_response()
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
    if link.is_some() {
        return Redirect::to("/account/security").into_response();
    }
    let token = state.auth.create_session(uid, &user_agent(&headers)).await;
    let clear =
        format!("{OAUTH_COOKIE}=; Path=/api/auth/google; HttpOnly; SameSite=Lax; Max-Age=0");
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
            _ => {
                c.execute(
                    "UPDATE users SET google_sub = ?2 WHERE id = ?1",
                    params![uid, sub],
                )?;
                Ok(Some((uid, false)))
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
