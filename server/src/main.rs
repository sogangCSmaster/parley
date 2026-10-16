mod auth;
mod dev;
mod ice;
mod limit;
mod pipeline;
mod protocol;
mod providers;
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
use ice::Ice;
use pipeline::Pipeline;
use protocol::{ErrorCode, RoomSettings};
use providers::Translator;
use providers::gemini::{self, Gemini};
use providers::live::{self, GeminiLive, SttTokens};
use room::Rooms;

#[derive(Clone)]
struct AppState {
    rooms: Arc<Rooms>,
    auth: Arc<Auth>,
    pipeline: Arc<Pipeline>,
    /// 브라우저 음성 인식 토큰 발급. Gemini 키가 없으면 None.
    stt: Option<Arc<dyn SttTokens>>,
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

impl FromRef<AppState> for Arc<Pipeline> {
    fn from_ref(state: &AppState) -> Self {
        state.pipeline.clone()
    }
}

impl FromRef<AppState> for Option<Arc<dyn SttTokens>> {
    fn from_ref(state: &AppState) -> Self {
        state.stt.clone()
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

    // `parley-server dev` : 개발용(테스트 계정 자동 생성, http 쿠키). loopback 바인딩에서만 허용.
    let dev_mode = std::env::args().nth(1).as_deref() == Some("dev");

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()
        .expect("BIND_ADDR must be host:port");
    assert!(
        !dev_mode || dev::allowed_bind(&addr),
        "dev mode only binds to loopback (BIND_ADDR={addr}); never run it on a public address"
    );

    // ICE 서버는 Welcome으로 입장한 참가자에게만 준다. TURN 자격은 참가자마다 시간제한으로 만든다.
    let ice = Ice::new(
        std::env::var("ICE_SERVERS").ok().as_deref(),
        std::env::var("TURN_URLS").ok().as_deref(),
        std::env::var("TURN_SECRET").ok().as_deref(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    if !ice.has_turn() && !dev_mode {
        tracing::warn!("TURN_URLS is not set: participants behind strict NATs may fail to connect");
    }

    let mut accounts = auth::parse_accounts(&std::env::var("HOST_ACCOUNTS").unwrap_or_default())
        .unwrap_or_else(|e| panic!("HOST_ACCOUNTS: {e}"));
    if dev_mode && accounts.is_empty() {
        let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.dev-host-password"));
        let creds = dev::ensure_credentials(path)
            .unwrap_or_else(|e| panic!("dev account file {}: {e}", path.display()));
        let hash = auth::hash_password(&creds.password).expect("hashing failed");
        // 비밀번호는 로그에 남기지 않고 파일 위치만 알린다.
        tracing::warn!(user = %creds.username, file = %path.display(), "dev mode: test host account ready");
        accounts.insert(creds.username, hash);
    }
    let auth = Arc::new(Auth::new(accounts));
    if !auth.has_accounts() {
        tracing::warn!("HOST_ACCOUNTS is empty: nobody can log in or create meetings");
    }
    // dev 모드는 http://localhost 이므로 기본 false. 그 외에는 명시하지 않으면 항상 Secure.
    let cookie_secure = std::env::var("COOKIE_SECURE")
        .map(|v| v != "false")
        .unwrap_or(!dev_mode);
    let trusted_proxy = std::env::var("TRUSTED_PROXY").is_ok_and(|v| v == "1");

    // 번역 키는 서버에만 둔다. 없으면 자막은 원문만 확정된다.
    // 음성 인식 토큰도 같은 키로 발급하고, 번역기와 HTTP 연결을 공유한다.
    let mut translator: Option<Arc<dyn Translator>> = None;
    let mut stt: Option<Arc<dyn SttTokens>> = None;
    match std::env::var("GEMINI_API_KEY") {
        Ok(key) if !key.trim().is_empty() => {
            let key = key.trim().to_owned();
            let model = std::env::var("GEMINI_TRANSLATE_MODEL")
                .unwrap_or_else(|_| gemini::DEFAULT_MODEL.into());
            let gemini = Arc::new(Gemini::new(key.clone(), model));
            let stt_model =
                std::env::var("GEMINI_STT_MODEL").unwrap_or_else(|_| live::DEFAULT_MODEL.into());
            stt = Some(Arc::new(GeminiLive::new(gemini.client(), key, stt_model)));
            let warm = gemini.clone();
            tokio::spawn(async move { warm.warm_up().await });
            translator = Some(gemini);
        }
        _ => tracing::warn!(
            "GEMINI_API_KEY is not set: no speech recognition, captions show the original only"
        ),
    }
    let rooms = Arc::new(Rooms::new(ice));

    let state = AppState {
        pipeline: Arc::new(Pipeline::new(rooms.clone(), translator)),
        stt,
        rooms,
        auth,
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
