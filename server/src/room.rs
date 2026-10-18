//! 방과 참가자 상태. 단일 프로세스 메모리에 둔다(MVP).
//!
//! 권한 규칙
//! - 방은 로그인한 주최자만 만든다(`create`).
//! - 주최자 계정으로 들어온 참가자는 바로 입장한다. 그 외(게스트)는 대기열에 들어가고,
//!   주최자가 수락해야 입장한다.
//! - 수락·거절·설정 변경은 그 방의 주최자 참가자만 할 수 있다. 모든 검사는 이 모듈에서 한다.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rand::RngExt;
use tokio::sync::{Notify, mpsc, watch};

use crate::ice::Ice;
use crate::protocol::{
    ErrorCode, Knock, MediaState, NoticeCode, Peer, PeerId, RoomSettings, ServerMsg, SignalData,
};

/// P2P mesh는 인원이 늘면 업링크가 (n-1)배가 되므로 4명으로 제한한다.
pub const MAX_PEERS: usize = 4;
/// 대기열 스팸 방지
pub const MAX_WAITING: usize = 8;
/// 주최자 한 명이 동시에 가질 수 있는 방
pub const MAX_ROOMS_PER_HOST: usize = 20;
/// 아무도 없는 방(만들고 아무도 안 들어왔거나 모두 나간 방)을 지우기까지의 시간
pub const ROOM_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
/// 연결이 끊긴 참가자가 주최자 수락 없이 돌아올 수 있는 시간
pub const RESUME_TTL: Duration = Duration::from_secs(2 * 60);
/// 음성 인식 토큰 발급 빈도(참가자당). 세션은 3분 넘게 조용하거나 언어가 바뀔 때 새로 연다. 평소 몇 분에 1개면 충분하다.
/// 연결이 아니라 참가자에 묶는다: 재입장 토큰으로 연결만 바꿔 가며 새 한도를 받지 못하게.
pub const STT_TOKENS_PER_SEC: f64 = 1.0 / 30.0;
pub const STT_TOKEN_BURST: f64 = 4.0;
/// 말(음성 인식 글자) 없이 받는 토큰은 더 묶는다: 말 없이 쓰는 세션은 번역 시간으로 세지 않으므로(말이 없으면 세지 않는 규칙),
/// 고친 화면이 토큰만 받아 글자를 보내지 않으면 무료로 쓰게 된다. 정직한 화면은 말이 없어도 세션을 미리 열어 두고
/// 3분 쉬면 새로 바꾼다(web/src/lib/stt.ts IDLE_ROTATE_MS): 3분에 1개. 처음·언어 바꾸기·연결이 몇 번 끊긴 몫으로 6개까지 더 받는다.
/// 글자가 오면 다시 찬다(말하는 사람은 일반 한도만 받는다). 세션은 15분까지 가므로 말 없는 세션은 참가자당 몇 개로 묶인다.
const TEXTLESS_TOKENS_PER_SEC: f64 = 1.0 / 180.0;
const TEXTLESS_TOKEN_BURST: f64 = 6.0;
/// 발급에 실패한 토큰의 일반 몫을 돌려주는 횟수(10분에 2번). 실패를 일부러 일으키는(이상한 언어 태그, 공유 키의 429) 화면이
/// 돌려받은 한도로 Gemini를 끝없이 부르지 못하게 묶고, 가끔 생기는 실패만 정직한 화면에 돌려준다.
const STT_REFUNDS_PER_SEC: f64 = 2.0 / 600.0;
/// 주최자 한 명(그의 모든 방)이 말 없이 받을 수 있는 토큰. 참가자마다만 세면 방과 참가자를 늘려 과금 없이 쓸 수 있으므로
/// 주최자 단위로도 묶는다. 정직하게 조용한 회의(3분마다 바꾸는 참가자 열두 명)까지는 걸리지 않고, 누군가 말하면 다시 찬다.
const HOST_TEXTLESS_TOKENS_PER_SEC: f64 = 1.0 / 15.0;
const HOST_TEXTLESS_TOKEN_BURST: f64 = 24.0;
const STT_REFUND_BURST: f64 = 2.0;
/// 번역 시간은 누군가 실제로 말한 뒤 이 시간까지만 센다(인원과 상관없이: 말없이 켜 둔 회의는 주최자의 시간을 쓰지 않는다).
const SPEECH_ACTIVE: Duration = Duration::from_secs(90);
/// 같은 사람의 음성 인식 글자가 이 간격 안에 이어 오면 그 사이를 말하는 중으로 본다(중간 결과는 말하는 동안 0.5초마다 온다).
/// 둘째 이후 동시에 말한 사람의 시간을 더할 때만 쓴다. 마지막 글자 뒤로 꼬리를 붙이지 않아 번갈아 말하면 겹치지 않는다.
/// 기본 시간처럼 화면이 보낸 글자를 믿는다(글자를 드물게 보내도록 고친 화면은 더한 시간을 피할 수 있다).
const TALK_GAP: Duration = Duration::from_secs(3);
/// 두 명 이상이 함께 말한 상태가 이만큼 이어져야 센다(이어지면 처음부터). 말을 넘겨받을 때 앞사람의 확정 글자가
/// 늦게 와서(최대 `TALK_GAP`) 겹치는 것은 세지 않도록 `TALK_GAP`보다 길게 둔다.
const OVERLAP_MIN: Duration = Duration::from_secs(4);
/// 주최자 계정으로 들어온 사람이 이만큼 없으면 남은 사람의 음성 인식·번역을 멈춘다(주최자 없이 그의 시간을 계속 쓰지 않게).
pub const HOST_AWAY: Duration = Duration::from_secs(10 * 60);

/// 혼동하기 쉬운 문자(l, o, 0, 1)를 뺀 32자. 4-4-4 형식이면 60bit.
const CODE_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";

#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<ServerMsg>,
    disconnect: Arc<Notify>,
}

impl Outbox {
    pub fn new(tx: mpsc::Sender<ServerMsg>, disconnect: Arc<Notify>) -> Self {
        Self { tx, disconnect }
    }

    /// 이 연결을 닫게 한다(같은 참가자가 새 연결로 돌아와 옛 연결을 밀어낼 때).
    pub fn close(&self) {
        self.disconnect.notify_one();
    }

    pub fn send(&self, msg: ServerMsg) {
        if let Err(err) = self.tx.try_send(msg) {
            // 제어 메시지가 누락된 연결은 복구할 수 없으므로 닫아 상태 불일치를 알린다.
            tracing::warn!(%err, "disconnecting slow or closed client");
            self.disconnect.notify_one();
        }
    }
}

struct Member {
    caption_epoch: u64,
    peer: Peer,
    tx: Outbox,
    stt_tokens: SttLimits,
}

/// 참가자 한 명의 음성 인식 토큰 한도: 모든 토큰, 그리고 마지막으로 말한 뒤의 토큰.
/// 두 한도를 모두 본 뒤에만 쓰고(거절된 요청이 한도를 쓰지 않게), 발급에 실패하면 돌려준다.
#[derive(Clone)]
struct SttLimits {
    all: Bucket,
    textless: Bucket,
    refunds: Bucket,
}

impl SttLimits {
    fn take(&mut self, now: Instant) -> bool {
        let ready = self.all.ready(now) & self.textless.ready(now);
        if ready {
            self.all.tokens -= 1.0;
            self.textless.tokens -= 1.0;
        }
        ready
    }

    /// 발급에 실패한 토큰: 세션이 열리지 않았으므로 말 없는 몫은 언제나 돌려준다(긴 장애 뒤에도 바로 다시 받게).
    /// Gemini를 부르는 횟수인 일반 몫은 돌려주는 횟수를 묶는다.
    fn give_back(&mut self, now: Instant) {
        self.textless.give_back();
        if self.refunds.ready(now) {
            self.refunds.tokens -= 1.0;
            self.all.give_back();
        }
    }

    /// 말한 글자가 왔다: 말 없는 토큰 한도를 다시 채운다.
    fn spoke(&mut self) {
        self.textless.tokens = self.textless.burst;
    }
}

/// 초당 `rate`개, 최대 `burst`개(limit::TokenBucket과 같고, 먼저 볼 수 있고 돌려줄 수 있다).
#[derive(Clone)]
struct Bucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn new(rate: f64, burst: f64) -> Self {
        Bucket { rate, burst, tokens: burst, at: Instant::now() }
    }

    fn ready(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.at = now;
        self.tokens >= 1.0
    }

    fn give_back(&mut self) {
        self.tokens = (self.tokens + 1.0).min(self.burst);
    }
}

fn new_stt_tokens() -> SttLimits {
    SttLimits {
        all: Bucket::new(STT_TOKENS_PER_SEC, STT_TOKEN_BURST),
        textless: Bucket::new(TEXTLESS_TOKENS_PER_SEC, TEXTLESS_TOKEN_BURST),
        refunds: Bucket::new(STT_REFUNDS_PER_SEC, STT_REFUND_BURST),
    }
}

struct Room {
    host_account: String,
    settings: RoomSettings,
    caption_epoch: u64,
    caption_changes: watch::Sender<u64>,
    members: HashMap<PeerId, Member>,
    /// 도착 순서대로(주최자에게 먼저 온 사람부터 보인다). 최대 `MAX_WAITING`명이라 찾기는 훑어도 된다.
    waiting: Vec<Member>,
    /// 입장한 참가자가 없어진 시점(생성 직후 포함). 입장한 참가자가 있으면 None.
    /// 대기 중인 게스트만으로는 방이 유지되지 않는다(방을 지울 때 그들에게 알린다).
    idle_since: Option<Instant>,
    /// 재입장 토큰 → 참가자. 입장할 때 하나씩 발급하고, 쓰면 없어진다.
    resumes: HashMap<String, Resume>,
    /// 번역 시간 측정: 마지막으로 잰 시점과 아직 보고하지 않은 초.
    /// 입장·퇴장·번역 켜고 끄기마다 그 앞까지를 쌓는다(주기 측정 순간의 상태만 보면 그 순간만 피해 갈 수 있다).
    metered_at: Instant,
    unreported: f64,
    /// 마지막으로 누군가 실제로 말한 때(음성 인식 글자가 도착함). 번역 시간은 이것으로 센다.
    /// 토큰 발급은 세지 않는다: 말하지 않아도 세션을 미리 열고 몇 분마다 바꾼다.
    used_at: Option<Instant>,
    /// 참가자별로 말한 구간(나간 참가자 포함, 아직 정산하지 않은 것만). 두 명 이상 겹친 시간을 더 센다(`settle_overlap`).
    talks: HashMap<PeerId, Talk>,
    /// 겹친 시간을 여기까지 정산했다. 이보다 `TALK_GAP` 늦게 따라간다(그 사이 구간은 다음 글자가 와야 정해진다).
    overlap_until: Instant,
    /// 정산 경계까지 이어진 겹침(다음 정산에서 이어 잰다).
    overlap: Option<Overlap>,
    /// 주최자 계정으로 들어온 사람이 모두 나간 때(방을 만든 뒤 아직 안 들어온 경우 포함). 있으면 None.
    host_left_at: Option<Instant>,
    /// 남은 사람에게 주최자가 없다고(`HostAway`) 알렸는지
    host_away_told: bool,
    /// 측정할 때 본 최대 인원과 마지막으로 보고한 값
    peak: usize,
    reported_peak: usize,
    /// 예약 회의 방. 주최자보다 먼저 와서 기다리는 게스트가 있으면 지우지 않는다
    /// (늦는 주최자를 기다리던 사람에게 회의가 없어졌다고 알리지 않게).
    reserved: bool,
}

