//! 음성 인식 텍스트 → 문장 버퍼 → 번역 → 자막 이벤트. WebRTC와 무관해서 대면 모드에서도 그대로 쓴다.
//!
//! 화자(=WS 연결) 하나에 워커 태스크 하나가 상태를 혼자 가진다. 잠금이 없고, 방 정보는 `Rooms`에서
//! 필요할 때마다 짧게 복사해 온다.
//!
//! - **문장 버퍼**: 확정된 STT 조각 중 아직 문장이 끝나지 않은 부분 + 현재 중간 결과를 한 줄로 잇는다.
//!   확정된 텍스트에서 문장 끝 부호가 나오면 그 문장을 확정한다. 확정 조각이 문장 끝 없이 끝나고
//!   `FLUSH_AFTER` 동안 말이 없으면 그대로 확정한다.
//! - **최신 우선**: 말하는 중인 문장은 목표 언어마다 동시에 1개만 번역한다. 그사이 바뀐 텍스트는 모아 두었다가
//!   끝나는 즉시 최신 것 하나만 번역한다(중간 버전은 건너뜀). 진행 중 요청을 매번 취소하면 중간 결과가
//!   0.5초마다 오고 번역은 약 1초 걸려서 말하는 동안 번역이 하나도 끝나지 않는다.
//!   문장이 확정되면 진행 중 요청을 취소하고 곧바로 확정 번역을 시작한다.
//! - **헤징**: `providers::translate_hedged`.
//! - **팬아웃**: 목표 언어마다 한 번 번역해 그 언어를 읽는 사람들에게 보낸다.

mod text;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;

use crate::protocol::{NoticeCode, PeerId, ServerMsg};
use crate::providers::{
    TranslateError, TranslateRequest, Translation, Translator, translate_hedged,
};
use crate::room::{self, Audience, CaptionEpoch, Rooms};

/// 이 시간 안에 첫 토큰이 없으면 같은 요청을 한 번 더 보낸다.
const HEDGE_AFTER: Duration = Duration::from_millis(1500);
/// 확정 조각이 문장 끝 없이 끝난 뒤 이만큼 조용하면 문장을 확정한다.
const FLUSH_AFTER: Duration = Duration::from_secs(2);
/// 화자가 나간 뒤에도 이미 말을 마친 문장의 번역은 이만큼 더 기다려 남은 사람에게 보낸다
/// ("감사합니다, 다음에 봬요" 하고 바로 나가는 경우). 넘기면 원문으로 확정한다.
const DRAIN_AFTER_LEAVE: Duration = Duration::from_secs(5);
/// STT 메시지 하나의 최대 길이(글자). 넘는 부분은 버린다(거부하지 않는다: 거부하면 그 구간의 확정이 오지 않는다).
pub const MAX_STT_CHARS: usize = 1000;
/// 문장 끝 부호 없이 이만큼 길어지면 강제로 확정한다(번역 프롬프트가 끝없이 커지지 않게).
const MAX_OPEN_CHARS: usize = 400;
/// 번역을 기다리는 문장 수 상한(번역이 꺼진 사이 쌓이는 것 방지)
const MAX_SENTENCES: usize = 16;
/// Bound fanout from punctuation-heavy STT frames while preserving every word.
const MAX_SENTENCES_PER_UPDATE: usize = 4;
/// 번역 문맥으로 넘길 직전 확정 문장 쌍
const CONTEXT_PAIRS: usize = 3;
const NOTICE_INTERVAL: Duration = Duration::from_secs(30);
const INBOX_CAPACITY: usize = 32;

pub struct Pipeline {
    rooms: Arc<Rooms>,
    translator: Option<Arc<dyn Translator>>,
}

impl Pipeline {
    /// `translator`가 없으면 자막은 원문만 확정되고 번역 실패 알림이 간다.
    pub fn new(rooms: Arc<Rooms>, translator: Option<Arc<dyn Translator>>) -> Self {
        Pipeline { rooms, translator }
    }

