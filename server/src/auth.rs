//! 로그인·세션. 계정은 SQLite(`db.rs`)에 두고, 세션은 DB에 해시로 저장하면서 메모리에도 사본을 둔다
//! (재시작해도 로그인이 유지되고, 회의 입장 경로는 DB를 읽지 않는다).

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use axum::http::HeaderMap;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::db::{Db, OptionalExtension, now_secs, params};

pub const COOKIE_NAME: &str = "tolkmeet_session";
/// 사용자 한 명이 동시에 가질 수 있는 로그인 세션 수
const MAX_SESSIONS: i64 = 50;
/// 로그인 유지 기간. 쿠키와 DB 세션이 같은 기간을 쓴다.
const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// (클라이언트 IP, 아이디)별로 이 시간 안에 이만큼 실패하면 시간이 지날 때까지 로그인을 막는다.
/// IP별로 세므로 아이디를 아는 공격자가 다른 곳의 주최자를 잠글 수 없다.
const MAX_FAILURES: u32 = 10;
/// 여러 IP에 나눠 시도하는 공격을 막는 아이디별 상한. 정상 사용자가 닿지 않도록 넉넉하게 둔다.
/// 이 상한에 걸려도 최근 같은 클라이언트에서 그 계정으로 로그인에 성공한 적이 있으면 막지 않는다
/// (아이디만 아는 사람이 여러 곳에서 틀려 주인의 로그인을 막지 못하게).
const MAX_USER_FAILURES: u32 = 100;
/// 로그인에 성공한 (클라이언트, 아이디)를 기억하는 기간
const KNOWN_CLIENT_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// 계정마다 기억하는 최근 로그인 위치 수. 자기 계정으로 아무리 많은 곳에서 들어와도
/// 다른 계정의 기억을 밀어내지 못하게 계정마다 따로 센다.
const KNOWN_CLIENTS_PER_USER: usize = 8;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// 한 클라이언트(/64)·한 망(/48)이 여러 아이디에 걸쳐 틀릴 수 있는 횟수(같은 창 안).
const MAX_IP_FAILURES: u32 = 60;
/// 망(/48)은 통신사·회사에서 모르는 사람 수천 명이 나눠 쓸 수 있어 넉넉하게 둔다(/64를 바꿔 가며 뿌리기만 막는다).
const MAX_NET_FAILURES: u32 = 2000;
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
    /// 비밀번호 확인이 몰려 잠시 처리할 수 없다(다시 시도하면 된다)
    Busy,
    /// 비밀번호는 맞지만 이메일 인증 전
    Unverified,
    /// 비밀번호는 맞고 2차 인증 코드가 필요하다(티켓으로 이어서 확인)
    MfaRequired(String),
}

struct Session {
    user_id: i64,
    /// 유닉스 초
    expires: i64,
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
    /// 성공하면 실패 기록을 지운다(계정·(클라이언트, 계정) 단위). 클라이언트 단위는 지우지 않는다:
    /// 자기 계정으로 한 번 맞게 들어와 기록을 지우고 다른 계정을 계속 맞혀 보지 못하게.
    forgive_on_success: bool,
}

impl<K: Eq + Hash + Clone> Limiter<K> {
    fn new(max: u32, cap: usize) -> Self {
        Self {
            max,
            cap,
            map: HashMap::new(),
            forgive_on_success: true,
        }
    }

