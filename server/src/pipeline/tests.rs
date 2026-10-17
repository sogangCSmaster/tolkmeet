use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, mpsc};

use super::*;
use crate::protocol::{MediaState, RoomSettings};
use crate::providers::testing::Fake;
use crate::room::{JoinOutcome, JoinRequest, Outbox};

const HOST: &str = "alice";

struct Room {
    rooms: Arc<Rooms>,
    code: String,
    host: PeerId,
}

type Inbox = mpsc::Receiver<ServerMsg>;

fn join_req(name: &str, lang: &str, partner: &str) -> JoinRequest {
    JoinRequest {
        name: name.into(),
        lang: lang.into(),
        partner_lang: partner.into(),
        media: MediaState::default(),
        resume: None,
    }
}

/// 주최자(화자, ko→ja)와 게스트들이 입장한 방
fn room(guest_langs: &[&str]) -> (Room, Inbox, Vec<Inbox>) {
    let rooms = Arc::new(Rooms::default());
    let code = rooms
        .create(HOST, RoomSettings { translation: true })
        .unwrap();
    let (tx, mut host_rx) = mpsc::channel(256);
    let tx = Outbox::new(tx, Arc::new(Notify::new()));
    let JoinOutcome::Joined(host) = rooms
        .join(&code, join_req("A", "ko", "ja"), Some(HOST), tx)
        .unwrap()
    else {
        panic!("host joins directly")
    };
    let mut guests = Vec::new();
    for (i, lang) in guest_langs.iter().enumerate() {
        let (tx, mut rx) = mpsc::channel(256);
        let tx = Outbox::new(tx, Arc::new(Notify::new()));
        let JoinOutcome::Waiting(id) = rooms
            .join(&code, join_req(&format!("G{i}"), lang, "ko"), None, tx)
            .unwrap()
        else {
            panic!("guest waits")
        };
        rooms.admit(&code, &host, &id).unwrap();
        while rx.try_recv().is_ok() {}
        guests.push(rx);
    }
    while host_rx.try_recv().is_ok() {}
    (Room { rooms, code, host }, host_rx, guests)
}

fn pipeline(room: &Room, fake: Fake) -> (Arc<Pipeline>, Arc<Fake>) {
    let fake = Arc::new(fake);
    let p = Arc::new(Pipeline::new(room.rooms.clone(), Some(fake.clone())));
    (p, fake)
}

#[derive(Debug, PartialEq)]
struct Cap {
    seg: String,
    src: String,
    tgt_lang: String,
    text: Option<String>,
    is_final: bool,
}

fn captions(rx: &mut Inbox) -> Vec<Cap> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let ServerMsg::Caption {
            seg,
            src,
            tgt_lang,
            text,
            is_final,
            ..
        } = msg
        {
            out.push(Cap {
                seg,
                src,
                tgt_lang,
                text,
                is_final,
            });
        }
    }
    out
}

