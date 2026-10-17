mod account;
mod auth;
mod billing;
mod db;
mod mail;
mod meetings;
mod totp;
mod webauthn;
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

use axum::extract::FromRef;
use axum::routing::{get, post};
use axum::Router;
use tracing_subscriber::EnvFilter;

use auth::Auth;
use ice::Ice;
use pipeline::Pipeline;
use providers::Translator;
use providers::gemini::{self, Gemini};
use providers::live::{self, GeminiLive, SttTokens};
use room::Rooms;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) rooms: Arc<Rooms>,
    pub(crate) auth: Arc<Auth>,
    pub(crate) pipeline: Arc<Pipeline>,
    /// 브라우저 음성 인식 토큰 발급. Gemini 키가 없으면 None.
    pub(crate) stt: Option<Arc<dyn SttTokens>>,
    /// 가입·메일·패스키·Google 로그인
    pub(crate) accounts: Arc<account::Accounts>,
    /// Paddle 결제. 키가 없으면 None.
    pub(crate) billing: Option<Arc<billing::Billing>>,
    /// `tolkmeet-server dev`(로컬 개발)
    pub(crate) dev_mode: bool,
    /// 로컬 http 개발에서만 false. 운영(https)에서는 반드시 true.
    pub(crate) cookie_secure: bool,
    /// 리버스 프록시(Caddy) 뒤에서만 true. 그때만 X-Forwarded-For로 클라이언트 IP를 판단한다.
    pub(crate) trusted_proxy: bool,
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
    // `tolkmeet-server hash-password` : 표준입력의 비밀번호를 HOST_ACCOUNTS용 해시로 바꿔 출력한다.
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

    // `tolkmeet-server dev` : 개발용(테스트 계정 자동 생성, http 쿠키). loopback 바인딩에서만 허용.
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
    // 개발 모드는 따로 된 파일을 써서 개발용 계정이 운영 DB에 섞이지 않게 한다.
    let db_path = std::env::var("DATABASE_PATH").unwrap_or_else(|_| {
        let name = if dev_mode { "dev.db" } else { "tolkmeet.db" };
        format!("{}/data/{name}", env!("CARGO_MANIFEST_DIR"))
    });
    let custom_path = std::env::var_os("DATABASE_PATH").is_some();
    if let Some(dir) = std::path::Path::new(&db_path).parent() {
        let existed = dir.exists();
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("database dir {}: {e}", dir.display()));
        // 폴더 권한은 앱 전용 폴더(기본 위치이거나 서버가 새로 만든 폴더)만 좁힌다.
        // DATABASE_PATH=/srv/x.db처럼 원래 있던 공용 폴더를 잠그지 않게.
        if !custom_path || !existed {
            restrict(dir, 0o700);
        }
    }
    let db = db::Db::open(std::path::Path::new(&db_path))
        .unwrap_or_else(|e| panic!("database {db_path}: {e}"));
    // 비밀번호 해시·2차 인증 키가 들어 있으므로 서버 계정만 읽게 한다.
    for suffix in ["", "-wal", "-shm"] {
        let p = format!("{db_path}{suffix}");
        if std::path::Path::new(&p).exists() {
            restrict(std::path::Path::new(&p), 0o600);
        }
    }
    // 운영자 계정(HOST_ACCOUNTS·개발용 계정)은 시작할 때마다 DB를 목록에 맞춘다(빠진 계정은 권한을 거둔다).
    auth::sync_operators(&db, &accounts).unwrap_or_else(|e| panic!("operator accounts: {e}"));
    if let Err(e) = meetings::close_orphans(&db) {
        tracing::warn!(error = %e, "could not close meetings left open by a restart");
    }
    let auth = Arc::new(Auth::with_db(db));
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

    // 앱 주소: 메일 속 링크, Google 로그인 되돌아올 주소, 패스키 출처 확인에 쓴다.
    let origin = std::env::var("APP_ORIGIN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().trim_end_matches('/').to_owned())
        .unwrap_or_else(|| if dev_mode { "http://localhost:5173".into() } else { "https://meet.tolkmeet.com".into() });
    let mail_folder = dev_mode.then(|| std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/data/mail")));
    let mailer = mail::Mailer::from_env(mail_folder);
    let google = account::Google::from_env();
    if google.is_none() {
        tracing::info!("GOOGLE_CLIENT_ID/SECRET not set: Google sign-in is hidden");
    }
    let billing = billing::Billing::from_env().map(Arc::new);
    if billing.is_none() {
        tracing::warn!("PADDLE_* not set: checkout is disabled");
    }

    let state = AppState {
        pipeline: Arc::new(Pipeline::new(rooms.clone(), translator)),
        stt,
        rooms,
        auth,
        accounts: Arc::new(account::Accounts::new(mailer, origin, google)),
        billing,
        dev_mode,
        cookie_secure,
        trusted_proxy,
    };

    let sweeper = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let now = Instant::now();
            // 번역 시간을 먼저 재고(지울 방 포함), 그다음 빈 방을 지워 끝난 회의로 남긴다.
            let metered = sweeper.rooms.meter(now);
            meetings::tick(&sweeper, &metered).await;
            let ended = sweeper.rooms.sweep(now);
            meetings::spawn_end(&sweeper, ended);
            sweeper.auth.sweep();
        }
    });

    let app = Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/login", post(account::login))
        .route("/api/login/mfa", post(account::login_mfa))
        .route("/api/logout", post(account::logout))
        .route("/api/me", get(account::me))
        .route("/api/signup", post(account::signup))
        .route("/api/verify", post(account::verify))
        .route("/api/verify/resend", post(account::resend))
        .route("/api/password/forgot", post(account::forgot))
        .route("/api/password/reset", post(account::reset))
        .route("/api/account", axum::routing::patch(account::update_profile).delete(account::delete_account))
        .route("/api/account/password", post(account::change_password))
        .route("/api/account/sessions/revoke", post(account::revoke_other_sessions))
        .route("/api/account/sessions/{id}", axum::routing::delete(account::revoke_session))
        .route("/api/account/email", post(account::email_change))
        .route("/api/account/email/verify", post(account::email_change_verify))
        .route("/api/totp/recovery", post(account::totp_recovery))
        .route("/api/account/export", get(account::export))
        .route("/api/totp/setup", post(account::totp_setup))
        .route("/api/totp/enable", post(account::totp_enable))
        .route("/api/totp/disable", post(account::totp_disable))
        .route("/api/passkeys/register/options", post(account::passkey_register_options))
        .route("/api/passkeys/register", post(account::passkey_register))
        .route("/api/passkeys/login/options", post(account::passkey_login_options))
        .route("/api/passkeys/login", post(account::passkey_login))
        .route("/api/passkeys/{id}", axum::routing::delete(account::passkey_delete))
        .route("/api/auth/google", get(account::google_start))
        .route("/api/auth/google/callback", get(account::google_callback))
        .route("/api/auth/google/unlink", post(account::google_unlink))
        .route("/api/auth/google/link", post(account::google_link))
        .route("/api/rooms", post(meetings::create_room))
        .route("/api/rooms/{code}", get(meetings::room_info))
        .route("/api/meetings", get(meetings::list).post(meetings::schedule))
        .route("/api/meetings/{id}", axum::routing::patch(meetings::update).delete(meetings::remove))
        .route("/api/meetings/{id}/invite", post(meetings::invite))
        .route("/api/meetings/{id}/ics", get(meetings::ics))
        .route("/api/billing", get(meetings::billing_summary))
        .route("/api/billing/checkout", post(meetings::checkout))
        .route("/api/billing/dev", post(meetings::dev_activate))
        .route("/api/billing/portal", post(meetings::portal))
        .route("/api/billing/{action}", post(meetings::manage))
        .route("/api/paddle/webhook", post(meetings::paddle_webhook))
        .route("/ws", get(ws::handler))
        // API 응답에는 계정·회의 정보가 들어 있으므로 브라우저·프록시가 저장하지 않게 한다.
        .layer(axum::middleware::map_response(no_store))
        .layer(axum::middleware::from_fn_with_state(
            state.accounts.origin.clone(),
            same_site_writes,
        ))
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