    fn strict(max: u32, cap: usize) -> Self {
        Self {
            forgive_on_success: false,
            ..Self::new(max, cap)
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
        if failed {
            a.count += 1;
        } else if self.forgive_on_success {
            a.count = 0;
        }
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
    /// 아이디를 바꿔 가며 틀리는 것(비밀번호 뿌리기·검증 슬롯 독차지): 클라이언트(/64)와 망(/48) 단위.
    /// 검증 중인 시도도 세므로 동시에 몰아 보내도 한도를 넘지 못한다.
    by_ip: Limiter<IpAddr>,
    by_net: Limiter<IpAddr>,
    /// 아이디 → 최근 로그인에 성공한 클라이언트(인증된 계정만). 아이디별 상한을 넘어도 이 곳의 주인은 들어올 수 있다.
    known: HashMap<String, Vec<(IpAddr, Instant)>>,
    cap: usize,
}

impl Throttle {
    fn new(cap: usize) -> Self {
        Self {
            by_client: Limiter::new(MAX_FAILURES, cap),
            by_user: Limiter::new(MAX_USER_FAILURES, cap),
            by_ip: Limiter::strict(MAX_IP_FAILURES, cap),
            by_net: Limiter::strict(MAX_NET_FAILURES, cap),
            known: HashMap::new(),
            cap,
        }
    }

    fn is_known(&self, client: IpAddr, username: &str, now: Instant) -> bool {
        self.known.get(username).is_some_and(|list| {
            list.iter().any(|(ip, at)| *ip == client && now.duration_since(*at) < KNOWN_CLIENT_TTL)
        })
    }

    /// 인증된 계정에 로그인한 곳을 기억한다.
    fn remember(&mut self, client: IpAddr, username: &str, now: Instant) {
        if !self.known.contains_key(username) && self.known.len() >= self.cap {
            self.known
                .retain(|_, list| list.iter().any(|(_, at)| now.duration_since(*at) < KNOWN_CLIENT_TTL));
            // 그래도 가득하면 가장 오래 들어오지 않은 계정의 기억을 버린다. 계정마다 한 칸이라
            // 남의 기억을 밀어내려면 인증된 계정이 그만큼 있어야 한다.
            if self.known.len() >= self.cap
                && let Some(oldest) = self
                    .known
                    .iter()
                    .min_by_key(|(_, list)| list.iter().map(|(_, at)| *at).max())
                    .map(|(k, _)| k.clone())
            {
                self.known.remove(&oldest);
            }
        }
        let list = self.known.entry(username.to_owned()).or_default();
        list.retain(|(ip, at)| *ip != client && now.duration_since(*at) < KNOWN_CLIENT_TTL);
        list.push((client, now));
        if list.len() > KNOWN_CLIENTS_PER_USER {
            list.remove(0);
        }
    }

    fn try_reserve(&mut self, client: IpAddr, username: &str, now: Instant) -> Result<(), LoginError> {
        let client_key = (client, username.to_owned());
        let net = net_key(client);
        let user_full = self.by_user.is_full(&username.to_owned(), now) && !self.is_known(client, username, now);
        if self.by_client.is_full(&client_key, now) || user_full || self.by_ip.is_full(&client, now) {
            return Err(LoginError::Locked);
        }
        // 망 단위는 이웃(같은 통신사·회사의 다른 /64)이 채웠을 수 있다. 이 사람이 틀린 게 아니므로
        // "너무 많이 틀렸다"가 아니라 "잠시 뒤 다시"로 답한다.
        if self.by_net.is_full(&net, now) {
            return Err(LoginError::Busy);
        }
        self.by_client.reserve(client_key, now);
        self.by_user.reserve(username.to_owned(), now);
        self.by_ip.reserve(client, now);
        self.by_net.reserve(net, now);
        Ok(())
    }

    fn finish(&mut self, client: IpAddr, username: &str, failed: bool) {
        self.by_client
            .finish(&(client, username.to_owned()), failed);
        self.by_user.finish(&username.to_owned(), failed);
        self.by_ip.finish(&client, failed);
        self.by_net.finish(&net_key(client), failed);
    }

    fn sweep(&mut self, now: Instant) {
        self.by_client.sweep(now);
        self.by_user.sweep(now);
        self.by_ip.sweep(now);
        self.by_net.sweep(now);
        self.known.retain(|_, list| {
            list.retain(|(_, at)| now.duration_since(*at) < KNOWN_CLIENT_TTL);
            !list.is_empty()
        });
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
    db: Db,
    /// 유효한 세션의 메모리 사본(토큰 해시 → 세션). 회의 입장·WebSocket 확인은 DB를 읽지 않는다.
    sessions: Mutex<HashMap<String, Session>>,
    /// 비밀번호까지 맞고 2차 인증을 기다리는 로그인.
    /// 티켓 → (사용자, 만든 때, 코드 시도 수, 비밀번호로 로그인한 곳). 2차 인증까지 끝나야 그곳을 기억한다.
    mfa_tickets: Mutex<HashMap<String, MfaTicket>>,
    throttle: Mutex<Throttle>,
    verify_slots: Arc<Semaphore>,
    /// 없는 계정도 같은 시간이 걸리도록 비교에 쓰는 해시(계정 존재 여부를 응답 시간으로 알 수 없게).
    dummy_hash: String,
}

type MfaTicket = (i64, Instant, u32, Option<(IpAddr, String)>);

/// 2차 인증 티켓은 비밀번호를 맞힌 직후 5분 안에만 쓸 수 있다.
const MFA_TICKET_TTL: Duration = Duration::from_secs(5 * 60);

impl Auth {
    /// 저장된 세션을 메모리로 올린다(만료된 것은 지운다).
    pub fn with_db(db: Db) -> Self {
        let dummy_hash = hash_password("dummy-password-for-timing").expect("argon2 hashing works");
        let now = now_secs();
        let sessions = db
            .blocking(|c| {
                c.execute("DELETE FROM sessions WHERE expires_at <= ?1", params![now])?;
                let mut stmt = c.prepare("SELECT token_hash, user_id, expires_at FROM sessions")?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        Session {
                            user_id: r.get(1)?,
                            expires: r.get(2)?,
                        },
                    ))
                })?;
                rows.collect::<rusqlite::Result<HashMap<_, _>>>()
            })
            .expect("load sessions");
        Self {
            db,
            sessions: Mutex::new(sessions),
            mfa_tickets: Mutex::default(),
            throttle: Mutex::new(Throttle::new(MAX_TRACKED)),
            verify_slots: Arc::new(Semaphore::new(VERIFY_CONCURRENCY)),
            dummy_hash,
        }
    }

    /// 테스트·운영자 계정용: 이메일 → 해시 목록을 메모리 DB에 넣는다(인증 완료, 요금제 있음).
    pub fn new(accounts: HashMap<String, String>) -> Self {
        let db = Db::memory();
        sync_operators(&db, &accounts).expect("insert accounts");
        Self::with_db(db)
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// 성공하면 새 세션 토큰을 돌려준다. 비밀번호 검증은 블로킹 스레드에서 돌려 런타임을 막지 않는다.
    /// 없는 계정도 같은 방식으로 실패를 세므로 429 응답으로 계정 존재 여부를 알 수 없다.
    pub async fn login(
        &self,
        client: IpAddr,
        username: &str,
        password: &str,
    ) -> Result<String, LoginError> {
        self.login_with_agent(client, username, password, "").await
    }

    /// `login`과 같고, 새 세션에 브라우저 정보(로그인된 기기 목록에 보일 이름)를 남긴다.
    pub async fn login_with_agent(
        &self,
        client: IpAddr,
        username: &str,
        password: &str,
        user_agent: &str,
    ) -> Result<String, LoginError> {
        let username = &normalize_username(username);
        // 형식에 맞지 않는 아이디는 어떤 계정과도 일치할 수 없다(형식은 공개 정보).
        // 해싱도 기록도 하지 않아 큰 키가 기록 맵에 쌓이지 않는다.
        if !is_valid_username(username) {
            return Err(LoginError::Invalid);
        }
        let client = client_key(client);
        // 검증 슬롯이 잠깐 비지 않으면 시도 기록을 소비하지 않고 "잠시 뒤 다시"를 알린다.
        let permit = self.acquire_slot().await?;
        self.throttle
            .lock()
            .unwrap()
            .try_reserve(client, username, Instant::now())?;
        let mut reservation = Reservation {
            throttle: &self.throttle,
            client,
            username,
            succeeded: false,
        };
        let Some(user) = self.verify(username, password, permit).await else {
            return Err(LoginError::Invalid);
        };
        reservation.succeeded = true;
        drop(reservation);

        // 비밀번호가 맞은 뒤에만 인증·2차 인증 상태를 알린다(계정 존재 여부는 이미 드러나도 되는 시점).
        if !user.verified {
            return Err(LoginError::Unverified);
        }
        // 로그인을 다 마친 곳만 주인이 들어오는 곳으로 기억한다. 인증 전 계정은 아무나 만들 수 있으므로 세지 않고,
        // 2차 인증 계정은 코드까지 맞힌 뒤에 기억한다(비밀번호만 아는 사람이 주인의 기억을 밀어내지 못하게).
        if user.totp {
            let origin = Some((client, username.to_owned()));
            return Err(LoginError::MfaRequired(self.new_mfa_ticket_from(user.id, origin)));
        }
        self.throttle.lock().unwrap().remember(client, username, Instant::now());
        // 세션을 저장하지 못했으면(일시적인 DB 오류) 잠시 뒤 다시 시도하게 한다.
        self.create_session(user.id, user_agent).await.ok_or(LoginError::Busy)
    }

    /// 비밀번호 확인 슬롯을 잡는다. 몰렸을 때 바로 거절하지 않고 잠깐 기다린다.
    async fn acquire_slot(&self) -> Result<OwnedSemaphorePermit, LoginError> {
        tokio::time::timeout(Duration::from_secs(2), self.verify_slots.clone().acquire_owned())
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or(LoginError::Busy)
    }

    /// 계정 조회와 비밀번호 비교를 한 블로킹 작업에서 한다. 요청이 취소돼도 작업이 슬롯을 끝까지 쥔다.
    async fn verify(
        &self,
        email: &str,
        password: &str,
        permit: OwnedSemaphorePermit,
    ) -> Option<LoginAccount> {
        if password.len() > MAX_PASSWORD_BYTES {
            return None;
        }
        let db = self.db.clone();
        let dummy = self.dummy_hash.clone();
        let email = email.to_owned();
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let account = db
                .blocking(|c| {
                    let account = c
                        .query_row(
                            "SELECT id, password_hash, email_verified, totp_enabled FROM users WHERE email = ?1",
                            params![email],
                            |r| {
                                Ok(LoginAccount {
                                    id: r.get(0)?,
                                    hash: r.get(1)?,
                                    verified: r.get::<_, i64>(2)? != 0,
                                    totp: r.get::<_, i64>(3)? != 0,
                                })
                            },
                        )
                        .optional()?;
                    // 인증 전 가입은 비밀번호가 아직 계정이 아니라 마지막 인증 코드에 있다. 맞으면 "인증 필요"를 알리려고 그것과 비교한다.
                    let Some(mut a) = account else { return Ok(None) };
                    if !a.verified && a.hash.is_none() {
                        let payload: Option<Option<String>> = c
                            .query_row(
                                "SELECT payload FROM email_codes WHERE user_id = ?1 AND kind = 'verify' ORDER BY id DESC LIMIT 1",
                                params![a.id],
                                |r| r.get(0),
                            )
                            .optional()?;
                        a.hash = payload.flatten().and_then(|p| SignupPayload::decode(&p)).map(|p| p.hash);
                    }
                    Ok(Some(a))
                })
                .ok()
                .flatten();
            // 비밀번호가 없는 계정(Google로만 가입)도 없는 계정과 똑같이 다룬다.
            let hash = account.as_ref().and_then(|a| a.hash.clone()).unwrap_or(dummy);
            let ok = verify_password(&password, &hash);
            account.filter(|a| ok && a.hash.is_some())
        })
        .await
        .ok()
        .flatten()
    }

    /// 비밀번호 확인(계정 설정의 민감한 작업 전). 시도 제한은 로그인과 같은 기록을 쓴다.
    pub async fn check_password(&self, client: IpAddr, user_id: i64, password: &str) -> Result<(), LoginError> {
        let email = self
            .db
            .call(move |c| c.query_row("SELECT email FROM users WHERE id = ?1", params![user_id], |r| r.get::<_, String>(0)))
            .await
            .map_err(|_| LoginError::Invalid)?;
        let permit = self.acquire_slot().await?;
        let client = client_key(client);
        self.throttle.lock().unwrap().try_reserve(client, &email, Instant::now())?;
        let mut reservation = Reservation { throttle: &self.throttle, client, username: &email, succeeded: false };
        if self.verify(&email, password, permit).await.is_none() {
            return Err(LoginError::Invalid);
        }
        reservation.succeeded = true;
        Ok(())
    }

    pub fn new_mfa_ticket(&self, user_id: i64) -> String {
        self.new_mfa_ticket_from(user_id, None)
    }

    fn new_mfa_ticket_from(&self, user_id: i64, origin: Option<(IpAddr, String)>) -> String {
        let ticket = new_token();
        let mut tickets = self.mfa_tickets.lock().unwrap();
        let now = Instant::now();
        tickets.retain(|_, (_, at, _, _)| now.duration_since(*at) < MFA_TICKET_TTL);
        tickets.insert(ticket.clone(), (user_id, now, 0, origin));
        ticket
    }

    /// 2차 인증 티켓의 사용자. 코드가 맞을 때까지 티켓은 남겨 두고, 맞으면 `take_mfa_ticket`으로 지운다.
    pub fn mfa_ticket_user(&self, ticket: &str) -> Option<i64> {
        let tickets = self.mfa_tickets.lock().unwrap();
        tickets
            .get(ticket)
            .filter(|(_, at, _, _)| at.elapsed() < MFA_TICKET_TTL)
            .map(|(id, _, _, _)| *id)
    }

    /// 티켓 하나에 코드 시도는 5번까지. 넘으면 티켓을 지운다(처음부터 다시 로그인).
    pub fn count_mfa_attempt(&self, ticket: &str) -> bool {
        let mut tickets = self.mfa_tickets.lock().unwrap();
        let Some(entry) = tickets.get_mut(ticket) else { return false };
        entry.2 += 1;
        if entry.2 > 5 {
            tickets.remove(ticket);
            return false;
        }
        true
    }

    /// 이 사용자의 진행 중인 2차 인증 티켓을 모두 버린다(계정을 지울 때).
    pub fn drop_mfa_tickets(&self, user_id: i64) {
        self.mfa_tickets.lock().unwrap().retain(|_, (u, _, _, _)| *u != user_id);
    }

    /// 2차 인증을 통과했다: 티켓을 지우고, 비밀번호로 로그인한 곳을 주인이 들어오는 곳으로 기억한다.
    pub fn take_mfa_ticket(&self, ticket: &str) {
        let taken = self.mfa_tickets.lock().unwrap().remove(ticket);
        if let Some((_, _, _, Some((client, username)))) = taken {
            self.throttle.lock().unwrap().remember(client, &username, Instant::now());
        }
    }

    /// 새 세션을 만들고 DB와 메모리에 모두 넣는다.
    /// 새 세션 토큰. DB에 저장하지 못하면 None(쿠키를 주고도 로그인되지 않는 일이 없게).
    pub async fn create_session(&self, user_id: i64, user_agent: &str) -> Option<String> {
        let token = new_token();
        let hash = token_hash(&token);
        let now = now_secs();
        let expires = now + SESSION_TTL.as_secs() as i64;
        let ua: String = user_agent.chars().take(200).collect();
        let h = hash.clone();
        // 사용자마다 최근 MAX_SESSIONS개만 둔다(로그인만 반복해 세션을 끝없이 쌓지 못하게). 넘친 옛 세션은 함께 지운다.
        let saved = self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, user_agent) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![h, user_id, now, expires, ua],
                )?;
                let old: Vec<String> = tx
                    .prepare(
                        "SELECT token_hash FROM sessions WHERE user_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT -1 OFFSET ?2",
                    )?
                    .query_map(params![user_id, MAX_SESSIONS], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                for h in &old {
                    tx.execute("DELETE FROM sessions WHERE token_hash = ?1", params![h])?;
                }
                tx.commit()?;
                Ok(old)
            })
            .await;
        // DB에 저장된 세션만 메모리에 둔다. 저장에 실패하면(예: 그사이 계정이 지워짐) 이 토큰은 로그인되지 않는다.
        match saved {
            Ok(old) => {
                let mut sessions = self.sessions.lock().unwrap();
                for h in old {
                    sessions.remove(&h);
                }
                sessions.insert(hash, Session { user_id, expires });
                Some(token)
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not persist session");
                None
            }
        }
    }

    /// 세션의 사용자 id(문자열). 회의실의 주최자 식별에 쓴다.
    pub fn session_user(&self, token: &str) -> Option<String> {
        self.session_user_id(token).map(|id| id.to_string())
    }

    pub fn session_user_id(&self, token: &str) -> Option<i64> {
        let hash = token_hash(token);
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get(&hash) {
            Some(s) if s.expires > now_secs() => Some(s.user_id),
            Some(_) => {
                sessions.remove(&hash);
                None
            }
            None => None,
        }
    }

    pub fn user_from_headers(&self, headers: &HeaderMap) -> Option<String> {
        token_from_headers(headers).and_then(|t| self.session_user(&t))
    }

    pub fn user_id_from_headers(&self, headers: &HeaderMap) -> Option<i64> {
        token_from_headers(headers).and_then(|t| self.session_user_id(&t))
    }

    pub fn logout(&self, token: &str) {
        let hash = token_hash(token);
        self.sessions.lock().unwrap().remove(&hash);
        let db = self.db.clone();
        tokio::spawn(async move {
            let _ = db
                .call(move |c| c.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash]))
                .await;
        });
    }

    /// 세션 마지막 사용 시각을 남긴다(로그인된 기기 목록용). 5분에 한 번만 쓴다.
    pub async fn touch(&self, token: &str) {
        let hash = token_hash(token);
        let now = now_secs();
        let _ = self
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE sessions SET last_seen_at = ?2 WHERE token_hash = ?1 AND COALESCE(last_seen_at, 0) < ?2 - 300",
                    params![hash, now],
                )
            })
            .await;
    }

    /// 세션 하나를 끊는다(토큰 해시 앞부분으로 찾고, 그 사용자의 것만). 끊었으면 true.
    pub async fn revoke_session_prefix(&self, user_id: i64, prefix: &str) -> bool {
        let p = prefix.to_owned();
        let removed: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare("SELECT token_hash FROM sessions WHERE user_id = ?1 AND substr(token_hash, 1, 16) = ?2")?;
                let hashes = stmt.query_map(params![user_id, p], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                for h in &hashes {
                    c.execute("DELETE FROM sessions WHERE token_hash = ?1", params![h])?;
                }
                Ok(hashes)
            })
            .await
            .unwrap_or_default();
        let mut sessions = self.sessions.lock().unwrap();
        for h in &removed {
            sessions.remove(h);
        }
        !removed.is_empty()
    }

    /// 한 사용자의 세션을 모두(또는 지금 세션만 남기고) 끊는다. 비밀번호 변경·재설정·계정 삭제 때.
    pub async fn revoke_user_sessions(&self, user_id: i64, keep_token: Option<&str>) {
        let keep = keep_token.map(token_hash);
        self.sessions
            .lock()
            .unwrap()
            .retain(|h, s| s.user_id != user_id || Some(h) == keep.as_ref());
        let _ = self
            .db
            .call(move |c| {
                c.execute(
                    "DELETE FROM sessions WHERE user_id = ?1 AND token_hash IS NOT ?2",
                    params![user_id, keep],
                )
            })
            .await;
    }

    /// 만료된 세션과 오래된 실패 기록을 정리한다(주기 작업).
    pub fn sweep(&self) {
        let now = now_secs();
        self.sessions.lock().unwrap().retain(|_, s| s.expires > now);
        self.mfa_tickets
            .lock()
            .unwrap()
            .retain(|_, (_, at, _, _)| at.elapsed() < MFA_TICKET_TTL);
        self.throttle.lock().unwrap().sweep(Instant::now());
        let db = self.db.clone();
        tokio::spawn(async move {
            let _ = db
                .call(move |c| c.execute("DELETE FROM sessions WHERE expires_at <= ?1", params![now]))
                .await;
        });
    }
}