async fn wait(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[tokio::test(start_paused = true)]
async fn interim_is_shown_then_translated_then_finalized() {
    let (room, mut host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(900)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    let seg = format!("{}-0", room.host);

    speaker.push(0, "저희 투자팀은".into(), false);
    wait(10).await;
    let first = captions(&mut guests[0]);
    assert_eq!(
        first,
        [Cap {
            seg: seg.clone(),
            src: "저희 투자팀은".into(),
            tgt_lang: "ja".into(),
            text: None,
            is_final: false,
        }]
    );
    // 화자 자신도 같은 줄을 상대 언어 번역으로 본다.
    assert_eq!(captions(&mut host_rx).len(), 1);

    wait(900).await;
    let translated = captions(&mut guests[0]);
    assert_eq!(
        translated.last().unwrap().text.as_deref(),
        Some("[ja] 저희 투자팀은")
    );
    assert!(!translated.last().unwrap().is_final);

    speaker.push(0, "저희 투자팀은 AI에 투자합니다.".into(), true);
    wait(10).await;
    let locked = captions(&mut guests[0]);
    // 확정 번역이 오기 전까지는 이전 번역을 보여주며 확정되지 않은 상태다.
    assert_eq!(locked.last().unwrap().src, "저희 투자팀은 AI에 투자합니다.");
    assert!(!locked.last().unwrap().is_final);

    wait(900).await;
    let done = captions(&mut guests[0]);
    assert_eq!(
        done.last().unwrap(),
        &Cap {
            seg,
            src: "저희 투자팀은 AI에 투자합니다.".into(),
            tgt_lang: "ja".into(),
            text: Some("[ja] 저희 투자팀은 AI에 투자합니다.".into()),
            is_final: true,
        }
    );
    assert_eq!(fake.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn interims_during_a_translation_are_coalesced_to_the_latest() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(1000)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "하나".into(), false);
    wait(300).await;
    speaker.push(0, "하나 둘".into(), false);
    wait(300).await;
    speaker.push(0, "하나 둘 셋".into(), false);
    wait(300).await;
    assert_eq!(fake.calls(), 1, "no new request while one is in flight");

    wait(200).await; // 첫 요청 완료(1000ms) → 최신 텍스트로 바로 다음 요청
    assert_eq!(fake.calls(), 2);
    assert_eq!(fake.requests.lock().unwrap()[1].text, "하나 둘 셋");
    assert!(!fake.requests.lock().unwrap()[1].complete);

    let caps = captions(&mut guests[0]);
    let last = caps.last().unwrap();
    // 번역은 이전 원문의 것이지만 원문은 최신이다.
    assert_eq!(last.src, "하나 둘 셋");
    assert_eq!(last.text.as_deref(), Some("[ja] 하나"));
}

#[tokio::test(start_paused = true)]
async fn locking_cancels_the_in_progress_request() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(1000)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "네".into(), false);
    wait(100).await;
    speaker.push(0, "네.".into(), true);
    wait(10).await;
    assert_eq!(fake.calls(), 2, "final request starts immediately");
    assert!(fake.requests.lock().unwrap()[1].complete);

    wait(1000).await;
    let caps = captions(&mut guests[0]);
    assert!(caps.last().unwrap().is_final);
    assert_eq!(caps.iter().filter(|c| c.is_final).count(), 1);
}

#[tokio::test(start_paused = true)]
async fn finals_split_into_sentences_and_carry_context() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "안녕하세요. 저는".into(), true);
    wait(200).await;
    speaker.push(1, "유승재입니다.".into(), true);
    wait(200).await;

    let finals: Vec<Cap> = captions(&mut guests[0])
        .into_iter()
        .filter(|c| c.is_final)
        .collect();
    assert_eq!(finals.len(), 2);
    assert_eq!(finals[0].src, "안녕하세요.");
    assert_eq!(finals[1].src, "저는 유승재입니다.");
    assert_ne!(finals[0].seg, finals[1].seg);

    let reqs = fake.requests.lock().unwrap();
    let last = reqs.last().unwrap();
    assert_eq!(last.text, "저는 유승재입니다.");
    assert_eq!(
        last.context,
        [("안녕하세요.".to_owned(), "[ja] 안녕하세요.".to_owned())]
    );
}

#[tokio::test(start_paused = true)]
async fn unfinished_final_is_locked_after_silence() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "주로 AI에".into(), true);
    wait(1500).await;
    assert!(!captions(&mut guests[0]).iter().any(|c| c.is_final));
    wait(700).await;
    let caps = captions(&mut guests[0]);
    let last = caps.last().unwrap();
    assert!(last.is_final);
    assert_eq!(last.src, "주로 AI에");
}

#[tokio::test(start_paused = true)]
async fn missing_final_is_recovered_from_next_segment() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "주로".into(), false);
    wait(10).await;
    // 구간 0의 확정 결과 없이 구간 1이 시작됨
    speaker.push(1, "AI에 투자합니다.".into(), true);
    wait(200).await;
    let caps = captions(&mut guests[0]);
    let last = caps.last().unwrap();
    assert!(last.is_final);
    assert_eq!(last.src, "주로 AI에 투자합니다.");
    // 늦게 온 구간 0 메시지는 무시한다.
    speaker.push(0, "주로 무시".into(), false);
    wait(10).await;
    assert!(captions(&mut guests[0]).is_empty());
}

