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
use crate::limit::TokenBucket;
use crate::pipeline::{Pipeline, SpeakerHandle};
use crate::protocol::{ClientMsg, ErrorCode, PeerId, ServerMsg};
use crate::providers::live::SttTokens;
use crate::room::{self, JoinOutcome, JoinRequest, Outbox, Rooms};
use crate::spend::Budget;

/// SDP는 수 KB 수준이다. 이보다 큰 메시지는 거부한다.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// 상대가 허용 한도만큼 ICE 후보를 한꺼번에 보내도(CANDIDATES_BURST) 받는 쪽 큐가 넘치지 않을 만큼.
/// 넘치면 그 연결을 끊는다(느린 클라이언트). 큐는 찰 때만 메모리를 쓴다. 후보는 수백 바이트지만 SDP는
/// 메시지 한도(64KB)까지 될 수 있다. 시그널은 문자열 필드만 받으므로 메모리는 글자 수와 비슷하고,
/// 최악(여러 상대가 읽지 않는 한 명에게 큰 SDP를 계속 보냄)이라도 512 × 64KB ≈ 32MB에서 그 연결이 끊긴다.
const OUTBOX_CAPACITY: usize = 512;
const PING_INTERVAL: Duration = Duration::from_secs(20);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// STT 중간 결과는 약 0.5초마다 온다. 이보다 훨씬 잦으면 버린다(번역 비용 폭주 방지).
const STT_PER_SEC: f64 = 5.0;
const STT_BURST: f64 = 10.0;
/// 연결 하나가 보내는 모든 프레임(잘못된 것·ping 포함). 아래 한도를 다 써도 닿지 않는 값이라,
/// 넘으면 오작동이나 남용으로 보고 연결을 끊는다.
const MESSAGES_PER_SEC: f64 = 100.0;
const MESSAGES_BURST: f64 = 500.0;
/// ICE 후보는 상대에게 그대로 중계된다. 최악의 정상 경우: 4번째로 들어와 3명과 동시에 연결하면서
/// 인터페이스가 많은 노트북(호스트·srflx·TURN 후보)이면 상대당 30~40개, 합쳐 약 120개가 1~2초에 나온다.
const CANDIDATES_PER_SEC: f64 = 50.0;
/// 이보다 큰 시그널은 후보가 아니라 SDP 한도로 센다.
const MAX_CANDIDATE_BYTES: usize = 2048;
const CANDIDATES_BURST: f64 = 300.0;
/// SDP(offer/answer)는 하나만 빠져도 그 연결이 복구되지 않는다. 정상이면 상대당 재협상마다 한두 개라
/// 아주 넉넉하게 두고, 큰 SDP로 상대 큐를 채우는 것만 막는다.
const DESCRIPTIONS_PER_SEC: f64 = 20.0;
const DESCRIPTIONS_BURST: f64 = 50.0;
/// 설정·수락/거절(주최자 동작)은 방 전원에게 퍼진다. 사람이 누르는 속도면 충분하다.
const CONTROLS_PER_SEC: f64 = 5.0;
const CONTROLS_BURST: f64 = 20.0;
/// 마이크 상태·언어 알림. 버리지 않고 미루며, 주최자 동작의 한도를 써 버리지 않게 따로 센다.
const STATES_PER_SEC: f64 = 5.0;
const STATES_BURST: f64 = 20.0;
/// 한도에 걸려 미룬 마이크 상태·언어 알림을 다시 보내 보는 간격
const ANNOUNCE_RETRY: Duration = Duration::from_millis(250);

