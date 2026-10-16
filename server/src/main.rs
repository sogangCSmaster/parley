mod auth;
mod protocol;
mod room;
mod ws;

use std::io::BufRead;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRef, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing_subscriber::EnvFilter;

use auth::{Auth, LoginError};
use protocol::{ErrorCode, RoomSettings};
use room::Rooms;

#[derive(Clone)]
struct AppState {
    rooms: Arc<Rooms>,
    auth: Arc<Auth>,
    ice_servers: Arc<Value>,
    /// 로컬 http 개발에서만 false. 운영(https)에서는 반드시 true.
    cookie_secure: bool,
    /// 리버스 프록시(Caddy) 뒤에서만 true. 그때만 X-Forwarded-For로 클라이언트 IP를 판단한다.
    trusted_proxy: bool,
}

impl FromRef<AppState> for Arc<Rooms> {
    fn from_ref(state: &AppState) -> Self {
        state.rooms.clone()
    }
}

impl FromRef<AppState> for Arc<Auth> {
    fn from_ref(state: &AppState) -> Self {
        state.auth.clone()
    }
}

fn main() {
    // Load before starting worker threads; environment writes must stay single-threaded.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../.env");
    if let Err(error) = dotenvy::from_path(path)
        && !error.not_found()
    {
        // dotenv parse errors can contain the source line, including credentials.
        eprintln!("Could not load root .env; check its syntax and permissions.");
        std::process::exit(1);
    }
    run();
}

#[tokio::main]
async fn run() {
    // `parley-server hash-password` : 표준입력의 비밀번호를 HOST_ACCOUNTS용 해시로 바꿔 출력한다.
    if std::env::args().nth(1).as_deref() == Some("hash-password") {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .expect("failed to read stdin");
        let password = line.trim_end_matches(['\r', '\n']);
        assert!(!password.is_empty(), "password must not be empty");
        println!("{}", auth::hash_password(password).expect("hashing failed"));
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()
        .expect("BIND_ADDR must be host:port");

    // TURN을 붙이면 ICE_SERVERS에 JSON 배열로 넣는다. 기본값은 개발용 공개 STUN.
    let ice_servers: Value = match std::env::var("ICE_SERVERS") {
        Ok(raw) => serde_json::from_str(&raw).expect("ICE_SERVERS must be a JSON array"),
        Err(_) => json!([{ "urls": "stun:stun.l.google.com:19302" }]),
    };
    // /api/config는 공개라서 여기 든 값은 누구나 읽는다. 정적 TURN 자격을 넣으면 릴레이를 남이 쓰게 된다.
    // TURN은 서버가 발급하는 시간제한 HMAC 자격으로 붙일 예정이다(docs/ARCHITECTURE.md의 "TURN 자격").
    if let Some(field) = static_ice_credential(&ice_servers) {
        panic!(
            "ICE_SERVERS must not contain static `{field}` fields: /api/config is public. \
             Use time-limited TURN credentials instead (see docs/ARCHITECTURE.md)"
        );
    }

    let accounts = auth::parse_accounts(&std::env::var("HOST_ACCOUNTS").unwrap_or_default())
        .unwrap_or_else(|e| panic!("HOST_ACCOUNTS: {e}"));
    let auth = Arc::new(Auth::new(accounts));
    if !auth.has_accounts() {
        tracing::warn!("HOST_ACCOUNTS is empty: nobody can log in or create meetings");
    }
    let cookie_secure = std::env::var("COOKIE_SECURE")
        .map(|v| v != "false")
        .unwrap_or(true);
    let trusted_proxy = std::env::var("TRUSTED_PROXY").is_ok_and(|v| v == "1");

    let state = AppState {
        rooms: Arc::new(Rooms::default()),
        auth,
        ice_servers: Arc::new(ice_servers),
        cookie_secure,
        trusted_proxy,
    };

    let sweeper = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            sweeper.rooms.sweep(Instant::now());
            sweeper.auth.sweep();
        }
    });

    let app = Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/config", get(config))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/rooms", post(create_room))
        .route("/api/rooms/{code}", get(room_info))
        .route("/ws", get(ws::handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");
    tracing::info!(%addr, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .expect("server error");
}

fn static_ice_credential(ice_servers: &Value) -> Option<&'static str> {
    let servers = ice_servers.as_array()?;
    servers.iter().find_map(|server| {
        ["credential", "username"]
            .into_iter()
            .find(|field| server.get(field).is_some())
    })
}

async fn config(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "iceServers": *state.ice_servers }))
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

// Json 추출기는 Content-Type: application/json만 받는다. 이 덕분에 다른 사이트의 단순 폼 전송(CSRF)은
// 여기까지 오지 못하고, SameSite=Lax 쿠키와 함께 이중으로 막힌다.
async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> Response {
    let client = auth::client_ip(peer.ip(), &headers, state.trusted_proxy);
    match state.auth.login(client, &req.username, &req.password).await {
        Ok(token) => {
            tracing::info!(user = %req.username, "login");
            (
                StatusCode::NO_CONTENT,
                [(
                    header::SET_COOKIE,
                    auth::session_cookie(&token, state.cookie_secure),
                )],
            )
                .into_response()
        }
        Err(LoginError::Invalid) => StatusCode::UNAUTHORIZED.into_response(),
        Err(LoginError::Locked) => StatusCode::TOO_MANY_REQUESTS.into_response(),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::token_from_headers(&headers) {
        state.auth.logout(&token);
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, auth::clear_cookie(state.cookie_secure))],
    )
        .into_response()
}

/// 비로그인도 정상 응답(200, username: null)이다. 게스트가 매번 401 콘솔 오류를 보지 않도록.
async fn me(State(state): State<AppState>, headers: HeaderMap) -> Json<Value> {
    Json(json!({ "username": state.auth.user_from_headers(&headers) }))
}

/// 로그인한 주최자만 회의를 만들 수 있다.
async fn create_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(settings): Json<RoomSettings>,
) -> Response {
    let Some(username) = state.auth.user_from_headers(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match state.rooms.create(&username, settings) {
        Ok(code) => Json(json!({ "code": code })).into_response(),
        Err(ErrorCode::Forbidden) => StatusCode::TOO_MANY_REQUESTS.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// 로비에서 입장 전에 번역 사용 여부를 알기 위한 공개 정보. 설정 외에는 아무것도 노출하지 않는다.
async fn room_info(State(state): State<AppState>, Path(code): Path<String>) -> Response {
    if !room::is_valid_room_code(&code) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match state.rooms.settings(&code) {
        Some(settings) => Json(settings).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