#[tokio::test(start_paused = true)]
async fn each_target_language_is_translated_once() {
    // 화자 ko→ja, 독자: ja, ja, en
    let (room, mut host_rx, mut guests) = room(&["ja", "ja", "en"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "좋아요.".into(), true);
    wait(200).await;
    assert_eq!(fake.calls(), 2);
    for (rx, lang) in guests.iter_mut().zip(["ja", "ja", "en"]) {
        let last = captions(rx).pop().unwrap();
        assert_eq!(last.tgt_lang, lang);
        assert_eq!(last.text, Some(format!("[{lang}] 좋아요.")));
        assert!(last.is_final);
    }
    assert_eq!(captions(&mut host_rx).pop().unwrap().tgt_lang, "ja");
}

#[tokio::test(start_paused = true)]
async fn failed_translation_finalizes_original_and_notifies() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let mut fake = Fake::new(vec![ms(100)]);
    fake.fail = true;
    let (p, _fake) = pipeline(&room, fake);
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "좋아요.".into(), true);
    wait(200).await;
    let mut notices = 0;
    let mut last = None;
    while let Ok(msg) = guests[0].try_recv() {
        match msg {
            ServerMsg::Notice { code } => {
                assert_eq!(code, NoticeCode::TranslationFailed);
                notices += 1;
            }
            ServerMsg::Caption { text, is_final, .. } => last = Some((text, is_final)),
            _ => {}
        }
    }
    assert_eq!(notices, 1);
    assert_eq!(last, Some((None, true)));
}

#[tokio::test(start_paused = true)]
async fn nothing_is_sent_when_translation_is_off() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: false })
        .unwrap();
    while guests[0].try_recv().is_ok() {}
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "좋아요.".into(), true);
    wait(200).await;
    assert!(captions(&mut guests[0]).is_empty());
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn same_language_reader_gets_original_only() {
    // 화자 ko→ko(상대 언어를 같게 둠), 독자 ko
    let (room, _host_rx, mut guests) = room(&["ko"]);
    room.rooms
        .update(&room.code, &room.host, |p| p.partner_lang = "ko".into());
    while guests[0].try_recv().is_ok() {}
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "좋아요.".into(), true);
    wait(10).await;
    let last = captions(&mut guests[0]).pop().unwrap();
    assert_eq!(last.text, None);
    assert!(last.is_final);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn speech_while_waiting_is_never_captioned() {
    let (room, _host_rx, _guests) = room(&[]);
    let (tx, mut guest_rx) = mpsc::channel(256);
    let tx = Outbox::new(tx, Arc::new(Notify::new()));
    let JoinOutcome::Waiting(guest) = room
        .rooms
        .join(&room.code, join_req("B", "ja", "ko"), None, tx)
        .unwrap()
    else {
        panic!("guest waits")
    };
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), guest.clone());

    speaker.push(0, "まだ".into(), true);
    wait(10).await;
    room.rooms.admit(&room.code, &room.host, &guest).unwrap();
    wait(3000).await;
    assert!(captions(&mut guest_rx).is_empty());
    assert_eq!(fake.calls(), 0);

    // 입장 후에는 정상 동작
    speaker.push(1, "はい。".into(), true);
    wait(200).await;
    assert!(captions(&mut guest_rx).last().unwrap().is_final);
}

#[tokio::test(start_paused = true)]
async fn disable_enable_must_not_mix_old_buffer_into_new_speech() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "old words".into(), true);
    wait(200).await;
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: false })
        .unwrap();
    wait(100).await;
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: true })
        .unwrap();
    captions(&mut guests[0]);
    speaker.push(1, "new words.".into(), true);
    wait(200).await;
    let caps = captions(&mut guests[0]);
    assert_eq!(caps.last().unwrap().src, "new words.");
}

#[tokio::test(start_paused = true)]
async fn language_change_must_not_mix_source_languages() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "이전 한국어".into(), true);
    wait(200).await;
    room.rooms
        .update(&room.code, &room.host, |peer| peer.lang = "en".into());
    captions(&mut guests[0]);
    speaker.push(1, "English now.".into(), true);
    wait(200).await;
    let caps = captions(&mut guests[0]);
    assert_eq!(caps.last().unwrap().src, "English now.");
    assert_eq!(fake.requests.lock().unwrap().last().unwrap().src_lang, "en");
}