/// 한 참가자가 말한 구간들. `TALK_GAP` 안에 이어 온 글자 사이만 구간이 된다(외따로 온 글자는 길이 0).
#[derive(Default)]
struct Talk {
    last: Option<Instant>,
    spans: Vec<(Instant, Instant)>,
}

/// 이어지고 있는 겹침: 시작 시각과 `OVERLAP_MIN`을 채우기 전까지 쌓아 둔 초.
struct Overlap {
    since: Instant,
    pending: f64,
}

struct Resume {
    peer: PeerId,
    /// 주최자가 수락한 이름. 다시 들어올 때 요청의 이름 대신 쓴다(수락 없이 이름을 바꾸지 못하게).
    name: String,
    /// 그 참가자가 나간 시점. 아직 입장해 있으면 None(끊긴 걸 서버가 아직 모르는 경우 포함).
    left_at: Option<Instant>,
    /// 나갈 때 남은 음성 인식 토큰 한도. 다시 들어오면 이어 받는다.
    stt_tokens: Option<SttLimits>,
}

impl Room {
    fn new(host_account: &str, settings: RoomSettings) -> Self {
        let now = Instant::now();
        Room {
            host_account: host_account.to_owned(),
            settings,
            caption_epoch: 0,
            caption_changes: watch::channel(0).0,
            members: HashMap::new(),
            waiting: Vec::new(),
            resumes: HashMap::new(),
            idle_since: Some(now),
            metered_at: now,
            unreported: 0.0,
            used_at: None,
            talks: HashMap::new(),
            overlap_until: now,
            overlap: None,
            host_left_at: Some(now),
            host_away_told: false,
            peak: 0,
            reported_peak: 0,
            reserved: false,
        }
    }

    /// 지난 측정부터 지금까지를 쌓는다. 번역이 켜져 있을 때 마지막으로 말한 뒤 `SPEECH_ACTIVE`까지만 세고,
    /// 주최자가 `HOST_AWAY`보다 오래 없으면 그 뒤로는 세지 않는다.
    /// 여기에 둘째 이후 동시에 말한 사람의 시간을 더한다(`settle_overlap`): n명이 함께 말하면 n배로 센다.
    /// `blocked`: 주최자의 번역 시간이 떨어져 자막을 막는 중이면 세지 않는다(쓴 시간을 부풀리지 않게).
    /// 세는 조건이 바뀌는 곳(입장·퇴장·설정·막힘)마다 바뀌기 전에 부른다.
    fn accrue(&mut self, now: Instant, blocked: bool) {
        // 말한 구간은 자막을 만들 수 있을 때(번역 켜짐·막히지 않음·주최자 있음) 받은 글자로만 생기므로 조건 없이 정산한다.
        self.unreported += self.settle_overlap(now);
        let from = std::mem::replace(&mut self.metered_at, now);
        if blocked || !self.settings.translation || self.members.is_empty() {
            return;
        }
        let Some(spoke) = self.used_at else { return };
        let mut until = (spoke + SPEECH_ACTIVE).min(now);
        if let Some(left) = self.host_left_at {
            until = until.min(left + HOST_AWAY);
        }
        self.unreported += until.saturating_duration_since(from).as_secs_f64();
    }

    /// 이 참가자가 `now`에 말한 글자를 보냈다. 직전 글자와 `TALK_GAP` 안이면 그 사이를 말한 구간으로 둔다.
    fn talked(&mut self, id: &str, now: Instant) {
        let talk = self.talks.entry(id.to_owned()).or_default();
        if let Some(last) = talk.last.filter(|&l| now > l && now - l <= TALK_GAP) {
            match talk.spans.last_mut() {
                Some(span) if span.1 == last => span.1 = now,
                _ => talk.spans.push((last, now)),
            }
        }
        talk.last = Some(now);
    }

    /// 지난 정산부터 `now - TALK_GAP`까지 두 명 이상이 함께 말한 시간을 (인원 - 1)배로 더한 초(`OVERLAP_MIN` 넘게 이어진 것만).
    /// 그 뒤는 아직 정하지 않는다: 지금 이후에 오는 글자는 직전 글자부터 구간을 만드는데, 그 시작은 `now - TALK_GAP`보다 늦다.
    fn settle_overlap(&mut self, now: Instant) -> f64 {
        let from = self.overlap_until;
        let Some(upto) = now.checked_sub(TALK_GAP).filter(|&u| u > from) else {
            return 0.0;
        };
        self.overlap_until = upto;
        let mut edges: Vec<(Instant, i32)> = Vec::new();
        for talk in self.talks.values() {
            for &(start, end) in &talk.spans {
                let (start, end) = (start.max(from), end.min(upto));
                if start < end {
                    edges.push((start, 1));
                    edges.push((end, -1));
                }
            }
        }
        // 같은 시각이면 끝(-1)이 시작(+1)보다 앞선다: 한 사람이 멈춘 순간 다른 사람이 시작하면 겹치지 않는다.
        edges.sort_unstable();
        // 마지막 구간 끝부터 `upto`까지(아무도 말하지 않음)도 지나가야 끝난 겹침을 버린다.
        edges.push((upto, 0));
        let (mut talking, mut prev, mut extra) = (0i32, from, 0.0);
        for (at, delta) in edges {
            if at > prev {
                if talking > 1 {
                    let run = self.overlap.get_or_insert(Overlap { since: prev, pending: 0.0 });
                    run.pending += f64::from(talking - 1) * (at - prev).as_secs_f64();
                    if at - run.since >= OVERLAP_MIN {
                        extra += std::mem::take(&mut run.pending);
                    }
                } else {
                    self.overlap = None;
                }
            }
            talking += delta;
            prev = at;
        }
        self.talks.retain(|_, talk| {
            talk.spans.retain(|span| span.1 > upto);
            // 마지막 글자가 `upto`보다 앞이면 다음 글자와 이어질 수 없다.
            !talk.spans.is_empty() || talk.last.is_some_and(|l| l >= upto)
        });
        extra
    }

    /// 남은 사람에게 주최자가 없다고 한 번 알린다(`except`는 이미 따로 알린 참가자). 알린 뒤 돌아오면 `HostBack`을 보낸다.
    fn tell_host_away(&mut self, except: &str) {
        if !std::mem::replace(&mut self.host_away_told, true) {
            broadcast(&self.members, except, ServerMsg::Notice { code: NoticeCode::HostAway });
        }
    }

    /// 주최자 계정으로 들어온 사람이 `HOST_AWAY`보다 오래 없는지.
    fn host_away(&self, now: Instant) -> bool {
        self.host_left_at
            .is_some_and(|t| now.saturating_duration_since(t) >= HOST_AWAY)
    }

    fn issue_resume(&mut self, peer: &str, name: &str) -> String {
        let token = random_token(RESUME_LEN);
        self.resumes.insert(
            token.clone(),
            Resume {
                peer: peer.to_owned(),
                name: name.to_owned(),
                left_at: None,
                stt_tokens: None,
            },
        );
        token
    }
}

impl Room {
    fn is_host_member(&self, id: &str) -> bool {
        self.members.get(id).is_some_and(|m| m.peer.host)
    }

    fn notify_hosts(&self, msg: ServerMsg) {
        for m in self.members.values().filter(|m| m.peer.host) {
            send(&m.tx, msg.clone());
        }
    }

    fn take_waiting(&mut self, id: &str) -> Option<Member> {
        let i = self.waiting.iter().position(|w| w.peer.id == id)?;
        Some(self.waiting.remove(i))
    }