    /// 화자 한 명의 워커를 띄운다. 핸들을 버리면 말하던 중인 글자는 거두고, 이미 말을 마친 문장의 번역만
    /// `DRAIN_AFTER_LEAVE`까지 마저 보낸 뒤 워커와 진행 중인 번역 요청이 멈춘다.
    pub fn speaker(self: &Arc<Self>, room: String, speaker: PeerId) -> SpeakerHandle {
        let (tx, rx) = mpsc::channel(INBOX_CAPACITY);
        let (done_tx, done_rx) = mpsc::channel(INBOX_CAPACITY);
        let changes = self.rooms.caption_changes(&room);
        // 처음 본 독자 목록도 남겨 둔다: 첫 글자와 함께 바로 나가도 그 독자에게 마저 보낼 수 있게.
        let audience = self.rooms.caption_audience(&room, &speaker);
        let epoch = audience.as_ref().map(|a| a.epoch);
        let worker = Worker {
            last_audience: audience,
            epoch,
            pipeline: self.clone(),
            room: room.clone(),
            speaker: speaker.clone(),
            seg: 0,
            committed: String::new(),
            interim: String::new(),
            next_id: 0,
            open: None,
            sentences: BTreeMap::new(),
            context: HashMap::new(),
            done_tx,
            flush_at: None,
            last_notice: None,
            departed: false,
        };
        tokio::spawn(worker.run(rx, done_rx, changes));
        SpeakerHandle {
            tx,
            pipeline: self.clone(),
            room,
            speaker,
        }
    }
}

pub struct SpeakerHandle {
    pipeline: Arc<Pipeline>,
    room: String,
    speaker: PeerId,
    tx: mpsc::Sender<Stt>,
}

impl SpeakerHandle {
    pub fn push(&self, seg: u32, text: String, is_final: bool) {
        let text = text::truncate_chars(text, MAX_STT_CHARS);
        let rooms = &self.pipeline.rooms;
        let at = Instant::now();
        // Capture consent/language at receipt, before the worker can be delayed.
        // 글자가 있을 때만 실제로 말한 것으로 본다(빈 텍스트는 지우기·끝맺음). 한 사람만 있는 방의 번역 시간은 이것으로 잰다.
        let audience = if text.trim().is_empty() {
            rooms.caption_audience(&self.room, &self.speaker)
        } else {
            rooms.speech_audience(&self.room, &self.speaker, at)
        };
        let Some(audience) = audience else {
            return;
        };
        let stt = Stt {
            epoch: audience.epoch,
            seg,
            text,
            is_final,
            at,
        };
        // 워커가 밀리면 버린다. 중간 결과는 누적이라 다음 것이 앞의 것을 대신한다.
        if self.tx.try_send(stt).is_err() {
            tracing::warn!("dropping stt: speaker worker is behind");
        }
    }
}

struct Stt {
    epoch: CaptionEpoch,
    seg: u32,
    text: String,
    is_final: bool,
    at: Instant,
}

/// 번역 태스크가 워커에게 돌려주는 결과
struct Done {
    finished: bool,
    sentence: u64,
    lang: String,
    version: u32,
    complete: bool,
    stt_at: Instant,
    result: Result<Translation, TranslateError>,
}

struct Sentence {
    finalized_readers: BTreeSet<PeerId>,
    src: String,
    /// `src`가 바뀔 때마다 1씩 오른다(1부터).
    version: u32,
    locked: bool,
    rev: u32,
    /// 지금 `src`를 만든 STT 메시지가 도착한 시각(지연 측정용)
    stt_at: Instant,
    tr: HashMap<String, Tr>,
}