#[tokio::test(start_paused = true)]
async fn disabling_translation_must_cancel_inflight() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(1000)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "before off.".into(), true);
    wait(100).await;
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: false })
        .unwrap();
    wait(100).await;
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: true })
        .unwrap();
    captions(&mut guests[0]);
    wait(1000).await;
    assert!(captions(&mut guests[0]).is_empty());
}

#[tokio::test(start_paused = true)]
async fn target_removed_must_cancel_obsolete_translation() {
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Track(Arc<AtomicUsize>);
    impl Translator for Track {
        fn translate(
            &self,
            _req: TranslateRequest,
        ) -> futures_util::stream::BoxStream<'static, Result<String, TranslateError>> {
            let finished = self.0.clone();
            futures_util::stream::once(async move {
                tokio::time::sleep(ms(1000)).await;
                finished.fetch_add(1, Ordering::SeqCst);
                Ok("done".into())
            })
            .boxed()
        }
    }
    let (room, _host_rx, _guests) = room(&["ko"]);
    let finished = Arc::new(AtomicUsize::new(0));
    let p = Arc::new(Pipeline::new(
        room.rooms.clone(),
        Some(Arc::new(Track(finished.clone()))),
    ));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "before".into(), false);
    wait(100).await;
    room.rooms.update(&room.code, &room.host, |peer| {
        peer.partner_lang = "ko".into()
    });
    speaker.push(0, "before.".into(), true);
    wait(100).await;
    drop(speaker);
    wait(1100).await;
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "obsolete request was cancelled"
    );
}

#[tokio::test(start_paused = true)]
async fn queued_speech_cannot_cross_a_disable_enable_boundary() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "queued before off.".into(), true);
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: false })
        .unwrap();
    speaker.push(1, "disabled.".into(), true);
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: true })
        .unwrap();
    wait(200).await;
    assert!(captions(&mut guests[0]).is_empty());
    assert_eq!(fake.calls(), 0);
    speaker.push(2, "current.".into(), true);
    wait(200).await;
    assert_eq!(captions(&mut guests[0]).last().unwrap().src, "current.");
}

#[tokio::test(start_paused = true)]
async fn queued_speech_cannot_cross_a_source_language_change() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "이전 한국어.".into(), true);
    room.rooms
        .update(&room.code, &room.host, |peer| peer.lang = "en".into());
    wait(200).await;
    assert!(captions(&mut guests[0]).is_empty());
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn evicted_sentences_finish_with_original_instead_of_staying_pending() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![Duration::from_secs(30)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    for seg in 0..=MAX_SENTENCES as u32 {
        speaker.push(seg, format!("Sentence {seg}."), true);
        wait(10).await;
    }
    let caps = captions(&mut guests[0]);
    let first = caps.iter().rfind(|c| c.src == "Sentence 0.").unwrap();
    assert!(first.is_final);
    assert_eq!(first.text, None);
}

#[tokio::test(start_paused = true)]
async fn translated_chunks_are_visible_before_stream_completion() {
    use futures_util::{StreamExt, stream};
    struct Streaming;
    impl Translator for Streaming {
        fn translate(
            &self,
            _req: TranslateRequest,
        ) -> futures_util::stream::BoxStream<'static, Result<String, TranslateError>> {
            stream::once(async {
                tokio::time::sleep(ms(100)).await;
                Ok("first".into())
            })
            .chain(stream::once(async {
                tokio::time::sleep(ms(1000)).await;
                Ok(" second".into())
            }))
            .boxed()
        }
    }
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let p = Arc::new(Pipeline::new(room.rooms.clone(), Some(Arc::new(Streaming))));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "한 문장.".into(), true);
    wait(200).await;
    let caps = captions(&mut guests[0]);
    let first = caps.last().unwrap();
    assert_eq!(first.text.as_deref(), Some("first"));
    assert!(!first.is_final);
    wait(1000).await;
    let caps = captions(&mut guests[0]);
    let last = caps.last().unwrap();
    assert_eq!(last.text.as_deref(), Some("first second"));
    assert!(last.is_final);
}