/// 쿠키로 로그인한 상태를 바꾸는 API 요청은 우리 화면에서 온 것만 받는다(SameSite에 더한 CSRF 방어).
/// 브라우저가 보낸 Origin이 앱 주소와 다르거나 Sec-Fetch-Site가 다른 사이트면 거부한다.
/// Paddle 웹훅은 서명으로 확인하므로 제외한다.
async fn same_site_writes(
    axum::extract::State(origin): axum::extract::State<String>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{Method, StatusCode};
    use axum::response::IntoResponse;
    let writes = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let path = req.uri().path();
    if writes && path.starts_with("/api/") && path != "/api/paddle/webhook" {
        let headers = req.headers();
        let foreign_origin = headers
            .get(axum::http::header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|o| o != origin);
        let cross_site = headers
            .get("sec-fetch-site")
            .is_some_and(|v| v.as_bytes() == b"cross-site");
        if foreign_origin || cross_site {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    next.run(req).await
}

async fn no_store(mut res: axum::response::Response) -> axum::response::Response {
    res.headers_mut()
        .entry(axum::http::header::CACHE_CONTROL)
        .or_insert(axum::http::HeaderValue::from_static("no-store"));
    res
}

/// 파일·폴더 권한을 좁힌다(유닉스). 실패하면 기록만 하고 계속한다.
fn restrict(path: &std::path::Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            tracing::warn!(error = %e, path = %path.display(), "could not restrict permissions");
        }
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}