/// 인증 전 가입 정보(인증 코드에 함께 보관). 인증될 때 계정에 넣는다.
/// 예전 판은 비밀번호 해시 문자열만 담았으므로 그것도 읽는다.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SignupPayload {
    pub hash: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub locale: String,
    /// 예전 판(해시만 담음): 이름·언어를 모르므로 계정의 값을 그대로 둔다.
    #[serde(skip)]
    pub legacy: bool,
}

impl SignupPayload {
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn decode(raw: &str) -> Option<Self> {
        if raw.starts_with('{') {
            serde_json::from_str(raw).ok()
        } else {
            Some(Self { hash: raw.to_owned(), name: String::new(), locale: String::new(), legacy: true })
        }
    }
}

struct LoginAccount {
    id: i64,
    hash: Option<String>,
    verified: bool,
    totp: bool,
}

/// 운영자 계정(`HOST_ACCOUNTS`, 개발용 계정)을 시작할 때 DB에 맞춘다. 목록이 기준이다.
/// - 목록의 계정: 인증 완료·요금제 있음으로 넣고 비밀번호를 맞춘다. 비밀번호가 바뀌었으면 세션을 끊는다.
/// - 예전에 운영자였는데 목록에서 빠진 계정: 비밀번호·요금제를 거두고 세션을 끊는다
///   (결제한 일반 계정으로 바뀐 경우는 요금제를 남긴다).
pub fn sync_operators(db: &Db, accounts: &HashMap<String, String>) -> rusqlite::Result<()> {
    let now = now_secs();
    db.blocking(|c| {
        let tx = c.transaction()?;
        let mut listed = Vec::new();
        for (email, hash) in accounts {
            let email = normalize_username(email);
            let old: Option<Option<String>> = tx
                .query_row("SELECT password_hash FROM users WHERE email = ?1", params![email], |r| r.get(0))
                .optional()?;
            tx.execute(
                "INSERT INTO users (email, name, password_hash, email_verified, plan, operator, created_at)
                 VALUES (?1, ?2, ?3, 1, 'pro', 1, ?4)
                 ON CONFLICT(email) DO UPDATE SET password_hash = excluded.password_hash, email_verified = 1,
                   operator = 1, plan = CASE WHEN users.plan = 'none' THEN 'pro' ELSE users.plan END",
                params![email, email.split('@').next().unwrap_or(""), hash, now],
            )?;
            if matches!(old, Some(Some(ref h)) if h != hash) {
                tx.execute(
                    "DELETE FROM sessions WHERE user_id = (SELECT id FROM users WHERE email = ?1)",
                    params![email],
                )?;
            }
            listed.push(email);
        }
        let mut stmt = tx.prepare("SELECT id, email, paddle_subscription_id FROM users WHERE operator = 1")?;
        let operators: Vec<(i64, String, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        for (id, email, subscription) in operators {
            if listed.contains(&email) {
                continue;
            }
            tracing::warn!(%email, "operator account removed from HOST_ACCOUNTS: revoking password and sessions");
            tx.execute(
                "UPDATE users SET operator = 0, password_hash = NULL,
                   plan = CASE WHEN ?2 IS NULL THEN 'none' ELSE plan END WHERE id = ?1",
                params![id, subscription],
            )?;
            tx.execute("DELETE FROM sessions WHERE user_id = ?1", params![id])?;
        }
        tx.commit()
    })
}

/// 세션 토큰은 DB에 해시로만 둔다(DB가 새도 세션을 훔칠 수 없게).
pub fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 실패 기록에 쓰는 클라이언트 키. IPv6는 보통 /64 단위로 한 사용자에게 주어지므로
/// 주소를 바꿔 가며 한도를 피하지 못하게 /64로 묶는다.
pub fn client_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & (u128::MAX << 64))),
        },
        v4 => v4,
    }
}