#[tokio::test(start_paused = true)]
async fn empty_final_retracts_interim_and_next_segment_starts_a_new_line() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(1000)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "discard this interim".into(), false);
    wait(10).await;
    let old = captions(&mut guests[0]).pop().unwrap();
    speaker.push(0, "   ".into(), true);
    wait(10).await;
    let retracted = captions(&mut guests[0]).pop().unwrap();
    assert_eq!(retracted.seg, old.seg);
    assert!(retracted.src.is_empty());
    assert!(retracted.is_final);
    speaker.push(1, "new sentence.".into(), true);
    wait(10).await;
    let next = captions(&mut guests[0]).pop().unwrap();
    assert_ne!(next.seg, old.seg);
    assert_eq!(next.src, "new sentence.");
}

#[tokio::test(start_paused = true)]
async fn disabling_translation_retracts_pending_captions() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(1000)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "pending.".into(), true);
    wait(10).await;
    let first = captions(&mut guests[0]).pop().unwrap();
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: false })
        .unwrap();
    wait(10).await;
    let retracted = captions(&mut guests[0]).pop().unwrap();
    assert_eq!(first.seg, retracted.seg);
    assert!(retracted.is_final && retracted.src.is_empty());
}

#[tokio::test(start_paused = true)]
async fn departing_speaker_keeps_finished_sentences_and_retracts_only_interim() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(1000)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "finished.".into(), true);
    wait(10).await;
    speaker.push(1, "still talking".into(), false);
    wait(10).await;
    let before = captions(&mut guests[0]);
    let seg_of = |src: &str| before.iter().find(|c| c.src == src).unwrap().seg.clone();
    let (finished, open) = (seg_of("finished."), seg_of("still talking"));
    // 번역이 끝나기 전에 화자가 나간다(연결이 끊긴 경우도 같다).
    room.rooms.leave(&room.code, &room.host);
    drop(speaker);
    wait(10).await;
    let after = captions(&mut guests[0]);
    let last = |seg: &str| after.iter().rfind(|c| c.seg == seg).unwrap();
    // 말하던 중인 글자는 바로 지운다.
    assert!(last(&open).is_final && last(&open).src.is_empty());
    // 이미 말을 마친 문장은 번역이 끝나면 남은 사람에게 마저 보낸다.
    wait(1500).await;
    assert_eq!(
        captions(&mut guests[0]).last().unwrap(),
        &Cap {
            seg: finished.clone(),
            src: "finished.".into(),
            tgt_lang: "ja".into(),
            text: Some("[ja] finished.".into()),
            is_final: true,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn last_words_without_a_period_are_kept_when_the_speaker_leaves_at_once() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(500)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    // 확정 결과지만 마침표가 없어 문장 확정을 기다리는 중(FLUSH_AFTER 전)에 나간다.
    speaker.push(0, "네 그럼 다음에 봬요".into(), true);
    wait(10).await;
    room.rooms.leave(&room.code, &room.host);
    drop(speaker);
    wait(1000).await;
    let last = captions(&mut guests[0]).pop().unwrap();
    assert_eq!(last.src, "네 그럼 다음에 봬요");
    assert_eq!(last.text.as_deref(), Some("[ja] 네 그럼 다음에 봬요"));
    assert!(last.is_final);
}

#[tokio::test(start_paused = true)]
async fn departed_speakers_slow_translation_ends_as_the_original() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![Duration::from_secs(60)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "finished.".into(), true);
    wait(10).await;
    let seg = captions(&mut guests[0]).pop().unwrap().seg;
    room.rooms.leave(&room.code, &room.host);
    drop(speaker);
    // 기다리는 시간이 지나면 번역 없이 원문으로 확정한다.
    wait(6000).await;
    assert_eq!(
        captions(&mut guests[0]).last().unwrap(),
        &Cap { seg, src: "finished.".into(), tgt_lang: "ja".into(), text: None, is_final: true }
    );
}