    fn knocks(&self) -> Vec<Knock> {
        self.waiting
            .iter()
            .map(|w| Knock {
                id: w.peer.id.clone(),
                name: w.peer.name.clone(),
                lang: w.peer.lang.clone(),
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptionEpoch(pub u64, pub u64);

#[derive(Clone)]
pub struct Audience {
    pub epoch: CaptionEpoch,
    pub src_lang: String,
    pub partner_lang: String,
    /// 화자 자신을 포함한 입장한 참가자 전원
    pub readers: Vec<Reader>,
}

#[derive(Clone)]
pub struct Reader {
    pub id: PeerId,
    pub lang: String,
    pub tx: Outbox,
}

impl Audience {
    /// 이 독자에게 보여줄 번역 언어. 원문과 다른 자기 언어가 있으면 그것, 아니면 화자가 고른 상대 언어.
    /// 원문과 같은 언어가 나오면 번역할 필요가 없다.
    pub fn target_for<'a>(&'a self, reader_lang: &'a str) -> Option<&'a str> {
        let tgt = if reader_lang != self.src_lang {
            reader_lang
        } else {
            self.partner_lang.as_str()
        };
        (tgt != self.src_lang).then_some(tgt)
    }
}

#[derive(Default)]
pub struct Rooms {
    rooms: Mutex<HashMap<String, Room>>,
    ice: Ice,
    /// 이번 결제 기간의 번역 시간을 다 쓴 주최자(계정 id). 이들의 방은 음성 인식 토큰을 받지 못한다.
    out_of_time: Mutex<HashSet<String>>,
    /// 주최자별 말 없는 토큰 한도(`HOST_TEXTLESS_*`). 잠금 순서: `rooms` 다음에 잡는다.
    host_textless: Mutex<HashMap<String, Bucket>>,
}

/// 번역 시간 측정 한 번의 결과: 이 방에서 지난 측정 뒤로 번역이 돌아간 초.
#[derive(Clone)]
pub struct Metered {
    pub host: String,
    pub code: String,
    pub seconds: i64,
    /// 지금까지 한꺼번에 입장해 있던 최대 인원(회의 기록용)
    pub peak: usize,
}

pub struct JoinRequest {
    pub name: String,
    pub lang: String,
    pub partner_lang: String,
    pub media: MediaState,
    pub resume: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JoinOutcome {
    /// 입장 완료. Welcome은 이미 보냈다.
    Joined(PeerId),
    /// 주최자 수락 대기. Waiting은 이미 보냈다.
    Waiting(PeerId),
}

impl Rooms {
    pub fn new(ice: Ice) -> Self {
        Self {
            rooms: Mutex::default(),
            ice,
            out_of_time: Mutex::default(),
            host_textless: Mutex::default(),
        }
    }

    pub fn create(&self, host_account: &str, settings: RoomSettings) -> Result<String, ErrorCode> {
        let mut rooms = self.rooms.lock().unwrap();
        if rooms
            .values()
            // 손님이 링크를 열어 생긴 예약 회의 방은 주최자가 들어오기 전까지 세지 않는다.
            .filter(|r| r.host_account == host_account && !(r.reserved && r.members.is_empty()))
            .count()
            >= MAX_ROOMS_PER_HOST
        {
            return Err(ErrorCode::Forbidden);
        }
        let code = loop {
            let code = new_room_code();
            if !rooms.contains_key(&code) {
                break code;
            }
        };
        rooms.insert(code.clone(), Room::new(host_account, settings));
        Ok(code)
    }

    /// 예약해 둔 코드로 방을 연다(누군가 링크를 처음 열 때). 이미 있으면 그대로 둔다.
    pub fn open_reserved(&self, host_account: &str, code: &str, settings: RoomSettings) {
        self.rooms
            .lock()
            .unwrap()
            .entry(code.to_owned())
            .or_insert_with(|| Room::new(host_account, settings))
            .reserved = true;
    }

    /// 회의를 지웠을 때: 방을 닫고 안에 있던 사람과 기다리던 게스트에게 알린다. 연결은 ws가 곧 닫는다.
    pub fn close(&self, code: &str) {
        let Some(room) = self.rooms.lock().unwrap().remove(code) else {
            return;
        };
        let gone = ErrorCode::RoomNotFound;
        for m in room.members.values().chain(&room.waiting) {
            send(&m.tx, ServerMsg::Error { code: gone, message: format!("{gone:?}") });
        }
    }

    pub fn exists(&self, code: &str) -> bool {
        self.rooms.lock().unwrap().contains_key(code)
    }

    /// 새 회의 코드(메모리의 방과 겹치지 않게). 예약은 DB와도 겹치지 않는지 따로 확인한다.
    pub fn fresh_code(&self) -> String {
        let rooms = self.rooms.lock().unwrap();
        loop {
            let code = new_room_code();
            if !rooms.contains_key(&code) {
                return code;
            }
        }
    }

    /// 번역 시간을 잰다(주기 작업). 번역이 켜져 있고 누군가 말하는 동안 세고, 함께 말한 사람 수만큼 곱한다(`Room::accrue`).
    /// 1초 미만은 다음 측정으로 넘긴다. 주최자가 `HOST_AWAY`보다 오래 없는 방에는 그때 한 번 알린다.
    pub fn meter(&self, now: Instant) -> Vec<Metered> {
        let blocked = self.out_of_time_hosts();
        let mut rooms = self.rooms.lock().unwrap();
        let mut out = Vec::new();
        for (code, room) in rooms.iter_mut() {
            room.accrue(now, blocked.contains(&room.host_account));
            if room.settings.translation && room.host_away(now) {
                room.tell_host_away("");
            }
            room.peak = room.peak.max(room.members.len());
            let whole = room.unreported.floor();
            if whole >= 1.0 || room.peak > room.reported_peak {
                room.unreported -= whole;
                room.reported_peak = room.peak;
                out.push(Metered {
                    host: room.host_account.clone(),
                    code: code.clone(),
                    seconds: whole as i64,
                    peak: room.peak,
                });
            }
        }
        out
    }

    /// 시간을 다 쓴 주최자 목록을 바꾼다. 새로 막히거나 풀린 주최자의 방에는 모두에게 알린다.
    pub fn set_out_of_time(&self, hosts: HashSet<String>) {
        self.set_out_of_time_at(hosts, Instant::now());
    }

    fn set_out_of_time_at(&self, hosts: HashSet<String>, now: Instant) {
        // 잠금 순서는 다른 곳과 같게 방 → 시간 목록.
        let mut rooms = self.rooms.lock().unwrap();
        let (blocked, restored): (Vec<String>, Vec<String>) = {
            let mut cur = self.out_of_time.lock().unwrap();
            let blocked = hosts.difference(&cur).cloned().collect();
            let restored = cur.difference(&hosts).cloned().collect();
            *cur = hosts;
            (blocked, restored)
        };
        if blocked.is_empty() && restored.is_empty() {
            return;
        }
        for room in rooms.values_mut() {
            // 바뀌기 전 상태로 지금까지를 먼저 잰다(막히기 전까지는 세고, 막혀 있던 동안은 세지 않게).
            let code = if blocked.contains(&room.host_account) {
                room.accrue(now, false);
                NoticeCode::QuotaExceeded
            } else if restored.contains(&room.host_account) {
                room.accrue(now, true);
                NoticeCode::QuotaRestored
            } else {
                continue;
            };
            broadcast(&room.members, "", ServerMsg::Notice { code });
        }
    }

    pub fn out_of_time_hosts(&self) -> HashSet<String> {
        self.out_of_time.lock().unwrap().clone()
    }

    /// 열려 있는 방의 주최자들(아직 번역 시간을 재지 않은 방의 주최자도 시간이 남았는지 확인하려고).
    pub fn open_hosts(&self) -> HashSet<String> {
        self.rooms.lock().unwrap().values().map(|r| r.host_account.clone()).collect()
    }

    /// 잰 시간을 기록하지 못했을 때 되돌려 둔다(다음 측정에서 다시 보고하게).
    pub fn restore_metered(&self, metered: &[Metered]) {
        let mut rooms = self.rooms.lock().unwrap();
        for m in metered {
            if let Some(room) = rooms.get_mut(&m.code) {
                room.unreported += m.seconds as f64;
            }
        }
    }

    /// 이 방에서 지금 음성 인식 토큰을 주지 않는 까닭: 주최자의 번역 시간이 떨어졌거나 주최자가 오래 없다.
    /// 주최자가 없는 것을 여기서 처음 알았으면(주기 작업보다 먼저) 방 전원에게 알리고 알린 것으로 둔다
    /// (돌아왔을 때 `HostBack`을 보내게).
    pub fn stt_blocked(&self, code: &str, now: Instant) -> Option<NoticeCode> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code)?;
        if self.out_of_time.lock().unwrap().contains(&room.host_account) {
            Some(NoticeCode::QuotaExceeded)
        } else if room.host_away(now) {
            room.tell_host_away("");
            Some(NoticeCode::HostAway)
        } else {
            None
        }
    }

    pub fn settings(&self, code: &str) -> Option<RoomSettings> {
        self.rooms.lock().unwrap().get(code).map(|r| r.settings)
    }

    /// `account`는 요청자의 로그인 계정(없으면 게스트). 방 주최자 계정과 같을 때만 주최자로 입장한다.
    pub fn join(
        &self,
        code: &str,
        req: JoinRequest,
        account: Option<&str>,
        tx: Outbox,
    ) -> Result<JoinOutcome, ErrorCode> {
        let id = random_token(12);
        // 자격은 잠금 밖에서 만든다. 주최자가 아니면 수락 때 다시 만들므로 여기 것은 버려진다(수 μs).
        let ice_servers = self.ice.servers_for(&id, SystemTime::now());
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        let now = Instant::now();
        let blocked = self.out_of_time.lock().unwrap().contains(&room.host_account);
        // 1회용: 맞든 틀리든 쓴 토큰은 지운다.
        let mut resumed = req
            .resume
            .as_deref()
            .and_then(|token| room.resumes.remove(token))
            .filter(|r| r.left_at.is_none_or(|t| now.duration_since(t) < RESUME_TTL));
        // 같은 참가자의 남은 음성 인식 토큰 한도를 이어 받는다(옛 연결이 아직 있으면 그쪽이 최신).
        let mut stt_tokens = resumed.as_mut().and_then(|r| r.stt_tokens.take());
        if let Some(r) = &resumed
            && room.members.contains_key(&r.peer)
        {
            room.accrue(now, blocked);
            let old = room.members.remove(&r.peer).expect("checked above");
            if old.peer.host && !room.members.values().any(|m| m.peer.host) {
                room.host_left_at.get_or_insert(now);
            }
            // 서버가 아직 끊긴 줄 모르는 옛 연결(반쯤 열린 TCP)을 밀어낸다.
            old.tx.close();
            stt_tokens = Some(old.stt_tokens);
            room.caption_changes.send_modify(|v| *v += 1);
            broadcast(&room.members, &r.peer, ServerMsg::PeerLeft { id: r.peer.clone() });
        }
        if room.members.len() >= MAX_PEERS {
            return Err(ErrorCode::RoomFull);
        }
        let is_host = account == Some(room.host_account.as_str());
        if !is_host && resumed.is_none() && room.waiting.len() >= MAX_WAITING {
            return Err(ErrorCode::TooManyWaiting);
        }
        let peer = Peer {
            id: id.clone(),
            name: resumed.as_ref().map_or(req.name, |r| r.name.clone()),
            lang: req.lang,
            partner_lang: req.partner_lang,
            media: req.media,
            host: is_host,
        };

        // 주최자 계정이거나, 수락받은 적 있는 참가자가 재입장 토큰으로 돌아온 경우 바로 입장한다.
        if is_host || resumed.is_some() {
            room.idle_since = None;
            let resume = room.issue_resume(&id, &peer.name);
            let welcome = ServerMsg::Welcome {
                you: id.clone(),
                host: is_host,
                settings: room.settings,
                peers: room.members.values().map(|m| m.peer.clone()).collect(),
                knocks: if is_host { room.knocks() } else { Vec::new() },
                ice_servers,
                resume,
            };
            send(&tx, welcome);
            // 다시 연결한 화면도 지금 상태를 알게 입장 직후 알린다(그사이 바뀐 알림을 놓쳤을 수 있다).
            if blocked {
                send(&tx, ServerMsg::Notice { code: NoticeCode::QuotaExceeded });
            } else if !is_host && room.settings.translation && room.host_away(now) {
                send(&tx, ServerMsg::Notice { code: NoticeCode::HostAway });
                room.tell_host_away(&id);
            }
            broadcast(
                &room.members,
                &id,
                ServerMsg::PeerJoined { peer: peer.clone() },
            );
            room.accrue(now, blocked);
            if is_host {
                room.host_left_at = None;
                if std::mem::take(&mut room.host_away_told) {
                    broadcast(&room.members, &id, ServerMsg::Notice { code: NoticeCode::HostBack });
                }
            }
            room.members.insert(
                id.clone(),
                Member {
                    caption_epoch: 0,
                    peer,
                    tx,
                    stt_tokens: stt_tokens.unwrap_or_else(new_stt_tokens),
                },
            );
            room.peak = room.peak.max(room.members.len());
            room.caption_changes.send_modify(|v| *v += 1);
            return Ok(JoinOutcome::Joined(id));
        }

        send(&tx, ServerMsg::Waiting);
        room.notify_hosts(ServerMsg::Knock {
            knock: Knock {
                id: id.clone(),
                name: peer.name.clone(),
                lang: peer.lang.clone(),
            },
        });
        room.waiting.push(Member {
            caption_epoch: 0,
            peer,
            tx,
            stt_tokens: new_stt_tokens(),
        });
        Ok(JoinOutcome::Waiting(id))
    }

    pub fn admit(&self, code: &str, by: &str, guest: &str) -> Result<(), ErrorCode> {
        // 자격은 잠금 밖에서 만든다. 수락이 거절되면 버려진다.
        // 권한 확인 전이라 아무 문자열에나 계산될 수 있지만 HMAC 한 번이고 Welcome으로만 나간다.
        let ice_servers = self.ice.servers_for(guest, SystemTime::now());
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        if !room.is_host_member(by) {
            return Err(ErrorCode::Forbidden);
        }
        if !room.waiting.iter().any(|w| w.peer.id == guest) {
            return Err(ErrorCode::UnknownPeer);
        }
        // 가득 찬 상태에서 수락하면 게스트는 대기열에 그대로 남는다(자리가 나면 다시 수락 가능).
        if room.members.len() >= MAX_PEERS {
            return Err(ErrorCode::RoomFull);
        }
        let member = room.take_waiting(guest).expect("checked above");
        let resume = room.issue_resume(guest, &member.peer.name);
        let welcome = ServerMsg::Welcome {
            you: guest.to_owned(),
            host: false,
            settings: room.settings,
            peers: room.members.values().map(|m| m.peer.clone()).collect(),
            knocks: Vec::new(),
            ice_servers,
            resume,
        };
        send(&member.tx, welcome);
        let blocked = self.out_of_time.lock().unwrap().contains(&room.host_account);
        if blocked {
            send(&member.tx, ServerMsg::Notice { code: NoticeCode::QuotaExceeded });
        }
        broadcast(
            &room.members,
            guest,
            ServerMsg::PeerJoined {
                peer: member.peer.clone(),
            },
        );
        room.accrue(Instant::now(), blocked);
        room.members.insert(guest.to_owned(), member);
        room.peak = room.peak.max(room.members.len());
        room.caption_changes.send_modify(|v| *v += 1);
        room.notify_hosts(ServerMsg::KnockResolved {
            id: guest.to_owned(),
        });
        Ok(())
    }

    pub fn deny(&self, code: &str, by: &str, guest: &str) -> Result<(), ErrorCode> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        if !room.is_host_member(by) {
            return Err(ErrorCode::Forbidden);
        }
        let member = room.take_waiting(guest).ok_or(ErrorCode::UnknownPeer)?;
        send(&member.tx, ServerMsg::Denied);
        room.notify_hosts(ServerMsg::KnockResolved {
            id: guest.to_owned(),
        });
        Ok(())
    }

    pub fn set_settings(
        &self,
        code: &str,
        by: &str,
        settings: RoomSettings,
    ) -> Result<(), ErrorCode> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        if !room.is_host_member(by) {
            return Err(ErrorCode::Forbidden);
        }
        room.accrue(Instant::now(), self.out_of_time.lock().unwrap().contains(&room.host_account));
        if room.settings.translation != settings.translation {
            room.caption_epoch += 1;
            room.caption_changes.send_modify(|v| *v += 1);
        }
        room.settings = settings;
        for m in room.members.values() {
            send(&m.tx, ServerMsg::Settings { settings });
        }
        Ok(())
    }

    /// 입장한 참가자만 자기 정보를 바꿀 수 있다(대기 중인 게스트는 무시).
    pub fn update(&self, code: &str, id: &str, f: impl FnOnce(&mut Peer)) {
        self.apply(code, id, true, f);
    }

    /// 상태만 바꾸고 다른 참가자에게 알리는 것은 미룬다(빈도 제한 중). 나중에 `announce`로 최신 상태를 한 번 보낸다.
    pub fn update_quietly(&self, code: &str, id: &str, f: impl FnOnce(&mut Peer)) {
        self.apply(code, id, false, f);
    }

    /// 이 참가자의 현재 상태를 다른 참가자에게 알린다.
    pub fn announce(&self, code: &str, id: &str) {
        let rooms = self.rooms.lock().unwrap();
        if let Some(room) = rooms.get(code)
            && let Some(member) = room.members.get(id)
        {
            let peer = member.peer.clone();
            broadcast(&room.members, id, ServerMsg::PeerUpdated { peer });
        }
    }

    fn apply(&self, code: &str, id: &str, announce: bool, f: impl FnOnce(&mut Peer)) {
        let mut rooms = self.rooms.lock().unwrap();
        let Some(room) = rooms.get_mut(code) else {
            return;
        };
        let Some(member) = room.members.get_mut(id) else {
            return;
        };
        let previous_lang = member.peer.lang.clone();
        let previous_partner = member.peer.partner_lang.clone();
        f(&mut member.peer);
        if previous_lang != member.peer.lang {
            member.caption_epoch += 1;
        }
        if previous_lang != member.peer.lang || previous_partner != member.peer.partner_lang {
            room.caption_changes.send_modify(|v| *v += 1);
        }
        if announce {
            let peer = member.peer.clone();
            broadcast(&room.members, id, ServerMsg::PeerUpdated { peer });
        }
    }

    /// 입장한 참가자끼리만 시그널을 중계한다. 대기 중인 게스트·다른 방 참가자는 거부한다.
    pub fn relay_signal(
        &self,
        code: &str,
        from: &str,
        to: &str,
        data: SignalData,
    ) -> Result<(), ErrorCode> {
        let rooms = self.rooms.lock().unwrap();
        let room = rooms.get(code).ok_or(ErrorCode::UnknownPeer)?;
        if !room.members.contains_key(from) || from == to {
            return Err(ErrorCode::UnknownPeer);
        }
        let target = room.members.get(to).ok_or(ErrorCode::UnknownPeer)?;
        send(
            &target.tx,
            ServerMsg::Signal {
                from: from.to_owned(),
                data,
            },
        );
        Ok(())
    }

    pub fn leave(&self, code: &str, id: &str) {
        let mut rooms = self.rooms.lock().unwrap();
        let Some(room) = rooms.get_mut(code) else {
            return;
        };
        room.accrue(Instant::now(), self.out_of_time.lock().unwrap().contains(&room.host_account));
        if let Some(member) = room.members.remove(id) {
            room.caption_changes.send_modify(|v| *v += 1);
            broadcast(&room.members, id, ServerMsg::PeerLeft { id: id.to_owned() });
            let now = Instant::now();
            for r in room.resumes.values_mut().filter(|r| r.peer == id) {
                r.left_at.get_or_insert(now);
                r.stt_tokens = Some(member.stt_tokens.clone());
            }
        } else if room.take_waiting(id).is_some() {
            room.notify_hosts(ServerMsg::KnockResolved { id: id.to_owned() });
        }
        if room.host_left_at.is_none() && !room.members.values().any(|m| m.peer.host) {
            room.host_left_at = Some(Instant::now());
        }
        if room.members.is_empty() {
            room.idle_since.get_or_insert_with(Instant::now);
        }
    }

    /// 이 참가자가 지금 입장해 있는지(자막 파이프라인이 화자가 나갔는지 가리는 데 쓴다).
    pub fn is_member(&self, code: &str, id: &str) -> bool {
        self.rooms
            .lock()
            .unwrap()
            .get(code)
            .is_some_and(|r| r.members.contains_key(id))
    }

    pub fn caption_changes(&self, code: &str) -> Option<watch::Receiver<u64>> {
        self.rooms
            .lock()
            .unwrap()
            .get(code)
            .map(|r| r.caption_changes.subscribe())
    }

    /// 화자의 자막을 받을 사람들. 번역이 꺼진 방이거나 화자가 입장한 참가자가 아니면 None.
    /// 잠금은 복사하는 동안만 잡는다(보내는 쪽 채널은 복제 비용이 작다).
    pub fn caption_audience(&self, code: &str, speaker: &str) -> Option<Audience> {
        self.audience(code, speaker, None)
    }

    /// 화자가 실제로 말한 글자를 받았을 때의 `caption_audience`. 이때부터 `SPEECH_ACTIVE` 동안 번역 시간으로 센다.
    pub fn speech_audience(&self, code: &str, speaker: &str, now: Instant) -> Option<Audience> {
        self.audience(code, speaker, Some(now))
    }

    fn audience(&self, code: &str, speaker: &str, spoke_at: Option<Instant>) -> Option<Audience> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code)?;
        // 주최자의 번역 시간이 떨어졌거나 주최자가 오래 없으면 자막을 만들지 않는다(이미 받은 음성 인식 세션이 남아 있어도).
        if !room.settings.translation
            || self.out_of_time.lock().unwrap().contains(&room.host_account)
            || room.host_away(spoke_at.unwrap_or_else(Instant::now))
        {
            return None;
        }
        let me = room.members.get(speaker)?;
        let audience = Audience {
            epoch: CaptionEpoch(room.caption_epoch, me.caption_epoch),
            src_lang: me.peer.lang.clone(),
            partner_lang: me.peer.partner_lang.clone(),
            readers: room
                .members
                .values()
                .map(|m| Reader {
                    id: m.peer.id.clone(),
                    lang: m.peer.lang.clone(),
                    tx: m.tx.clone(),
                })
                .collect(),
        };
        if let Some(now) = spoke_at {
            // 한동안 말이 없다가 다시 말하면 그 사이(쓰지 않은 시간)는 세지 않도록 먼저 잰다.
            room.accrue(now, false);
            room.used_at = Some(now);
            room.talked(speaker, now);
            if let Some(m) = room.members.get_mut(speaker) {
                m.stt_tokens.spoke();
            }
            if let Some(host) = self.host_textless.lock().unwrap().get_mut(&room.host_account) {
                host.tokens = host.burst;
            }
        }
        Some(audience)
    }

    /// 이 참가자가 음성 인식 토큰을 하나 더 받아도 되는지(받으면 한도를 하나 쓴다). 입장한 참가자만.
    pub fn take_stt_token(&self, code: &str, id: &str, now: Instant) -> bool {
        let mut rooms = self.rooms.lock().unwrap();
        let Some(room) = rooms.get_mut(code) else { return false };
        let Some(m) = room.members.get_mut(id) else { return false };
        let mut hosts = self.host_textless.lock().unwrap();
        let host = hosts
            .entry(room.host_account.clone())
            .or_insert_with(|| Bucket::new(HOST_TEXTLESS_TOKENS_PER_SEC, HOST_TEXTLESS_TOKEN_BURST));
        // 주최자 몫이 남아 있을 때만 참가자 몫을 쓴다(막힌 요청이 어느 쪽도 깎지 않게).
        if !host.ready(now) || !m.stt_tokens.take(now) {
            return false;
        }
        host.tokens -= 1.0;
        true
    }

    /// 발급에 실패한 토큰의 한도를 돌려준다(Gemini 오류로 다시 요청하는 정직한 화면이 한도를 잃지 않게). 돌려주는 횟수는 묶는다.
    pub fn give_back_stt_token(&self, code: &str, id: &str, now: Instant) {
        let mut rooms = self.rooms.lock().unwrap();
        let Some(room) = rooms.get_mut(code) else { return };
        if let Some(m) = room.members.get_mut(id) {
            m.stt_tokens.give_back(now);
            if let Some(host) = self.host_textless.lock().unwrap().get_mut(&room.host_account) {
                host.give_back();
            }
        }
    }

    /// 음성 인식 토큰에 잠글 언어: 내 언어, 그리고 다르면 상대 언어(상대 언어로 한 말도 그 언어 글자로 적게).
    /// 입장한 참가자이고 번역이 켜진 방이어야 한다(아니면 None).
    pub fn stt_langs(&self, code: &str, id: &str) -> Option<Vec<String>> {
        let rooms = self.rooms.lock().unwrap();
        let room = rooms.get(code).filter(|r| r.settings.translation)?;
        let peer = &room.members.get(id)?.peer;
        let mut langs = vec![peer.lang.clone()];
        if peer.partner_lang != peer.lang {
            langs.push(peer.partner_lang.clone());
        }
        Some(langs)
    }

    /// 오래 빈 방을 지운다(주기 작업). 지운 방의 코드를 돌려준다(회의 기록에 끝난 시각을 남기려고).
    pub fn sweep(&self, now: Instant) -> Vec<String> {
        let mut rooms = self.rooms.lock().unwrap();
        let mut ended = Vec::new();
        rooms.retain(|code, r| {
            let keep = (r.reserved && !r.waiting.is_empty())
                || r.idle_since.is_none_or(|t| now.duration_since(t) < ROOM_IDLE_TTL);
            if !keep {
                // 주최자를 기다리기만 하던 게스트에게 회의가 없어졌다고 알린다. 연결은 ws가 곧 닫는다.
                let gone = ErrorCode::RoomNotFound;
                for w in &r.waiting {
                    send(&w.tx, ServerMsg::Error { code: gone, message: format!("{gone:?}") });
                }
                ended.push(code.clone());
            }
            keep
        });
        for room in rooms.values_mut() {
            room.resumes
                .retain(|_, r| r.left_at.is_none_or(|t| now.duration_since(t) < RESUME_TTL));
        }
        // 방이 남지 않았고 한도도 다시 다 찬 주최자만 지운다(방을 지웠다 다시 만들어 깎인 한도를 되살리지 못하게).
        self.host_textless.lock().unwrap().retain(|host, bucket| {
            bucket.ready(now);
            bucket.tokens < bucket.burst || rooms.values().any(|r| &r.host_account == host)
        });
        ended
    }
}