/// 넓은 단위: IPv6는 /48(보통 한 사이트·터널에 주는 크기), IPv4는 그대로.
pub fn net_key(ip: IpAddr) -> IpAddr {
    match client_key(ip) {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & (u128::MAX << 80))),
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

/// 계정 아이디는 이메일 주소다(예전 형식의 짧은 아이디도 그대로 받는다). 비교는 소문자로 한다.
pub fn is_valid_username(user: &str) -> bool {
    (1..=254).contains(&user.len())
        && user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-@+".contains(&b))
}

/// 이메일은 대소문자를 가리지 않는다. 저장·비교·시도 제한 모두 이 값으로 한다.
pub fn normalize_username(user: &str) -> String {
    user.trim().to_ascii_lowercase()
}

/// `HOST_ACCOUNTS` 형식: 공백·줄바꿈으로 구분한 `이메일:argon2 PHC 해시` 목록.
/// 이메일에는 `:`가 들어가지 않으므로 첫 `:`에서 나눈다.
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
        // 대소문자만 다른 같은 이메일이 두 번 있으면 어느 비밀번호가 쓰일지 모호하므로 거부한다.
        if accounts.insert(normalize_username(user), hash.to_owned()).is_some() {
            return Err(format!("duplicate account `{user}`"));
        }
    }
    Ok(accounts)
}