#[tokio::test(start_paused = true)]
async fn translation_progress_goes_only_to_readers_of_that_language() {
    let (room, _host_rx, mut guests) = room(&["ja", "en"]);
    // Target scheduling is sorted: English is slow, Japanese finishes first.
    let (p, _fake) = pipeline(&room, Fake::new(vec![Duration::from_secs(30), ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "번역 중.".into(), true);
    wait(10).await;
    assert_eq!(captions(&mut guests[0]).len(), 1);
    assert_eq!(captions(&mut guests[1]).len(), 1);
    wait(200).await;
    assert!(captions(&mut guests[0]).last().unwrap().is_final);
    assert!(
        captions(&mut guests[1]).is_empty(),
        "the English reader's line did not change"
    );
}

fn bounded_room() -> (Room, Inbox, Arc<Notify>) {
    let rooms = Arc::new(Rooms::default());
    let code = rooms
        .create(HOST, RoomSettings { translation: true })
        .unwrap();
    let disconnect = Arc::new(Notify::new());
    let (tx, mut rx) = mpsc::channel(64);
    let JoinOutcome::Joined(host) = rooms
        .join(
            &code,
            join_req("A", "ko", "ja"),
            Some(HOST),
            Outbox::new(tx, disconnect.clone()),
        )
        .unwrap()
    else {
        panic!("host joins")
    };
    while rx.try_recv().is_ok() {}
    (Room { rooms, code, host }, rx, disconnect)
}

#[tokio::test(start_paused = true)]
async fn punctuation_heavy_frame_has_bounded_fanout_without_losing_text() {
    let (room, mut rx, disconnect) = bounded_room();
    let p = Arc::new(Pipeline::new(room.rooms.clone(), None));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    let text = "a. ".repeat(300);
    speaker.push(0, text.clone(), true);
    wait(10).await;
    assert!(
        tokio::time::timeout(ms(1), disconnect.notified())
            .await
            .is_err()
    );
    let finals: Vec<_> = captions(&mut rx)
        .into_iter()
        .filter(|c| c.is_final)
        .collect();
    assert_eq!(finals.len(), MAX_SENTENCES_PER_UPDATE);
    assert_eq!(
        finals
            .iter()
            .map(|c| c.src.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        text.trim()
    );
}

#[tokio::test(start_paused = true)]
async fn buffered_stream_burst_does_not_overflow_caption_outbox() {
    use futures_util::{StreamExt, stream};
    struct Burst;
    impl Translator for Burst {
        fn translate(
            &self,
            _req: TranslateRequest,
        ) -> futures_util::stream::BoxStream<'static, Result<String, TranslateError>> {
            stream::iter((0..1000).map(|_| Ok("a".to_owned()))).boxed()
        }
    }
    let (room, mut rx, disconnect) = bounded_room();
    let p = Arc::new(Pipeline::new(room.rooms.clone(), Some(Arc::new(Burst))));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "sentence.".into(), true);
    wait(10).await;
    assert!(
        tokio::time::timeout(ms(1), disconnect.notified())
            .await
            .is_err()
    );
    let last = captions(&mut rx).pop().unwrap();
    assert!(last.is_final);
    assert_eq!(last.text, Some("a".repeat(1000)));
}

#[tokio::test(start_paused = true)]
async fn cancelling_pending_languages_preserves_already_finalized_readers() {
    let (room, _host_rx, mut guests) = room(&["ja", "en"]);
    // Target scheduling is sorted: English is slow, Japanese finishes first.
    let (p, _fake) = pipeline(&room, Fake::new(vec![Duration::from_secs(30), ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    speaker.push(0, "완료된 번역.".into(), true);
    wait(200).await;
    assert!(captions(&mut guests[0]).last().unwrap().is_final);
    assert!(!captions(&mut guests[1]).last().unwrap().is_final);
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: false })
        .unwrap();
    wait(10).await;
    assert!(
        captions(&mut guests[0]).is_empty(),
        "finished Japanese caption must remain intact"
    );
    let cancelled = captions(&mut guests[1]).pop().unwrap();
    assert!(cancelled.is_final && cancelled.src.is_empty());
}