fn broadcast(members: &HashMap<PeerId, Member>, except: &str, msg: ServerMsg) {
    for (id, member) in members {
        if id != except {
            send(&member.tx, msg.clone());
        }
    }
}

pub fn send(tx: &Outbox, msg: ServerMsg) {
    tx.send(msg);
}

/// 재입장 토큰 모양(`issue_resume`이 만드는 24자). 다르면 찾아보지도 않는다.
pub fn is_valid_resume(token: &str) -> bool {
    token.len() == RESUME_LEN && token.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

const RESUME_LEN: usize = 24;

fn random_token(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// `abcd-efgh-ijkm` 형식의 새 회의 코드.
fn new_room_code() -> String {
    let t = random_token(12);
    format!("{}-{}-{}", &t[0..4], &t[4..8], &t[8..12])
}

pub fn is_valid_room_code(code: &str) -> bool {
    let parts: Vec<&str> = code.split('-').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| p.len() == 4 && p.bytes().all(|b| CODE_ALPHABET.contains(&b)))
}

/// 이름은 앞뒤 공백 제거 후 1~40자, 제어 문자 불가.
pub fn normalize_name(name: &str) -> Option<String> {
    if name.chars().any(char::is_control) {
        return None;
    }
    // 보이지 않는 방향·줄 제어 문자는 다른 참가자 화면의 이름을 속일 수 있어 지운다
    // (오른쪽에서 왼쪽으로 쓰는 입력기나 붙여 넣은 글에 흔히 섞여 있으므로 거절하지 않는다).
    let visible: String = name.chars().filter(|&c| !crate::account::invisible(c)).collect();
    let name = visible.trim();
    let len = name.chars().count();
    (1..=40).contains(&len).then(|| name.to_owned())
}

/// BCP-47 형태만 확인한다(`ko`, `ja`, `en-US`, `zh-Hant`). 지원 여부는 번역 단계에서 판단.
pub fn is_valid_lang(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let primary_ok = parts
        .next()
        .is_some_and(|p| (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()));
    primary_ok
        && tag.len() <= 35
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "alice";
    const ON: RoomSettings = RoomSettings { translation: true };

    fn req(name: &str) -> JoinRequest {
        JoinRequest {
            name: name.into(),
            lang: "ko".into(),
            partner_lang: "ja".into(),
            media: MediaState::default(),
            resume: None,
        }
    }

    struct Client {
        tx: Outbox,
        rx: mpsc::Receiver<ServerMsg>,
    }

    fn client() -> Client {
        let (tx, rx) = mpsc::channel(32);
        Client {
            tx: Outbox::new(tx, Arc::new(Notify::new())),
            rx,
        }
    }

    impl Client {
        fn drain(&mut self) -> Vec<ServerMsg> {
            std::iter::from_fn(|| self.rx.try_recv().ok()).collect()
        }
    }

    fn joined(outcome: JoinOutcome) -> PeerId {
        match outcome {
            JoinOutcome::Joined(id) => id,
            other => panic!("expected joined, got {other:?}"),
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn billed(rooms: &Rooms, at: Instant) -> i64 {
        rooms.meter(at).iter().map(|m| m.seconds).sum()
    }

    #[test]
    fn one_person_is_metered_only_after_speaking_not_for_stt_tokens() {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let host = client();
        let id = joined(rooms.join(&code, req("Host"), Some(HOST), host.tx.clone()).unwrap());
        let t0 = Instant::now();
        // 들어와 있기만 하거나, 말하기 전에 음성 인식 세션을 미리 열어도(토큰) 세지 않는다.
        assert!(rooms.take_stt_token(&code, &id, t0 + secs(1)));
        assert_eq!(billed(&rooms, t0 + secs(60)), 0);
        // 말하면(음성 인식 글자가 도착) 그때부터 SPEECH_ACTIVE까지만 센다. 쉬던 시간은 세지 않는다.
        assert!(rooms.speech_audience(&code, &id, t0 + secs(61)).is_some());
        assert_eq!(billed(&rooms, t0 + secs(120)), 59);
        assert_eq!(billed(&rooms, t0 + secs(300)), 61 + SPEECH_ACTIVE.as_secs() as i64 - 120);
        assert_eq!(billed(&rooms, t0 + secs(400)), 0);
    }

    /// `from`초부터 `to`초까지 1초마다 `ids`가 모두 말한 글자를 보낸다.
    fn speak(rooms: &Rooms, code: &str, ids: &[&PeerId], t0: Instant, from: u64, to: u64) {
        for t in from..=to {
            for id in ids {
                assert!(rooms.speech_audience(code, id, t0 + secs(t)).is_some());
            }
        }
    }

    /// 말하는 중간(30초)과 다 끝난 뒤에 재서 더한다(정산이 측정 경계를 넘어도 같아야 한다).
    fn billed_after(rooms: &Rooms, t0: Instant, talk: impl Fn(u64, u64)) -> i64 {
        talk(0, 30);
        let mid = billed(rooms, t0 + secs(30));
        talk(31, 200);
        mid + billed(rooms, t0 + secs(1000))
    }

    #[test]
    fn concurrent_speakers_are_billed_per_speaker_but_turns_are_not() {
        let tail = SPEECH_ACTIVE.as_secs() as i64;
        let (rooms, code, host_id, _host, _, _guest, _) = room_with_admitted_guest();
        let t0 = Instant::now();
        let alone = billed_after(&rooms, t0, |a, b| {
            if a <= 60 {
                speak(&rooms, &code, &[&host_id], t0, a, b.min(60));
            }
        });
        assert_eq!(alone, 60 + tail, "one person for a minute: a minute");

        let (rooms, code, host_id, _host, guest_id, _guest, _) = room_with_admitted_guest();
        let t0 = Instant::now();
        let together = billed_after(&rooms, t0, |a, b| {
            if a <= 60 {
                speak(&rooms, &code, &[&host_id, &guest_id], t0, a, b.min(60));
            }
        });
        assert_eq!(together, 120 + tail, "two people at once for a minute: two minutes");

        // 주최자가 1분 말하고 게스트가 바로 이어서 1분 말한다. 주최자의 확정 글자가 2.6초 늦게 와서 겹쳐도 세지 않는다.
        let (rooms, code, host_id, _host, guest_id, _guest, _) = room_with_admitted_guest();
        let t0 = Instant::now();
        let turns = billed_after(&rooms, t0, |a, b| {
            for t in a..=b.min(120) {
                if t <= 60 {
                    speak(&rooms, &code, &[&host_id], t0, t, t);
                }
                if t == 63 {
                    let late_final = t0 + Duration::from_millis(62_600);
                    assert!(rooms.speech_audience(&code, &host_id, late_final).is_some());
                }
                if t >= 60 {
                    speak(&rooms, &code, &[&guest_id], t0, t, t);
                }
            }
        });
        assert_eq!(turns, 120 + tail, "a minute each in turn: two minutes");
    }

    #[test]
    fn silent_rooms_are_not_billed_whatever_their_size() {
        // 주최자 + 게스트가 말없이 켜 두기만 하면 세지 않고, 누가 말하면 그 뒤 SPEECH_ACTIVE까지만 센다.
        let (rooms, code, _host_id, _host, guest_id, _guest, _) = room_with_admitted_guest();
        let t0 = Instant::now();
        assert_eq!(billed(&rooms, t0 + secs(600)), 0, "nobody speaks: not billed");
        assert!(rooms.speech_audience(&code, &guest_id, t0 + secs(600)).is_some());
        assert_eq!(billed(&rooms, t0 + secs(660)), 60);
        assert_eq!(billed(&rooms, t0 + secs(900)), SPEECH_ACTIVE.as_secs() as i64 - 60);

        // 대면 대화·자막 화면(주최자 연결 둘)도 같다.
        let (rooms, code, _host_id, _host) = room_with_host();
        let second = client();
        let side = joined(rooms.join(&code, req("Host 2"), Some(HOST), second.tx.clone()).unwrap());
        let t0 = Instant::now();
        assert_eq!(billed(&rooms, t0 + secs(120)), 0);
        assert!(rooms.speech_audience(&code, &side, t0 + secs(120)).is_some());
        assert_eq!(billed(&rooms, t0 + secs(180)), 60);
    }

    #[test]
    fn guests_cannot_spend_the_hosts_time_after_the_host_leaves() {
        let (rooms, code, host_id, _host, guest_id, mut guest, _) = room_with_admitted_guest();
        rooms.leave(&code, &host_id);
        guest.drain();
        let t0 = Instant::now();
        let min = |m: u64| t0 + secs(m * 60);
        // 주최자가 나간 뒤 HOST_AWAY까지는 그대로 쓴다.
        assert!(rooms.speech_audience(&code, &guest_id, min(5)).is_some());
        assert_eq!(billed(&rooms, min(6)), 60);
        assert_eq!(rooms.stt_blocked(&code, min(9)), None);
        assert!(rooms.speech_audience(&code, &guest_id, min(9) + secs(30)).is_some());
        // 그 뒤로는 세지 않고(앞 발화의 남은 30초 + 나간 뒤 10분까지의 30초), 토큰·자막을 주지 않으며, 남은 사람에게 한 번 알린다.
        assert!((59..=60).contains(&billed(&rooms, min(11))));
        assert_eq!(billed(&rooms, min(12)), 0);
        assert_eq!(rooms.stt_blocked(&code, min(11)), Some(NoticeCode::HostAway));
        assert!(rooms.speech_audience(&code, &guest_id, min(11)).is_none());
        let told = guest.drain();
        assert_eq!(told.iter().filter(|m| matches!(m, ServerMsg::Notice { code: NoticeCode::HostAway })).count(), 1);
        // 주최자가 돌아오면 다시 시작한다.
        let back = client();
        joined(rooms.join(&code, req("Host"), Some(HOST), back.tx.clone()).unwrap());
        assert!(guest.drain().iter().any(|m| matches!(m, ServerMsg::Notice { code: NoticeCode::HostBack })));
        assert_eq!(rooms.stt_blocked(&code, min(13)), None);
        assert!(rooms.speech_audience(&code, &guest_id, min(13)).is_some());
    }

    #[test]
    fn switching_out_of_time_bills_only_the_unblocked_part() {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let host = client();
        let id = joined(rooms.join(&code, req("Host"), Some(HOST), host.tx.clone()).unwrap());
        let t0 = Instant::now();
        assert!(rooms.speech_audience(&code, &id, t0).is_some());
        // 10초 뒤 막히고 50초에 풀리면(추가 구매), 막혀 있던 40초는 세지 않는다.
        rooms.set_out_of_time_at(HashSet::from([HOST.to_owned()]), t0 + secs(10));
        rooms.set_out_of_time_at(HashSet::new(), t0 + secs(50));
        assert_eq!(billed(&rooms, t0 + secs(60)), 20);
    }

    #[test]
    fn stt_text_marks_use_only_for_members_of_translated_rooms() {
        let (rooms, code, _host_id, _host) = room_with_host();
        let g = client();
        let gid = waiting(rooms.join(&code, req("G"), None, g.tx).unwrap());
        let t0 = Instant::now();
        assert!(rooms.speech_audience(&code, &gid, t0).is_none(), "waiting guest");
        assert_eq!(billed(&rooms, t0 + secs(60)), 0);
    }

    #[test]
    fn captions_stop_when_host_is_out_of_time() {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let host = client();
        let id = joined(rooms.join(&code, req("Host"), Some(HOST), host.tx.clone()).unwrap());
        assert!(rooms.caption_audience(&code, &id).is_some());
        rooms.set_out_of_time(HashSet::from([HOST.to_owned()]));
        assert!(rooms.caption_audience(&code, &id).is_none());
        assert_eq!(rooms.stt_blocked(&code, Instant::now()), Some(NoticeCode::QuotaExceeded));
        // 다시 연결해 들어오면 입장 직후 지금 상태를 다시 받는다(놓친 알림 때문에 화면이 옛 상태에 머물지 않게).
        let mut again = client();
        joined(rooms.join(&code, req("Host"), Some(HOST), again.tx.clone()).unwrap());
        assert!(matches!(
            &again.drain()[..],
            [ServerMsg::Welcome { .. }, ServerMsg::Notice { code: NoticeCode::QuotaExceeded }]
        ));
    }

    fn waiting(outcome: JoinOutcome) -> PeerId {
        match outcome {
            JoinOutcome::Waiting(id) => id,
            other => panic!("expected waiting, got {other:?}"),
        }
    }

    /// 방 하나 + 주최자 입장까지 해 둔 상태
    fn room_with_host() -> (Rooms, String, PeerId, Client) {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let mut host = client();
        let host_id = joined(
            rooms
                .join(&code, req("Host"), Some(HOST), host.tx.clone())
                .unwrap(),
        );
        host.drain();
        (rooms, code, host_id, host)
    }

    #[test]
    fn joining_unknown_room_is_rejected() {
        let rooms = Rooms::default();
        let c = client();
        assert_eq!(
            rooms
                .join("abcd-efgh-ijkm", req("A"), Some(HOST), c.tx)
                .unwrap_err(),
            ErrorCode::RoomNotFound
        );
    }

    #[test]
    fn host_account_joins_immediately_and_gets_settings() {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let mut host = client();
        joined(
            rooms
                .join(&code, req("Host"), Some(HOST), host.tx.clone())
                .unwrap(),
        );
        assert!(
            matches!(&host.drain()[..], [ServerMsg::Welcome { host: true, settings, .. }] if settings.translation)
        );
    }

    #[test]
    fn guest_and_other_accounts_wait_and_host_is_notified() {
        let (rooms, code, _host_id, mut host) = room_with_host();

        let mut guest = client();
        let guest_id = waiting(
            rooms
                .join(&code, req("Guest"), None, guest.tx.clone())
                .unwrap(),
        );
        assert!(matches!(&guest.drain()[..], [ServerMsg::Waiting]));
        assert!(matches!(&host.drain()[..], [ServerMsg::Knock { knock }] if knock.id == guest_id));

        // 로그인했더라도 다른 주최자 계정이면 이 방에서는 게스트다.
        let other = client();
        waiting(
            rooms
                .join(&code, req("Bob"), Some("bob"), other.tx)
                .unwrap(),
        );
    }

    #[test]
    fn turn_credentials_go_only_to_admitted_participants() {
        let ice = Ice::new(
            Some("[]"),
            Some("turn:t.example:3478"),
            Some("0123456789abcdef0123456789abcdef"),
        )
        .unwrap();
        let rooms = Rooms::new(ice);
        let code = rooms.create(HOST, ON).unwrap();
        let turn_user = |msgs: Vec<ServerMsg>| match &msgs[..] {
            [ServerMsg::Welcome { ice_servers, .. }] => ice_servers[0].username.clone().unwrap(),
            other => panic!("expected welcome, got {other:?}"),
        };

        let mut host = client();
        let host_id = joined(
            rooms
                .join(&code, req("Host"), Some(HOST), host.tx.clone())
                .unwrap(),
        );
        assert!(turn_user(host.drain()).ends_with(&format!(":{host_id}")));

        let mut guest = client();
        let guest_id = waiting(
            rooms
                .join(&code, req("Guest"), None, guest.tx.clone())
                .unwrap(),
        );
        assert!(matches!(&guest.drain()[..], [ServerMsg::Waiting]));
        rooms.admit(&code, &host_id, &guest_id).unwrap();
        assert!(turn_user(guest.drain()).ends_with(&format!(":{guest_id}")));
    }

    fn resume_req(name: &str, token: &str) -> JoinRequest {
        JoinRequest {
            resume: Some(token.into()),
            ..req(name)
        }
    }

    /// 주최자 방 + 수락된 게스트. (rooms, code, host_id, host, guest_id, guest, 게스트의 재입장 토큰)
    fn room_with_admitted_guest() -> (Rooms, String, PeerId, Client, PeerId, Client, String) {
        let (rooms, code, host_id, mut host) = room_with_host();
        let mut guest = client();
        let guest_id = waiting(rooms.join(&code, req("Guest"), None, guest.tx.clone()).unwrap());
        rooms.admit(&code, &host_id, &guest_id).unwrap();
        let token = match &guest.drain()[..] {
            [ServerMsg::Waiting, ServerMsg::Welcome { resume, .. }] => resume.clone(),
            other => panic!("expected welcome, got {other:?}"),
        };
        host.drain();
        (rooms, code, host_id, host, guest_id, guest, token)
    }

    #[test]
    fn admitted_guest_comes_back_without_admission_once() {
        let (rooms, code, _, mut host, guest_id, _, token) = room_with_admitted_guest();
        rooms.leave(&code, &guest_id);
        host.drain();

        let mut back = client();
        let new_id = joined(rooms.join(&code, resume_req("CEO", &token), None, back.tx.clone()).unwrap());
        assert!(matches!(&back.drain()[..], [ServerMsg::Welcome { host: false, .. }]));
        let rejoined = host.drain();
        assert!(
            rejoined.iter().any(|m| matches!(m, ServerMsg::PeerJoined { peer } if peer.name == "Guest")),
            "keeps the name the host admitted"
        );
        assert!(rejoined.iter().any(|m| matches!(m, ServerMsg::PeerJoined { peer } if peer.id == new_id)));

        // 1회용: 같은 토큰으로는 다시 대기열로 간다.
        rooms.leave(&code, &new_id);
        let again = client();
        waiting(rooms.join(&code, resume_req("Guest", &token), None, again.tx).unwrap());
    }

    #[test]
    fn stt_token_limit_survives_reconnecting_with_a_resume_token() {
        let (rooms, code, _, _, guest_id, _old, token) = room_with_admitted_guest();
        let now = Instant::now();
        for _ in 0..STT_TOKEN_BURST as usize {
            assert!(rooms.take_stt_token(&code, &guest_id, now));
        }
        assert!(!rooms.take_stt_token(&code, &guest_id, now), "burst used up");

        // 정상적으로 나갔다가 토큰으로 돌아와도 한도가 다시 차지 않는다.
        rooms.leave(&code, &guest_id);
        let mut back = client();
        let id2 = joined(rooms.join(&code, resume_req("Guest", &token), None, back.tx.clone()).unwrap());
        assert!(!rooms.take_stt_token(&code, &id2, now), "not refilled by leave + resume");
        let token2 = match &back.drain()[..] {
            [ServerMsg::Welcome { resume, .. }] => resume.clone(),
            other => panic!("expected welcome, got {other:?}"),
        };

        // 옛 연결을 밀어내며 돌아와도 마찬가지다(반쯤 열린 연결).
        let again = client();
        let id3 = joined(rooms.join(&code, resume_req("Guest", &token2), None, again.tx).unwrap());
        assert!(!rooms.take_stt_token(&code, &id3, now), "not refilled by evicting a half-open connection");

        // 시간이 지나면 정상적으로 다시 찬다(말이 없는 동안은 3분에 하나).
        assert!(rooms.take_stt_token(&code, &id3, now + Duration::from_secs(181)));
        assert!(!rooms.take_stt_token(&code, "nobody", now), "only members get tokens");
    }

    #[test]
    fn resume_expires_and_wrong_tokens_wait_for_admission() {
        let (rooms, code, _, _, guest_id, _, token) = room_with_admitted_guest();
        let wrong = client();
        waiting(rooms.join(&code, resume_req("Mallory", "not-a-token"), None, wrong.tx).unwrap());

        rooms.leave(&code, &guest_id);
        rooms.sweep(Instant::now() + RESUME_TTL + Duration::from_secs(1));
        let late = client();
        waiting(rooms.join(&code, resume_req("Guest", &token), None, late.tx).unwrap());
    }

    #[test]
    fn resume_replaces_a_half_open_old_connection() {
        let (rooms, code, host_id, mut host, guest_id, _old, token) = room_with_admitted_guest();
        // 서버는 옛 연결이 끊긴 걸 아직 모른다(leave 없음).
        let mut back = client();
        let new_id = joined(rooms.join(&code, resume_req("Guest", &token), None, back.tx.clone()).unwrap());
        back.drain();
        let msgs = host.drain();
        assert!(msgs.iter().any(|m| matches!(m, ServerMsg::PeerLeft { id } if *id == guest_id)));
        assert!(msgs.iter().any(|m| matches!(m, ServerMsg::PeerJoined { peer } if peer.id == new_id)));
        // 옛 연결이 나중에 정리돼도 새 참가자에는 영향이 없다.
        rooms.leave(&code, &guest_id);
        assert!(!host.drain().iter().any(|m| matches!(m, ServerMsg::PeerLeft { id } if *id == new_id)));
        assert!(rooms.relay_signal(&code, &host_id, &new_id, SignalData::candidate("c")).is_ok());
    }

    #[test]
    fn host_admits_guest() {
        let (rooms, code, host_id, mut host) = room_with_host();
        let mut guest = client();
        let guest_id = waiting(
            rooms
                .join(&code, req("Guest"), None, guest.tx.clone())
                .unwrap(),
        );
        guest.drain();
        host.drain();

        rooms.admit(&code, &host_id, &guest_id).unwrap();
        let msgs = guest.drain();
        assert!(
            matches!(&msgs[..], [ServerMsg::Welcome { host: false, peers, .. }] if peers.len() == 1)
        );
        let host_msgs = host.drain();
        assert!(
            host_msgs
                .iter()
                .any(|m| matches!(m, ServerMsg::PeerJoined { peer } if peer.id == guest_id))
        );
        assert!(
            host_msgs
                .iter()
                .any(|m| matches!(m, ServerMsg::KnockResolved { id } if *id == guest_id))
        );

        // 입장 후에는 시그널을 주고받을 수 있다.
        rooms
            .relay_signal(&code, &guest_id, &host_id, SignalData::candidate("c"))
            .unwrap();
    }

    #[test]
    fn host_denies_guest() {
        let (rooms, code, host_id, _host) = room_with_host();
        let mut guest = client();
        let guest_id = waiting(
            rooms
                .join(&code, req("Guest"), None, guest.tx.clone())
                .unwrap(),
        );
        guest.drain();

        rooms.deny(&code, &host_id, &guest_id).unwrap();
        assert!(matches!(&guest.drain()[..], [ServerMsg::Denied]));
        assert_eq!(
            rooms.admit(&code, &host_id, &guest_id).unwrap_err(),
            ErrorCode::UnknownPeer
        );
    }

    #[test]
    fn non_hosts_cannot_admit_deny_or_change_settings() {
        let (rooms, code, host_id, _host) = room_with_host();
        let g1 = client();
        let g1_id = waiting(rooms.join(&code, req("G1"), None, g1.tx).unwrap());
        rooms.admit(&code, &host_id, &g1_id).unwrap();
        let g2 = client();
        let g2_id = waiting(rooms.join(&code, req("G2"), None, g2.tx).unwrap());

        // 입장한 게스트도, 대기 중인 게스트도 주최자 권한이 없다.
        for by in [&g1_id, &g2_id] {
            assert_eq!(
                rooms.admit(&code, by, &g2_id).unwrap_err(),
                ErrorCode::Forbidden
            );
            assert_eq!(
                rooms.deny(&code, by, &g2_id).unwrap_err(),
                ErrorCode::Forbidden
            );
            assert_eq!(
                rooms
                    .set_settings(&code, by, RoomSettings { translation: false })
                    .unwrap_err(),
                ErrorCode::Forbidden
            );
        }
        assert!(rooms.settings(&code).unwrap().translation);
    }

    #[test]
    fn waiting_guest_cannot_signal() {
        let (rooms, code, host_id, _host) = room_with_host();
        let g = client();
        let g_id = waiting(rooms.join(&code, req("G"), None, g.tx).unwrap());
        assert_eq!(
            rooms
                .relay_signal(&code, &g_id, &host_id, SignalData::candidate("c"))
                .unwrap_err(),
            ErrorCode::UnknownPeer
        );
        assert_eq!(
            rooms
                .relay_signal(&code, &host_id, &g_id, SignalData::candidate("c"))
                .unwrap_err(),
            ErrorCode::UnknownPeer
        );
    }

    #[test]
    fn host_changes_settings_and_everyone_is_told() {
        let (rooms, code, host_id, mut host) = room_with_host();
        let mut g = client();
        let g_id = waiting(rooms.join(&code, req("G"), None, g.tx.clone()).unwrap());
        rooms.admit(&code, &host_id, &g_id).unwrap();
        g.drain();
        host.drain();

        let off = RoomSettings { translation: false };
        rooms.set_settings(&code, &host_id, off).unwrap();
        assert!(
            matches!(&g.drain()[..], [ServerMsg::Settings { settings }] if !settings.translation)
        );
        assert!(matches!(&host.drain()[..], [ServerMsg::Settings { .. }]));
        assert_eq!(rooms.settings(&code), Some(off));
    }

    #[test]
    fn guest_leaving_queue_notifies_host() {
        let (rooms, code, _host_id, mut host) = room_with_host();
        let g = client();
        let g_id = waiting(rooms.join(&code, req("G"), None, g.tx).unwrap());
        host.drain();
        rooms.leave(&code, &g_id);
        assert!(matches!(&host.drain()[..], [ServerMsg::KnockResolved { id }] if *id == g_id));
    }

    #[test]
    fn knocks_waiting_before_host_arrives_are_listed_on_host_join() {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let g = client();
        let g_id = waiting(rooms.join(&code, req("Early"), None, g.tx).unwrap());

        let mut host = client();
        joined(
            rooms
                .join(&code, req("Host"), Some(HOST), host.tx.clone())
                .unwrap(),
        );
        assert!(
            matches!(&host.drain()[..], [ServerMsg::Welcome { knocks, .. }] if knocks[0].id == g_id)
        );
    }

    #[test]
    fn knocks_are_listed_in_arrival_order() {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let mut keep = Vec::new();
        let mut ids = Vec::new();
        for i in 0..MAX_WAITING {
            let c = client();
            ids.push(waiting(rooms.join(&code, req(&format!("G{i}")), None, c.tx.clone()).unwrap()));
            keep.push(c);
        }
        rooms.leave(&code, &ids[3]);
        ids.remove(3);

        // 주최자가 (다시) 들어오면 먼저 온 사람부터 보인다.
        let mut host = client();
        joined(rooms.join(&code, req("Host"), Some(HOST), host.tx.clone()).unwrap());
        match &host.drain()[..] {
            [ServerMsg::Welcome { knocks, .. }] => {
                assert_eq!(knocks.iter().map(|k| k.id.clone()).collect::<Vec<_>>(), ids);
            }
            other => panic!("expected welcome, got {other:?}"),
        }
    }

    #[test]
    fn waiting_guests_alone_do_not_keep_a_room_open() {
        let later = || Instant::now() + ROOM_IDLE_TTL + secs(1);
        // 주최자가 오지 않은 방
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let mut g = client();
        waiting(rooms.join(&code, req("Early"), None, g.tx.clone()).unwrap());
        g.drain();
        assert_eq!(rooms.sweep(later()), std::slice::from_ref(&code));
        assert!(!rooms.exists(&code));
        assert!(matches!(
            &g.drain()[..],
            [ServerMsg::Error { code: ErrorCode::RoomNotFound, .. }]
        ));

        // 주최자가 나가고 게스트만 기다리는 방
        let (rooms, code, host_id, _host) = room_with_host();
        let g = client();
        waiting(rooms.join(&code, req("Late"), None, g.tx).unwrap());
        rooms.leave(&code, &host_id);
        assert_eq!(rooms.sweep(later()), [code]);

        // 예약 회의는 늦는 주최자를 기다리는 동안 남기고, 기다리던 사람이 떠나면 지운다.
        let rooms = Rooms::default();
        rooms.open_reserved(HOST, "abcd-efgh-ijkm", ON);
        let g = client();
        let id = waiting(rooms.join("abcd-efgh-ijkm", req("Early"), None, g.tx).unwrap());
        assert!(rooms.sweep(later()).is_empty());
        rooms.leave("abcd-efgh-ijkm", &id);
        assert_eq!(rooms.sweep(later()), ["abcd-efgh-ijkm"]);
    }

    #[test]
    fn limits_room_size_queue_and_rooms_per_host() {
        let (rooms, code, host_id, _host) = room_with_host();
        let mut keep = Vec::new();
        for i in 1..MAX_PEERS {
            let c = client();
            let id = waiting(
                rooms
                    .join(&code, req(&i.to_string()), None, c.tx.clone())
                    .unwrap(),
            );
            rooms.admit(&code, &host_id, &id).unwrap();
            keep.push(c);
        }
        let c = client();
        assert_eq!(
            rooms.join(&code, req("late"), None, c.tx).unwrap_err(),
            ErrorCode::RoomFull
        );

        let (rooms, code, _host_id, _host) = room_with_host();
        for i in 0..MAX_WAITING {
            let c = client();
            waiting(
                rooms
                    .join(&code, req(&i.to_string()), None, c.tx.clone())
                    .unwrap(),
            );
            keep.push(c);
        }
        let c = client();
        assert_eq!(
            rooms.join(&code, req("spam"), None, c.tx).unwrap_err(),
            ErrorCode::TooManyWaiting
        );

        let rooms = Rooms::default();
        // 손님이 미리 열어 본 예약 회의 방은 주최자의 방 수에 들지 않는다.
        for i in 0..MAX_ROOMS_PER_HOST {
            rooms.open_reserved(HOST, &format!("rsvd-{i:04}-aaaa"), ON);
        }
        for _ in 0..MAX_ROOMS_PER_HOST {
            rooms.create(HOST, ON).unwrap();
        }
        assert_eq!(rooms.create(HOST, ON).unwrap_err(), ErrorCode::Forbidden);
    }

    #[test]
    fn silent_stt_tokens_are_also_capped_per_host() {
        // 방과 참가자를 늘려도 한 주최자가 말 없이 받는 토큰은 주최자 한도까지만.
        let rooms = Rooms::default();
        let now = Instant::now();
        let mut keep = Vec::new();
        let mut issued = 0;
        for _ in 0..5 {
            let code = rooms.create(HOST, ON).unwrap();
            let host = client();
            let host_id = joined(rooms.join(&code, req("Host"), Some(HOST), host.tx.clone()).unwrap());
            let mut ids = vec![host_id.clone()];
            for i in 1..MAX_PEERS {
                let c = client();
                let id = waiting(rooms.join(&code, req(&i.to_string()), None, c.tx.clone()).unwrap());
                rooms.admit(&code, &host_id, &id).unwrap();
                ids.push(id);
                keep.push(c);
            }
            keep.push(host);
            for id in &ids {
                for _ in 0..10 {
                    issued += usize::from(rooms.take_stt_token(&code, id, now));
                }
            }
        }
        assert_eq!(issued, HOST_TEXTLESS_TOKEN_BURST as usize);
        // 방을 모두 닫고 정리해도 깎인 한도는 그대로다(다시 만들어 새 한도를 받지 못한다).
        let codes: Vec<String> = rooms.rooms.lock().unwrap().keys().cloned().collect();
        for code in codes {
            rooms.close(&code);
        }
        rooms.sweep(now);
        let code = rooms.create(HOST, ON).unwrap();
        let host = client();
        let host_id = joined(rooms.join(&code, req("Host"), Some(HOST), host.tx.clone()).unwrap());
        assert!(!rooms.take_stt_token(&code, &host_id, now));
    }

    #[test]
    fn closing_a_deleted_meeting_tells_everyone_and_forgets_the_room() {
        let (rooms, code, _host_id, mut host) = room_with_host();
        let mut g = client();
        waiting(rooms.join(&code, req("Early"), None, g.tx.clone()).unwrap());
        host.drain();
        g.drain();
        rooms.close(&code);
        assert!(!rooms.exists(&code));
        for c in [&mut host, &mut g] {
            assert!(matches!(
                &c.drain()[..],
                [ServerMsg::Error { code: ErrorCode::RoomNotFound, .. }]
            ));
        }
    }

    #[test]
    fn idle_rooms_are_swept_but_occupied_rooms_stay() {
        let (rooms, busy, _host_id, _host) = room_with_host();
        let idle = rooms.create(HOST, ON).unwrap();
        rooms.sweep(Instant::now() + ROOM_IDLE_TTL + Duration::from_secs(1));
        assert!(rooms.settings(&idle).is_none());
        assert!(rooms.settings(&busy).is_some());
    }

    #[test]
    fn validates_inputs() {
        assert!(is_valid_room_code(&new_room_code()));
        assert!(!is_valid_room_code("abcd-efgh"));
        assert!(!is_valid_room_code("ABCD-efgh-ijkm"));
        assert_eq!(normalize_name("  유승재 ").as_deref(), Some("유승재"));
        assert!(normalize_name("   ").is_none());
        assert!(normalize_name(&"가".repeat(41)).is_none());
        assert_eq!(normalize_name("\u{200F}Mina\u{200B}").as_deref(), Some("Mina"));
        assert!(normalize_name("\u{202E}\u{3164}").is_none());
        assert!(is_valid_lang("ko") && is_valid_lang("zh-Hant") && is_valid_lang("en-US"));
        assert!(!is_valid_lang("") && !is_valid_lang("k") && !is_valid_lang("ko_KR"));
    }

    #[test]
    fn host_back_follows_host_away_however_it_was_told() {
        // 주기 작업보다 먼저 토큰 요청에서 주최자가 없다고 알렸어도, 돌아오면 HostBack을 보낸다.
        let (rooms, code, host_id, _host, _guest_id, mut guest, _) = room_with_admitted_guest();
        rooms.leave(&code, &host_id);
        guest.drain();
        let t0 = Instant::now();
        assert_eq!(rooms.stt_blocked(&code, t0 + HOST_AWAY + secs(1)), Some(NoticeCode::HostAway));
        assert!(guest.drain().iter().any(|m| matches!(m, ServerMsg::Notice { code: NoticeCode::HostAway })));
        let back = client();
        joined(rooms.join(&code, req("Host"), Some(HOST), back.tx.clone()).unwrap());
        assert!(guest.drain().iter().any(|m| matches!(m, ServerMsg::Notice { code: NoticeCode::HostBack })));
    }

    #[test]
    fn tokens_without_speech_are_limited_to_the_silent_prewarm_rate() {
        let (rooms, code, _host_id, _host, guest_id, _guest, _) = room_with_admitted_guest();
        let t0 = Instant::now();
        // 정직한 화면: 말이 없어도 세션을 미리 열어 두고 3분마다 바꾼다 → 한 시간 동안 모두 받는다(세지도 않는다).
        let honest = (0..=20u64).filter(|i| rooms.take_stt_token(&code, &guest_id, t0 + secs(i * 181))).count();
        assert_eq!(honest, 21);
        assert_eq!(billed(&rooms, t0 + secs(3700)), 0);

        // 고친 화면: 30초마다 토큰만 받고 글자는 보내지 않는다 → 처음 몫 + 3분에 하나만 받는다.
        let (rooms, code, _host_id, _host, guest_id, _guest, _) = room_with_admitted_guest();
        let greedy = (0..120u64).filter(|i| rooms.take_stt_token(&code, &guest_id, t0 + secs(i * 30))).count();
        assert!(greedy <= TEXTLESS_TOKEN_BURST as usize + 20, "{greedy} tokens without speech in an hour");
        // 말한 글자가 오면 일반 한도로 돌아간다(말하는 사람은 막히지 않는다).
        assert!(rooms.speech_audience(&code, &guest_id, t0 + secs(3600)).is_some());
        assert!(rooms.take_stt_token(&code, &guest_id, t0 + secs(3631)));
    }

    #[test]
    fn an_honest_silent_listener_reconnects_at_once_after_a_few_blips() {
        let (rooms, code, _host_id, _host, guest_id, _guest, _) = room_with_admitted_guest();
        let t0 = Instant::now();
        let at = |t: u64| t0 + secs(t);
        assert!(rooms.take_stt_token(&code, &guest_id, at(0)));
        // 연결이 세 번 끊겨 40초마다 다시 받고, 그 뒤 한 시간 동안 3분마다 바꾼다.
        for t in [40, 80, 120] {
            assert!(rooms.take_stt_token(&code, &guest_id, at(t)), "blip at {t}");
        }
        let mut t = 120;
        for _ in 0..20 {
            t += 181;
            assert!(rooms.take_stt_token(&code, &guest_id, at(t)), "rotation at {t}");
        }
        // 지금 세션이 끊기면 처음 다시 요청(1초 뒤)에 바로 받는다.
        assert!(rooms.take_stt_token(&code, &guest_id, at(t + 1)));
    }

    #[test]
    fn refused_or_failed_token_requests_do_not_use_the_limit() {
        let (rooms, code, _host_id, _host, guest_id, _guest, _) = room_with_admitted_guest();
        let now = Instant::now();
        for _ in 0..STT_TOKEN_BURST as usize {
            assert!(rooms.take_stt_token(&code, &guest_id, now));
        }
        // 일반 한도에서 거절된 요청은 말 없는 한도를 쓰지 않는다: 일반 한도가 다시 차면 남은 말 없는 몫으로 받는다.
        for _ in 0..10 {
            assert!(!rooms.take_stt_token(&code, &guest_id, now));
        }
        let later = now + secs(60);
        assert!(rooms.take_stt_token(&code, &guest_id, later));
        assert!(rooms.take_stt_token(&code, &guest_id, later));
        // 발급에 실패해 돌려준 토큰은 다시 쓸 수 있다. 다만 돌려받는 횟수는 묶여, 실패를 일으켜 가며 계속 받지는 못한다.
        assert!(!rooms.take_stt_token(&code, &guest_id, later));
        rooms.give_back_stt_token(&code, &guest_id, later);
        assert!(rooms.take_stt_token(&code, &guest_id, later));
        rooms.give_back_stt_token(&code, &guest_id, later);
        assert!(rooms.take_stt_token(&code, &guest_id, later));
        rooms.give_back_stt_token(&code, &guest_id, later);
        assert!(!rooms.take_stt_token(&code, &guest_id, later), "only two refunds per ten minutes");

        // 말 없는 한도에서 거절된 요청도 일반 한도를 쓰지 않는다: 말하면 바로 일반 한도만큼 받는다.
        let (rooms, code, _host_id, _host, guest_id, _guest, _) = room_with_admitted_guest();
        let t = now + secs(600);
        let silent = (0..10).filter(|_| rooms.take_stt_token(&code, &guest_id, t)).count();
        assert_eq!(silent, STT_TOKEN_BURST as usize);
        let t = t + secs(120);
        while rooms.take_stt_token(&code, &guest_id, t) {}
        for _ in 0..5 {
            assert!(!rooms.take_stt_token(&code, &guest_id, t));
        }
        assert!(rooms.speech_audience(&code, &guest_id, t).is_some());
        assert!(rooms.take_stt_token(&code, &guest_id, t), "the general limit was not used up by refusals");
    }

    #[test]
    fn honest_clients_recover_right_after_a_long_upstream_outage() {
        // Gemini가 오래 실패해도(발급 실패는 세션을 열지 않는다) 끝나면 다음 재시도(최대 30초 간격)에 바로 받는다.
        for outage in [300u64, 600, 900] {
            let t0 = Instant::now();
            let mut limits = new_stt_tokens();
            let (mut t, mut failures) = (0u64, 0usize);
            let recovered = loop {
                let now = t0 + secs(t);
                if limits.take(now) {
                    if t >= outage {
                        break t;
                    }
                    limits.give_back(now);
                }
                failures += 1;
                t += [1u64, 3, 10, 30][failures.min(4) - 1];
                assert!(t < 3000, "no token after a {outage} s outage");
            };
            assert!(recovered - outage <= 60, "{outage} s outage: first token {} s after it ended", recovered - outage);
        }
    }
}