#[derive(Default)]
struct Tr {
    /// 가장 최근 번역. 원문이 더 바뀌었으면 이전 원문의 번역이다.
    text: Option<String>,
    /// `text`가 어느 원문 버전의 번역인지(0 = 없음)
    version: u32,
    /// 끝난(성공·실패) 요청 중 가장 높은 원문 버전. 같은 버전을 다시 요청하지 않는다.
    tried: u32,
    /// 확정 문장의 번역이 끝남(실패 포함)
    done: bool,
    inflight: Option<Inflight>,
}

struct Inflight {
    version: u32,
    complete: bool,
    task: AbortHandle,
}

impl Drop for Inflight {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Worker {
    last_audience: Option<Audience>,
    epoch: Option<CaptionEpoch>,
    pipeline: Arc<Pipeline>,
    room: String,
    speaker: PeerId,
    /// 지금 받고 있는 STT 구간 번호
    seg: u32,
    /// 확정됐지만 아직 문장이 끝나지 않은 텍스트
    committed: String,
    /// 현재 구간의 중간 결과(누적)
    interim: String,
    next_id: u64,
    /// 말하는 중인 문장
    open: Option<u64>,
    sentences: BTreeMap<u64, Sentence>,
    /// 목표 언어별 직전 확정 문장 쌍
    context: HashMap<String, VecDeque<(String, String)>>,
    done_tx: mpsc::Sender<Done>,
    flush_at: Option<tokio::time::Instant>,
    last_notice: Option<Instant>,
    /// 화자가 나갔다(연결이 끊겼거나 방에서 나감). 남은 확정 문장의 번역만 직전 독자에게 마저 보낸다.
    departed: bool,
}

impl Drop for Worker {
    fn drop(&mut self) {
        // 연결이 끝났다(화자가 나갔거나 끊김): 확정된 문장은 원문으로 남기고 말하던 중인 글자만 지운다.
        self.retract_pending(true);
    }
}

impl Worker {
    async fn run(
        mut self,
        mut rx: mpsc::Receiver<Stt>,
        mut done_rx: mpsc::Receiver<Done>,
        mut changes: Option<watch::Receiver<u64>>,
    ) {
        loop {
            let flush_at = self.flush_at;
            let flush = async move {
                match flush_at {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                changed = async {
                    match changes.as_mut() {
                        Some(rx) => rx.changed().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if changed.is_err() { break; }
                    self.sync_audience();
                    for id in self.sentences.keys().copied().collect::<Vec<_>>() { self.refresh(id, None); }
                },
                stt = rx.recv() => match stt {
                    Some(stt) => self.on_stt(stt),
                    None => self.depart(),
                },
                Some(done) = done_rx.recv() => self.on_done(done),
                () = flush => self.on_flush(),
            }
            if self.departed {
                self.finish_departure(&mut rx);
                self.drain(&mut done_rx, &mut changes).await;
                break;
            }
            // Give socket writers a turn even when STT/progress queues stay ready.
            tokio::task::yield_now().await;
        }
    }

    /// 화자가 나갔다. 정리는 지금 하던 처리가 끝난 뒤 `finish_departure`에서 한다.
    fn depart(&mut self) {
        self.departed = true;
    }

    /// 나간 화자의 남은 글자를 정리한다: 나가기 직전에 도착한 음성 인식 결과까지 반영하고,
    /// 마침표 없이 끝난 확정 글자도 문장으로 확정해 번역을 마저 보낸다. 말하던 중인 글자만 거둔다.
    fn finish_departure(&mut self, rx: &mut mpsc::Receiver<Stt>) {
        while let Ok(stt) = rx.try_recv() {
            self.on_stt(stt);
        }
        self.interim.clear();
        let committed = std::mem::take(&mut self.committed);
        if !committed.trim().is_empty() {
            self.lock(committed, Instant::now());
        }
        self.open = None;
        self.flush_at = None;
        let open: Vec<u64> = self.sentences.iter().filter(|(_, s)| !s.locked).map(|(id, _)| *id).collect();
        for id in open {
            let mut s = self.sentences.remove(&id).expect("exists");
            s.src.clear();
            if let Some(audience) = &self.last_audience {
                self.send_original(audience, id, &s);
            }
        }
    }

    /// 나간 화자의 확정 문장 번역을 `DRAIN_AFTER_LEAVE`까지 기다린다. 남은 것은 Drop이 원문으로 확정한다.
    async fn drain(
        &mut self,
        done_rx: &mut mpsc::Receiver<Done>,
        changes: &mut Option<watch::Receiver<u64>>,
    ) {
        let deadline = tokio::time::Instant::now() + DRAIN_AFTER_LEAVE;
        while !self.sentences.is_empty() {
            tokio::select! {
                Some(done) = done_rx.recv() => self.on_done(done),
                // 그사이 방이 없어졌으면(회의 삭제·정리) 더 보낼 곳이 없다. 다른 변화(Ok)는 넘기고 계속 기다린다.
                changed = async {
                    match changes.as_mut() {
                        Some(rx) => rx.changed().await,
                        None => std::future::pending().await,
                    }
                } => if changed.is_err() { break },
                () = tokio::time::sleep_until(deadline) => break,
            }
        }
    }

    fn sync_audience(&mut self) -> Option<Audience> {
        if self.departed {
            // 나간 뒤에는 방의 독자 목록을 다시 보지 않는다(화자가 없어 비어 있다). 나가기 직전 독자에게 보낸다.
            return self.last_audience.clone();
        }
        let audience = self
            .pipeline
            .rooms
            .caption_audience(&self.room, &self.speaker);
        let epoch = audience.as_ref().map(|a| a.epoch);
        if self.epoch != epoch {
            // 화자가 나갔으면 확정된 문장은 남긴다(다른 사람에게서 갑자기 사라지지 않게).
            // 번역을 끄거나 언어를 바꿨으면 기다리던 문장을 모두 거둔다.
            let speaker_left =
                audience.is_none() && !self.pipeline.rooms.is_member(&self.room, &self.speaker);
            if speaker_left {
                self.depart();
                return self.last_audience.clone();
            }
            self.committed.clear();
            self.interim.clear();
            self.open = None;
            self.flush_at = None;
            self.retract_pending(false);
            self.context.clear();
            self.epoch = epoch;
        }
        self.last_audience = audience.clone();
        audience
    }

    fn on_stt(&mut self, stt: Stt) {
        let Some(audience) = self.sync_audience() else {
            return;
        };
        if stt.epoch != audience.epoch {
            return;
        }
        if stt.seg < self.seg {
            return; // 이미 확정된 구간의 늦은 메시지
        }
        if stt.seg > self.seg {
            // 확정 결과를 못 받고 다음 구간으로 넘어갔다: 마지막 중간 결과를 확정으로 본다.
            let interim = std::mem::take(&mut self.interim);
            self.committed = text::join(&self.committed, &interim);
            self.seg = stt.seg;
        }
        let text = text::normalize(&stt.text);
        if stt.is_final {
            self.committed = text::join(&self.committed, &text);
            self.interim.clear();
            self.seg = stt.seg.saturating_add(1);
        } else {
            self.interim = text;
        }
        self.update(stt.at);
    }

    fn on_flush(&mut self) {
        if self.sync_audience().is_none() {
            return;
        }
        self.flush_at = None;
        if self.interim.is_empty() && !self.committed.is_empty() {
            let text = std::mem::take(&mut self.committed);
            self.lock(text, Instant::now());
        }
    }

    fn update(&mut self, at: Instant) {
        let (finished, rest) = text::split_sentences(&self.committed);
        self.committed = rest;
        let group_size = finished.len().div_ceil(MAX_SENTENCES_PER_UPDATE).max(1);
        for group in finished.chunks(group_size) {
            let sentence = group.iter().fold(String::new(), |a, b| text::join(&a, b));
            self.lock(sentence, at);
        }
        if self.committed.chars().count() > MAX_OPEN_CHARS {
            let text = std::mem::take(&mut self.committed);
            self.lock(text, at);
        }

        let open_text = text::join(&self.committed, &self.interim);
        if open_text.is_empty() {
            if let Some(id) = self.open.take()
                && let Some(mut sentence) = self.sentences.remove(&id)
            {
                sentence.src.clear();
                self.finish_original(id, sentence);
            }
        } else {
            let id = match self.open {
                Some(id) => id,
                None => {
                    let id = self.new_sentence(at);
                    self.open = Some(id);
                    id
                }
            };
            let s = self.sentences.get_mut(&id).expect("open sentence exists");
            if s.src != open_text {
                s.src = open_text;
                s.version += 1;
                s.stt_at = at;
                self.refresh(id, None);
            }
        }

        self.flush_at = (self.interim.is_empty() && !self.committed.is_empty())
            .then(|| tokio::time::Instant::now() + FLUSH_AFTER);
    }

    fn new_sentence(&mut self, at: Instant) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.sentences.insert(
            id,
            Sentence {
                finalized_readers: BTreeSet::new(),
                src: String::new(),
                version: 0,
                locked: false,
                rev: 0,
                stt_at: at,
                tr: HashMap::new(),
            },
        );
        while self.sentences.len() > MAX_SENTENCES {
            let (old_id, old) = self.sentences.pop_first().expect("not empty");
            self.finish_original(old_id, old);
        }
        id
    }

    /// 기다리던 문장을 모두 끝낸다. 말하던 중인 문장은 지우고(빈 원문), `keep_locked`면 확정된 문장은
    /// 번역 없이 원문으로 확정한다.
    fn retract_pending(&mut self, keep_locked: bool) {
        let pending = std::mem::take(&mut self.sentences);
        if let Some(audience) = &self.last_audience {
            for (id, mut sentence) in pending {
                if !(keep_locked && sentence.locked) {
                    sentence.src.clear();
                }
                self.send_original(audience, id, &sentence);
            }
        }
    }

    fn send_original(&self, audience: &Audience, id: u64, s: &Sentence) {
        for reader in &audience.readers {
            if s.finalized_readers.contains(&reader.id) {
                continue;
            }
            room::send(
                &reader.tx,
                ServerMsg::Caption {
                    seg: format!("{}-{id}", self.speaker),
                    speaker: self.speaker.clone(),
                    src_lang: audience.src_lang.clone(),
                    src: s.src.clone(),
                    tgt_lang: audience
                        .target_for(&reader.lang)
                        .unwrap_or(&audience.src_lang)
                        .to_owned(),
                    text: None,
                    is_final: true,
                    rev: s.rev + 1,
                },
            );
        }
    }

    fn finish_original(&self, id: u64, s: Sentence) {
        // 나간 뒤에는 방의 독자 목록이 비어 있으므로 나가기 직전 독자에게 보낸다(`sync_audience`와 같게).
        let audience = if self.departed {
            self.last_audience.clone()
        } else {
            self.pipeline.rooms.caption_audience(&self.room, &self.speaker)
        };
        if let Some(audience) = audience {
            self.send_original(&audience, id, &s);
        }
    }

    /// 문장을 확정한다. 말하는 중이던 문장이 있으면 그 줄을 이어서 쓴다.
    fn lock(&mut self, text: String, at: Instant) {
        let id = match self.open.take() {
            Some(id) if self.sentences.contains_key(&id) => id,
            _ => self.new_sentence(at),
        };
        let s = self.sentences.get_mut(&id).expect("exists");
        if s.src != text {
            s.src = text;
            s.version += 1;
            s.stt_at = at;
        }
        s.locked = true;
        self.refresh(id, None);
    }

    /// 필요한 번역을 시작하고, 참가자에게 이 문장의 현재 상태를 보낸다.
    /// `only`: 이 목표 언어의 번역만 바뀌었다. 그 언어를 읽는 사람에게만 보낸다(번역 조각마다 모두에게 다시 보내지 않게).
    fn refresh(&mut self, id: u64, only: Option<&str>) {
        let Some(audience) = self.sync_audience() else {
            return;
        };
        let targets: BTreeSet<String> = audience
            .readers
            .iter()
            .filter_map(|r| audience.target_for(&r.lang))
            .map(str::to_owned)
            .collect();
        self.context.retain(|lang, _| targets.contains(lang));
        if let Some(s) = self.sentences.get_mut(&id) {
            s.tr.retain(|lang, _| targets.contains(lang));
        }
        for lang in &targets {
            self.schedule(id, lang, &audience.src_lang);
        }

        let Some(s) = self.sentences.get_mut(&id) else {
            return;
        };
        s.rev += 1;
        for reader in &audience.readers {
            let tgt = audience.target_for(&reader.lang);
            let tr = tgt.and_then(|l| s.tr.get(l));
            let is_final = s.locked && (tgt.is_none() || tr.is_some_and(|t| t.done));
            // 다른 언어의 진행만 바뀌었으면 건너뛴다. 단, 이 독자의 확정 여부가 바뀌면(방금 언어를 바꿨는데
            // 문장이 곧 지워지는 경우 등) 보낸다: 안 보내면 그 줄이 끝내 "번역 중"으로 남는다.
            if only.is_some() && tgt != only && s.finalized_readers.contains(&reader.id) == is_final {
                continue;
            }
            if is_final {
                s.finalized_readers.insert(reader.id.clone());
            } else {
                s.finalized_readers.remove(&reader.id);
            }
            let msg = ServerMsg::Caption {
                seg: format!("{}-{id}", self.speaker),
                speaker: self.speaker.clone(),
                src_lang: audience.src_lang.clone(),
                src: s.src.clone(),
                tgt_lang: tgt.unwrap_or(&audience.src_lang).to_owned(),
                text: tr.and_then(|t| t.text.clone()),
                is_final,
                rev: s.rev,
            };
            room::send(&reader.tx, msg);
        }

        // 모든 번역이 끝난 확정 문장은 문맥으로 옮기고 지운다.
        if s.locked && targets.iter().all(|l| s.tr.get(l).is_some_and(|t| t.done)) {
            let s = self.sentences.remove(&id).expect("exists");
            for (lang, tr) in s.tr {
                if let (true, Some(text)) = (tr.done, tr.text) {
                    let pairs = self.context.entry(lang).or_default();
                    pairs.push_back((s.src.clone(), text));
                    if pairs.len() > CONTEXT_PAIRS {
                        pairs.pop_front();
                    }
                }
            }
        }
    }

    fn schedule(&mut self, id: u64, lang: &str, src_lang: &str) {
        let context: Vec<(String, String)> = self
            .context
            .get(lang)
            .map(|c| c.iter().cloned().collect())
            .unwrap_or_default();
        let Some(s) = self.sentences.get_mut(&id) else {
            return;
        };
        let (version, complete) = (s.version, s.locked);
        let tr = s.tr.entry(lang.to_owned()).or_default();
        if tr.done {
            return;
        }
        if let Some(f) = &tr.inflight {
            if !complete || (f.complete && f.version == version) {
                // 말하는 중: 진행 중인 요청이 끝나면 on_done → refresh에서 최신 텍스트로 다시 온다.
                return;
            }
            // 확정됨: 말하는 중 요청은 버리고 확정 번역을 바로 시작한다.
            f.task.abort();
            tr.inflight = None;
        }
        if !complete && tr.tried >= version {
            return;
        }

        let req = TranslateRequest {
            src_lang: src_lang.to_owned(),
            tgt_lang: lang.to_owned(),
            text: s.src.clone(),
            complete,
            context,
        };
        let done_tx = self.done_tx.clone();
        let translator = self.pipeline.translator.clone();
        let (lang, stt_at) = (lang.to_owned(), s.stt_at);
        let task = tokio::spawn(async move {
            let result = match translator {
                Some(t) => {
                    translate_hedged(t, req, HEDGE_AFTER, |text| {
                        let _ = done_tx.try_send(Done {
                            finished: false,
                            sentence: id,
                            lang: lang.clone(),
                            version,
                            complete,
                            stt_at,
                            result: Ok(Translation {
                                text: text.to_owned(),
                                first_token: Duration::ZERO,
                                total: Duration::ZERO,
                                hedged: false,
                            }),
                        });
                    })
                    .await
                }
                None => Err(TranslateError("no translator configured".into())),
            };
            let _ = done_tx
                .send(Done {
                    finished: true,
                    sentence: id,
                    lang,
                    version,
                    complete,
                    stt_at,
                    result,
                })
                .await;
        });
        tr.inflight = Some(Inflight {
            version,
            complete,
            task: task.abort_handle(),
        });
    }

    fn on_done(&mut self, done: Done) {
        if self.sync_audience().is_none() {
            return;
        }
        let Some(s) = self.sentences.get_mut(&done.sentence) else {
            return;
        };
        let Some(tr) = s.tr.get_mut(&done.lang) else {
            return;
        };
        // 취소 직전에 끝난 요청의 결과는 버린다.
        if !tr
            .inflight
            .as_ref()
            .is_some_and(|f| f.version == done.version && f.complete == done.complete)
        {
            return;
        }
        if !done.finished {
            if let Ok(t) = done.result
                && done.version >= tr.version
            {
                tr.text = Some(t.text);
                tr.version = done.version;
            }
            self.refresh(done.sentence, Some(&done.lang));
            return;
        }
        tr.inflight = None;
        tr.tried = tr.tried.max(done.version);
        let is_final = done.complete && done.version == s.version;
        let mut failed = false;
        match done.result {
            Ok(t) => {
                // 지연 측정: STT 도착 → 번역 완료(e2e), 그중 번역 첫 토큰과 전체. 원문·번역은 남기지 않는다.
                tracing::debug!(
                    lang = %done.lang,
                    complete = done.complete,
                    hedged = t.hedged,
                    first_token_ms = t.first_token.as_millis() as u64,
                    translate_ms = t.total.as_millis() as u64,
                    e2e_ms = done.stt_at.elapsed().as_millis() as u64,
                    "translated"
                );
                if done.version >= tr.version {
                    tr.text = Some(t.text);
                    tr.version = done.version;
                }
                tr.done = is_final;
            }
            Err(err) => {
                tracing::warn!(lang = %done.lang, complete = done.complete, %err, "translation failed");
                if is_final {
                    // 원문만 확정한다. 이전 원문(말하는 중)의 번역은 틀릴 수 있으니 지운다.
                    tr.text = None;
                    tr.done = true;
                    failed = true;
                }
            }
        }
        if failed {
            self.notify_failure(&done.lang);
        }
        self.refresh(done.sentence, Some(&done.lang));
    }

    /// 번역 실패를 그 언어로 읽는 사람들에게 알린다. 장애 중 알림이 쏟아지지 않게 간격을 둔다.
    fn notify_failure(&mut self, lang: &str) {
        if self
            .last_notice
            .is_some_and(|t| t.elapsed() < NOTICE_INTERVAL)
        {
            return;
        }
        let Some(audience) = self.sync_audience() else {
            return;
        };
        self.last_notice = Some(Instant::now());
        for r in audience_readers_of(&audience, lang) {
            room::send(
                &r.tx,
                ServerMsg::Notice {
                    code: NoticeCode::TranslationFailed,
                },
            );
        }
    }
}

fn audience_readers_of<'a>(
    audience: &'a Audience,
    lang: &'a str,
) -> impl Iterator<Item = &'a room::Reader> {
    audience
        .readers
        .iter()
        .filter(move |r| audience.target_for(&r.lang) == Some(lang))
}

#[cfg(test)]
mod tests;