pub fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

pub fn new_token() -> String {
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
    async fn session_for_a_deleted_user_is_not_kept() {
        // 계정을 지우는 사이 끝난 로그인이 세션을 만들려 해도 DB에 없으면 메모리에도 두지 않는다.
        let auth = Auth::new(HashMap::new());
        let a = auth
            .db()
            .blocking(|c| {
                c.execute("INSERT INTO users (email, created_at) VALUES ('alice@x.co', 1)", [])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        auth.db().blocking(move |c| c.execute("DELETE FROM users WHERE id = ?1", params![a])).unwrap();
        // 저장하지 못한 세션은 토큰도 주지 않는다(쿠키를 받고도 로그인되지 않는 일이 없게).
        assert_eq!(auth.create_session(a, "").await, None);
    }

    #[tokio::test]
    async fn sessions_per_user_are_capped() {
        let auth = Auth::new(HashMap::new());
        let uid = auth
            .db()
            .blocking(|c| {
                c.execute("INSERT INTO users (email, created_at) VALUES ('a@x.co', 1)", [])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        let first = auth.create_session(uid, "").await.unwrap();
        for _ in 0..MAX_SESSIONS {
            auth.create_session(uid, "").await.unwrap();
        }
        let rows: i64 = auth
            .db()
            .blocking(move |c| c.query_row("SELECT count(*) FROM sessions WHERE user_id = ?1", params![uid], |r| r.get(0)))
            .unwrap();
        assert_eq!(rows, MAX_SESSIONS);
        // 가장 오래된 세션은 DB와 메모리에서 함께 빠진다.
        assert_eq!(auth.session_user_id(&first), None);
    }

    #[tokio::test]
    async fn login_succeeds_with_correct_password_and_creates_session() {
        let auth = auth_with("host", "correct horse");
        let token = auth.login(IP_A, "host", "correct horse").await.unwrap();
        assert_eq!(auth.session_user(&token).as_deref(), Some("1"));

        auth.logout(&token);
        assert_eq!(auth.session_user(&token), None);
    }

    #[tokio::test]
    async fn email_login_ignores_case_and_surrounding_spaces() {
        let auth = auth_with("Minjun+work@Company.com", "correct horse");
        let token = auth
            .login(IP_A, " minjun+WORK@company.com ", "correct horse")
            .await
            .unwrap();
        assert_eq!(
            auth.session_user(&token).as_deref(),
            Some("1")
        );
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

    #[test]
    fn logins_to_other_accounts_cannot_push_out_the_owners_known_client() {
        let mut t = Throttle::new(4);
        let now = Instant::now();
        t.remember(IP_B, "host", now);
        // 한 사람이 자기 계정으로 수많은 곳에서 들어와도 계정마다 따로 세므로 주인의 기억은 남는다.
        for i in 0..2000u32 {
            t.remember(IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + i)), "mallory", now);
        }
        assert!(t.is_known(IP_B, "host", now));
        assert_eq!(t.known["mallory"].len(), KNOWN_CLIENTS_PER_USER);
    }

    #[tokio::test]
    async fn a_password_without_the_second_factor_does_not_make_a_client_known() {
        let auth = auth_with("host", "correct horse");
        auth.db()
            .blocking(|c| c.execute("UPDATE users SET totp_enabled = 1 WHERE email = 'host'", []))
            .unwrap();
        let Err(LoginError::MfaRequired(ticket)) = auth.login(IP_B, "host", "correct horse").await else {
            panic!("2차 인증을 요구해야 한다");
        };
        assert!(!auth.throttle.lock().unwrap().is_known(IP_B, "host", Instant::now()));
        // 코드까지 맞혀 티켓을 쓰면 그때 기억한다.
        auth.take_mfa_ticket(&ticket);
        assert!(auth.throttle.lock().unwrap().is_known(IP_B, "host", Instant::now()));
    }

    #[tokio::test]
    async fn spread_failures_cannot_lock_the_owner_out_where_they_logged_in_before() {
        let auth = auth_with("host", "correct horse");
        assert!(auth.login(IP_B, "host", "correct horse").await.is_ok());
        for i in 0..MAX_USER_FAILURES / MAX_FAILURES {
            let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, i as u8));
            for _ in 0..MAX_FAILURES {
                assert_eq!(auth.login(ip, "host", "wrong").await, Err(LoginError::Invalid));
            }
        }
        // 처음 보는 곳은 상한에 막히고, 전에 들어왔던 곳에서는 들어온다.
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
    async fn one_client_spraying_many_accounts_is_stopped_but_correct_logins_are_not() {
        let auth = auth_with("host", "correct horse");
        // 맞게 들어오는 건 아무리 많아도 세지 않는다(같은 NAT 뒤의 여러 사람).
        for _ in 0..70 {
            assert!(auth.login(IP_A, "host", "correct horse").await.is_ok());
        }
        for i in 0..60 {
            assert_eq!(
                auth.login(IP_A, &format!("user{i}@example.test"), "guess").await,
                Err(LoginError::Invalid)
            );
        }
        assert_eq!(
            auth.login(IP_A, "host", "correct horse").await.unwrap_err(),
            LoginError::Locked
        );
        assert!(auth.login(IP_B, "host", "correct horse").await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_spraying_from_one_client_cannot_exceed_its_failure_limit() {
        let auth = Arc::new(auth_with("host", "correct horse"));
        let tasks: Vec<_> = (0..150)
            .map(|i| {
                let auth = auth.clone();
                tokio::spawn(async move { auth.login(IP_A, &format!("u{i}@example.test"), "guess").await })
            })
            .collect();
        let mut invalid = 0;
        for t in tasks {
            if t.await.unwrap() == Err(LoginError::Invalid) {
                invalid += 1;
            }
        }
        // 검증 중인 시도도 세므로 동시에 보내도 한도만큼만 비밀번호를 확인한다.
        assert!(invalid <= MAX_IP_FAILURES as usize, "{invalid} password checks");
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
            Err(LoginError::Busy)
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

    #[tokio::test]
    async fn operator_removed_from_list_loses_password_and_sessions() {
        let hash = hash_password("pw").unwrap();
        let db = Db::memory();
        sync_operators(&db, &HashMap::from([("ops@x.com".to_owned(), hash.clone())])).unwrap();
        let auth = Auth::with_db(db.clone());
        let token = auth.login(IP_A, "ops@x.com", "pw").await.unwrap();
        assert!(auth.session_user(&token).is_some());
        sync_operators(&db, &HashMap::new()).unwrap();
        // 재시작한 것처럼 다시 올린다.
        let auth = Auth::with_db(db);
        assert_eq!(auth.session_user(&token), None);
        assert_eq!(auth.login(IP_A, "ops@x.com", "pw").await, Err(LoginError::Invalid));
    }

    #[test]
    fn parses_accounts_and_rejects_malformed_entries() {
        let hash = hash_password("pw").unwrap();
        let accounts = parse_accounts(&format!("alice:{hash}\n bob:{hash}")).unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(parse_accounts("alice").is_err());
        assert!(parse_accounts(&format!("Alice@x.com:{hash} alice@x.com:{hash}")).is_err());
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