pub async fn handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(rooms): State<Arc<Rooms>>,
    State(auth): State<Arc<Auth>>,
    State(pipeline): State<Arc<Pipeline>>,
    State(stt): State<Option<Arc<dyn SttTokens>>>,
    State(spend): State<Arc<Budget>>,
) -> Response {
    // 다른 사이트의 페이지가 사용자의 쿠키로 연결하는 것(cross-site WebSocket hijacking)을 막는다.
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token =
        auth::token_from_headers(&headers).filter(|token| auth.session_user(token).is_some());
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, rooms, pipeline, stt, spend, auth, token))
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
    spend: Arc<Budget>,
    auth: Arc<Auth>,
    token: Option<String>,
) {
    let (mut sink, mut stream) = socket.split();
    let (sender, mut rx) = mpsc::channel::<ServerMsg>(OUTBOX_CAPACITY);
    let disconnect = Arc::new(Notify::new());
    let tx = Outbox::new(sender, disconnect.clone());

    let mut writer = tokio::spawn(async move {
        // 첫 핑은 한 주기 뒤에. 연결 직후 핑을 보내면 읽지 않은 퐁이 남은 채 끊을 수 있고,
        // 그러면 OS가 FIN 대신 RST를 보내 아직 안 보낸 데이터까지 버린다.
        let mut ping = tokio::time::interval_at(
            tokio::time::Instant::now() + PING_INTERVAL,
            PING_INTERVAL,
        );
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
    let mut message_limit = TokenBucket::new(MESSAGES_PER_SEC, MESSAGES_BURST);
    let mut candidate_limit = Limit::new(CANDIDATES_PER_SEC, CANDIDATES_BURST);
    let mut description_limit = Limit::new(DESCRIPTIONS_PER_SEC, DESCRIPTIONS_BURST);
    let mut control_limit = Limit::new(CONTROLS_PER_SEC, CONTROLS_BURST);
    let mut state_limit = TokenBucket::new(STATES_PER_SEC, STATES_BURST);
    // 마이크 상태·언어는 버리면 다른 참가자가 틀린 상태를 계속 보게 된다. 한도에 걸리면 상태는 바로 바꾸고
    // 알림만 미뤘다가, 여유가 생기면 최신 상태를 한 번 보낸다.
    // 시점을 고정해 둔다(다른 메시지가 계속 와도 미뤄지지 않게).
    let mut announce_at: Option<tokio::time::Instant> = None;
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
                // 방이 정리됐다(대기만 하다 오래 비어 있던 방). 알림은 정리할 때 보냈으니 연결을 닫는다.
                if session.as_ref().is_some_and(|(room, _)| !rooms.exists(room)) {
                    break;
                }
                continue;
            }
            _ = tokio::time::sleep_until(deadline) => break,
            _ = tokio::time::sleep_until(announce_at.unwrap_or(deadline)), if announce_at.is_some() => {
                match &session {
                    Some((room, id)) if state_limit.take(Instant::now()) => {
                        rooms.announce(room, id);
                        announce_at = None;
                    }
                    Some(_) => announce_at = Some(tokio::time::Instant::now() + ANNOUNCE_RETRY),
                    None => announce_at = None,
                }
                continue;
            }
            msg = stream.next() => match msg {
                Some(Ok(msg)) => msg,
                _ => break,
            },
        };
        last_seen = tokio::time::Instant::now();
        if !message_limit.take(Instant::now()) {
            let (room, peer) = session.as_ref().map_or(("", ""), |(r, p)| (r.as_str(), p.as_str()));
            tracing::warn!(room, peer, "closing connection: too many messages");
            break;
        }
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

        let now = Instant::now();
        let verdict = match &msg {
            // 진짜 ICE 후보 메시지는 500바이트도 안 된다. 크면 SDP 한도로 센다.
            ClientMsg::Signal { data, .. }
                if data.candidate.is_some() && text.len() <= MAX_CANDIDATE_BYTES =>
            {
                candidate_limit.check(now)
            }
            ClientMsg::Signal { .. } => description_limit.check(now),
            // 상태 변경은 버리지 않는다(아래에서 알림만 미룬다).
            ClientMsg::Lang { .. } | ClientMsg::Media { .. } => Verdict::Allow,
            ClientMsg::Settings { .. } | ClientMsg::Admit { .. } | ClientMsg::Deny { .. } => {
                control_limit.check(now)
            }
            _ => Verdict::Allow,
        };
        match verdict {
            Verdict::Allow => {}
            Verdict::Drop { notify } => {
                if notify {
                    reply(&tx, ErrorCode::RateLimited);
                }
                continue;
            }
        }

        let result = match (msg, &session) {
            (
                ClientMsg::Join {
                    room,
                    name,
                    lang,
                    partner_lang,
                    media,
                    resume,
                },
                None,
            ) => match (room::normalize_name(&name), room::is_valid_room_code(&room)) {
                (Some(name), true)
                    if room::is_valid_lang(&lang) && room::is_valid_lang(&partner_lang) =>
                {
                    // 예약해 둔 회의는 누군가 처음 들어올 때 방을 연다.
                    crate::meetings::ensure_room(auth.db(), &rooms, &room).await;
                    let req = JoinRequest {
                        name,
                        lang,
                        partner_lang,
                        media,
                        resume: resume.filter(|t| room::is_valid_resume(t)),
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
                    let f = |p: &mut crate::protocol::Peer| {
                        p.lang = lang;
                        p.partner_lang = partner_lang;
                    };
                    if announce_at.is_none() && state_limit.take(now) {
                        rooms.update(room, id, f);
                    } else {
                        rooms.update_quietly(room, id, f);
                        announce_at.get_or_insert(tokio::time::Instant::now() + ANNOUNCE_RETRY);
                    }
                    Ok(())
                } else {
                    Err(ErrorCode::BadRequest)
                }
            }
            (ClientMsg::Media { media }, Some((room, id))) => {
                if announce_at.is_none() && state_limit.take(now) {
                    rooms.update(room, id, |p| p.media = media);
                } else {
                    rooms.update_quietly(room, id, |p| p.media = media);
                    announce_at.get_or_insert(tokio::time::Instant::now() + ANNOUNCE_RETRY);
                }
                Ok(())
            }
            (ClientMsg::Signal { to, data }, Some((room, id))) => {
                if data.is_valid() {
                    rooms.relay_signal(room, id, &to, data)
                } else {
                    Err(ErrorCode::BadRequest)
                }
            }
            (ClientMsg::Admit { id: guest }, Some((room, id))) => rooms.admit(room, id, &guest),
            (ClientMsg::Deny { id: guest }, Some((room, id))) => rooms.deny(room, id, &guest),
            (ClientMsg::Settings { settings }, Some((room, id))) => {
                rooms.set_settings(room, id, settings)
            }
            // 입장 여부·번역 사용 여부는 파이프라인이 자막을 보낼 때마다 확인한다.
            // 너무 긴 텍스트는 파이프라인이 `MAX_STT_CHARS`까지 자른다(거부하면 그 구간이 확정되지 않는다).
            (ClientMsg::Stt { seg, text, is_final }, Some((room, id))) => {
                if stt_limit.take(Instant::now()) {
                    speaker
                        .get_or_insert_with(|| pipeline.speaker(room.clone(), id.clone()))
                        .push(seg, text, is_final);
                }
                Ok(())
            }
            (ClientMsg::SttToken { id: req }, Some((room, id))) => {
                while stt_tasks.try_join_next().is_some() {}
                match (&stt, rooms.stt_langs(room, id), rooms.stt_blocked(room, Instant::now())) {
                    // 주최자의 번역 시간이 떨어졌거나 주최자가 오래 없으면 다시 시도해도 소용없다. 까닭을 화면에 알린다
                    // (풀리면 QuotaRestored·HostBack 알림으로 다시 시작한다).
                    (Some(_), Some(_), Some(code)) => {
                        tx.send(ServerMsg::Notice { code });
                        tx.send(ServerMsg::SttTokenDenied { id: req, retry: false });
                    }
                    // 발급 빈도는 참가자에 묶여 있어 다시 연결해도 새로 차지 않는다(재입장 토큰으로 연결만 바꿔 우회하지 못하게).
                    (Some(issuer), Some(langs), None) if rooms.take_stt_token(room, id, Instant::now()) => {
                        // 발급은 Gemini 왕복(수백 ms)이라 연결 루프를 막지 않도록 따로 돌린다.
                        let issue = issuer.issue(&langs);
                        let spend = spend.clone();
                        let tx = tx.clone();
                        let (rooms, room, id) = (rooms.clone(), room.clone(), id.clone());
                        stt_tasks.spawn(async move {
                            let msg = match issue.await {
                                Ok(grant) => {
                                    spend.stt_issued();
                                    // 발급하는 사이 막혔으면(지출 상한 등) 받은 토큰을 건네지 않는다.
                                    if let Some(code) = rooms.stt_blocked(&room, Instant::now()) {
                                        tx.send(ServerMsg::Notice { code });
                                        ServerMsg::SttTokenDenied { id: req, retry: false }
                                    } else {
                                        ServerMsg::SttToken { id: req, token: grant.token, model: grant.model }
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!(%err, "stt token failed");
                                    // 받지 못한 토큰은 한도에서 빼지 않는다(다시 요청하게 하는 실패라서).
                                    rooms.give_back_stt_token(&room, &id, Instant::now());
                                    ServerMsg::SttTokenDenied { id: req, retry: true }
                                }
                            };
                            tx.send(msg);
                        });
                    }
                    (Some(_), Some(_), None) => tx.send(ServerMsg::SttTokenDenied { id: req, retry: true }),
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

enum Verdict {
    Allow,
    /// 버린다. `notify`는 한도에 걸리기 시작한 첫 메시지에만 true(오류 답장이 출력 큐를 넘치게 하지 않도록).
    Drop { notify: bool },
}

/// 버리는 한도. 걸려 있는 동안 오류는 한 번만 알린다.
struct Limit {
    bucket: TokenBucket,
    limited: bool,
}

impl Limit {
    fn new(rate: f64, burst: f64) -> Self {
        Limit {
            bucket: TokenBucket::new(rate, burst),
            limited: false,
        }
    }

    fn allow(&mut self, now: Instant) -> bool {
        let ok = self.bucket.take(now);
        if ok {
            self.limited = false;
        }
        ok
    }

    fn check(&mut self, now: Instant) -> Verdict {
        if self.allow(now) {
            return Verdict::Allow;
        }
        let notify = !self.limited;
        self.limited = true;
        Verdict::Drop { notify }
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
                accounts: Arc::new(crate::account::Accounts::new(
                    crate::mail::Mailer::off(),
                    "http://localhost".into(),
                    None,
                )),
                billing: None,
                spend: Arc::new(crate::spend::Budget::new(None, Default::default())),
                dev_mode: false,
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
                // 테스트 계정 "host"는 메모리 DB의 첫 사용자(id 1)다.
                .create("1", crate::protocol::RoomSettings { translation: true })
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
                resume: None,
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

    /// 소켓이 닫히거나 잠시 조용해질 때까지 받은 메시지
    async fn drain(socket: &mut TestSocket) -> (Vec<ServerMsg>, bool) {
        drain_for(socket, Duration::from_millis(300)).await
    }

    async fn drain_for(socket: &mut TestSocket, quiet: Duration) -> (Vec<ServerMsg>, bool) {
        let mut msgs = Vec::new();
        loop {
            match tokio::time::timeout(quiet, socket.next()).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                    msgs.push(serde_json::from_str(&text).unwrap())
                }
                Ok(Some(Ok(_))) => {}
                Ok(_) => return (msgs, true),
                Err(_) => return (msgs, false),
            }
        }
    }

    /// 같은 방에 두 참가자를 넣고 (a, b, b의 id)를 돌려준다.
    async fn pair(server: &TestServer, room: &str) -> (TestSocket, TestSocket, PeerId) {
        let mut a = welcomed(server, room).await;
        let b = welcomed(server, room).await;
        let Some(ServerMsg::PeerJoined { peer }) = next_message(&mut a).await else {
            panic!("expected peer_joined");
        };
        (a, b, peer.id)
    }

    fn rate_limited(msgs: &[ServerMsg]) -> usize {
        msgs.iter()
            .filter(|m| matches!(m, ServerMsg::Error { code: ErrorCode::RateLimited, .. }))
            .count()
    }

    #[tokio::test]
    async fn media_flood_is_coalesced_and_others_end_with_the_latest_state() {
        let server = TestServer::start().await;
        let room = server.room();
        let (mut a, mut b, _) = pair(&server, &room).await;
        for i in 0..40 {
            let media = crate::protocol::MediaState {
                mic: i % 2 == 1,
                cam: false,
                screen: false,
            };
            send(&mut a, ClientMsg::Media { media }).await;
        }
        // 미룬 알림(ANNOUNCE_RETRY)보다 충분히 오래 기다린다.
        let (msgs, closed) = drain_for(&mut b, Duration::from_secs(1)).await;
        let updates: Vec<_> = msgs
            .iter()
            .filter_map(|m| match m {
                ServerMsg::PeerUpdated { peer } => Some(peer.media.mic),
                _ => None,
            })
            .collect();
        assert!(!closed);
        assert!(updates.len() < 40, "broadcasts are limited: {}", updates.len());
        assert_eq!(updates.last(), Some(&true), "the final toggle (mic on) reaches others");
        let (mine, closed) = drain(&mut a).await;
        assert!(!closed);
        assert_eq!(rate_limited(&mine), 0, "state changes are never refused");
    }

    #[tokio::test]
    async fn candidates_are_relayed_up_to_a_worst_case_join_and_refused_once_beyond() {
        let server = TestServer::start().await;
        let room = server.room();
        let (mut a, mut b, b_id) = pair(&server, &room).await;
        // SDP는 세지 않는다.
        for _ in 0..5 {
            let data = crate::protocol::SignalData {
                description: Some(crate::protocol::SessionDescription {
                    kind: crate::protocol::SdpType::Offer,
                    sdp: Some("v=0".into()),
                }),
                candidate: None,
            };
            send(&mut a, ClientMsg::Signal { to: b_id.clone(), data }).await;
        }
        for i in 0..400 {
            let data = crate::protocol::SignalData::candidate(&format!("candidate:{i}"));
            send(&mut a, ClientMsg::Signal { to: b_id.clone(), data }).await;
        }
        let (relayed, b_closed) = drain(&mut b).await;
        assert!(!b_closed, "the receiver keeps up with a full burst");
        let relayed = relayed
            .iter()
            .filter(|m| matches!(m, ServerMsg::Signal { .. }))
            .count();
        // SDP 5개 + 후보 한도 300개는 반드시 지나가고, 그사이 다시 찬 토큰만큼 조금 더 지나갈 수 있다.
        assert!((305..405).contains(&relayed), "{relayed} relayed");
        let (mine, closed) = drain(&mut a).await;
        assert!(!closed);
        // 걸릴 때마다 한 번만 알린다(토큰이 다시 차서 잠깐 풀렸다 걸리면 다시 한 번).
        let refusals = rate_limited(&mine);
        let dropped = 405 - relayed;
        assert!(refusals >= 1 && refusals <= dropped, "{refusals} refusals for {dropped} dropped");
    }

    #[tokio::test]
    async fn large_candidates_count_against_the_sdp_limit() {
        let server = TestServer::start().await;
        let room = server.room();
        let (mut a, mut b, b_id) = pair(&server, &room).await;
        let padding = "x".repeat(4096);
        for _ in 0..80 {
            let data = crate::protocol::SignalData::candidate(&padding);
            send(&mut a, ClientMsg::Signal { to: b_id.clone(), data }).await;
        }
        let (relayed, _) = drain(&mut b).await;
        let relayed = relayed
            .iter()
            .filter(|m| matches!(m, ServerMsg::Signal { .. }))
            .count();
        assert!((50..80).contains(&relayed), "{relayed} large signals relayed (SDP limit 50)");
    }

    #[tokio::test]
    async fn malformed_signals_are_rejected_not_relayed() {
        let server = TestServer::start().await;
        let room = server.room();
        let (mut a, mut b, b_id) = pair(&server, &room).await;
        let both = serde_json::json!({ "t": "signal", "to": b_id,
            "data": { "description": { "type": "offer", "sdp": "v=0" }, "candidate": { "candidate": "c" } } });
        let neither = serde_json::json!({ "t": "signal", "to": b_id, "data": {} });
        let arbitrary = serde_json::json!({ "t": "signal", "to": b_id, "data": { "candidate": [0, 0, 0] } });
        for msg in [both, neither, arbitrary] {
            a.send(tokio_tungstenite::tungstenite::Message::Text(msg.to_string().into()))
                .await
                .unwrap();
        }
        let (mine, _) = drain(&mut a).await;
        let bad = mine
            .iter()
            .filter(|m| matches!(m, ServerMsg::Error { code: ErrorCode::BadRequest, .. }))
            .count();
        assert_eq!(bad, 3);
        let (theirs, _) = drain(&mut b).await;
        assert!(!theirs.iter().any(|m| matches!(m, ServerMsg::Signal { .. })));
    }

    #[tokio::test]
    async fn message_flood_closes_the_connection() {
        let server = TestServer::start().await;
        let room = server.room();
        let mut socket = welcomed(&server, &room).await;
        // 답장이 없는 프레임만 보낸다: 출력 큐가 넘쳐서가 아니라 한도 때문에 끊겨야 한다.
        for _ in 0..3000 {
            if socket
                .send(tokio_tungstenite::tungstenite::Message::Binary(vec![0u8].into()))
                .await
                .is_err()
            {
                break;
            }
        }
        let (_, closed) = drain(&mut socket).await;
        assert!(closed);
    }

    #[tokio::test]
    async fn overlong_stt_text_is_truncated_not_rejected() {
        let server = TestServer::start().await;
        let room = server.room();
        let mut socket = welcomed(&server, &room).await;
        let limit = crate::pipeline::MAX_STT_CHARS;
        send(
            &mut socket,
            ClientMsg::Stt { seg: 0, text: "가".repeat(limit + 500), is_final: true },
        )
        .await;
        let (msgs, closed) = drain(&mut socket).await;
        assert!(!closed);
        assert!(!msgs.iter().any(|m| matches!(m, ServerMsg::Error { .. })), "{msgs:?}");
        // 이 서버에는 번역기가 없어 원문만 확정된다. 확정은 와야 한다(자막이 "번역 중"에 머물지 않게).
        let finals: Vec<usize> = msgs
            .iter()
            .filter_map(|m| match m {
                ServerMsg::Caption { src, is_final: true, .. } => Some(src.chars().count()),
                _ => None,
            })
            .collect();
        assert_eq!(finals, [limit]);
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
                assert_eq!(token, "auth_tokens/fake-en+ko", "issued for my language, then my partner's");
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
    async fn spend_limit_pauses_stt_tokens_and_resumes() {
        let server = TestServer::start().await;
        let room = server.room();
        let mut host = welcomed(&server, &room).await;
        server.rooms.set_spend_paused(true);
        assert!(matches!(
            next_message(&mut host).await,
            Some(ServerMsg::Notice { code: crate::protocol::NoticeCode::SpendLimit })
        ));
        send(&mut host, ClientMsg::SttToken { id: 1 }).await;
        assert!(matches!(
            next_message(&mut host).await,
            Some(ServerMsg::Notice { code: crate::protocol::NoticeCode::SpendLimit })
        ));
        assert!(matches!(
            next_message(&mut host).await,
            Some(ServerMsg::SttTokenDenied { retry: false, .. })
        ));
        server.rooms.set_spend_paused(false);
        assert!(matches!(
            next_message(&mut host).await,
            Some(ServerMsg::Notice { code: crate::protocol::NoticeCode::SpendRestored })
        ));
        send(&mut host, ClientMsg::SttToken { id: 2 }).await;
        assert!(matches!(next_message(&mut host).await, Some(ServerMsg::SttToken { id: 2, .. })));
    }

    #[tokio::test]
    async fn stt_tokens_are_rate_limited_and_failures_are_retryable() {
        let server = TestServer::start().await;
        let room = server.room();
        let mut socket = welcomed(&server, &room).await;
        for _ in 0..room::STT_TOKEN_BURST as usize {
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
    async fn failing_token_issuance_cannot_be_retried_without_limit() {
        // 발급이 실패하면 한도를 돌려주지만 횟수가 묶여, 실패를 일으켜 가며 Gemini를 계속 부르게 하지 못한다.
        let failing = Arc::new(crate::providers::live::testing::FakeTokens { fail: true, ..Default::default() });
        let server = TestServer::start_with(Some(failing.clone())).await;
        let room = server.room();
        let mut socket = welcomed(&server, &room).await;
        for _ in 0..20 {
            send(&mut socket, ClientMsg::SttToken { id: 7 }).await;
            assert!(matches!(next_message(&mut socket).await, Some(ServerMsg::SttTokenDenied { retry: true, .. })));
        }
        let calls = failing.langs.lock().unwrap().len();
        assert_eq!(calls, room::STT_TOKEN_BURST as usize + 2, "{calls} upstream calls");
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
