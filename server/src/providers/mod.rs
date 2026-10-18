//! 외부 번역 제공자. 파이프라인은 `Translator` trait만 알고, 구현체를 바꿔 끼워 여러 제공자를 지원한다.

pub mod gemini;
pub mod live;

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;

#[derive(Debug, Clone)]
pub struct TranslateRequest {
    /// BCP-47
    pub src_lang: String,
    pub tgt_lang: String,
    pub text: String,
    /// 말하는 중인 문장이면 false. 모델에게 뒷부분을 추측하지 말라고 알린다.
    pub complete: bool,
    /// 직전에 확정된 (원문, 번역) 쌍. 오래된 것부터.
    pub context: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct TranslateError(pub String);

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait Translator: Send + Sync {
    /// 번역 결과를 조각(토큰 묶음) 단위로 흘려보낸다. 요청은 스트림을 처음 poll할 때 시작한다.
    fn translate(
        &self,
        req: TranslateRequest,
    ) -> BoxStream<'static, Result<String, TranslateError>>;
}

pub struct Translation {
    pub text: String,
    pub first_token: Duration,
    pub total: Duration,
    pub hedged: bool,
}

/// 헤징: `hedge_after` 안에 첫 조각이 없으면 같은 요청을 하나 더 보내고, 먼저 첫 조각을 내는 쪽을 쓴다.
/// 드물게 생기는 20초급 지연을 없앤다(docs/benchmarks/2026-10-16-gemini.md). 진 쪽 요청은 버려서 취소한다.
pub async fn translate_hedged(
    translator: Arc<dyn Translator>,
    req: TranslateRequest,
    hedge_after: Duration,
    mut on_chunk: impl FnMut(&str),
) -> Result<Translation, TranslateError> {
    let started = Instant::now();
    let mut first = translator.translate(req.clone());
    let (head, mut rest, hedged) =
        match tokio::time::timeout(hedge_after, first_chunk(&mut first)).await {
            Ok(head) => (head?, first, false),
            Err(_) => {
                let mut second = translator.translate(req);
                tokio::select! {
                    head = first_chunk(&mut first) => match head {
                        Ok(head) => {
                            drop(second);
                            (head, first, true)
                        }
                        Err(_) => {
                            drop(first);
                            (first_chunk(&mut second).await?, second, true)
                        }
                    },
                    head = first_chunk(&mut second) => match head {
                        Ok(head) => {
                            drop(first);
                            (head, second, true)
                        }
                        Err(_) => {
                            drop(second);
                            (first_chunk(&mut first).await?, first, true)
                        }
                    },
                }
            }
        };
    let first_token = started.elapsed();
    let mut text = head;
    on_chunk(text.trim());
    while let Some(chunk) = rest.next().await {
        let chunk = chunk?;
        if !chunk.is_empty() {
            text.push_str(&chunk);
            on_chunk(text.trim());
        }
    }
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(TranslateError("empty translation".into()));
    }
    Ok(Translation {
        text,
        first_token,
        total: started.elapsed(),
        hedged,
    })
}

async fn first_chunk(
    stream: &mut BoxStream<'static, Result<String, TranslateError>>,
) -> Result<String, TranslateError> {
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if !chunk.is_empty() {
            return Ok(chunk);
        }
    }
    Err(TranslateError("empty translation".into()))
}

/// 프롬프트에 넣을 영어 언어 이름. 모르는 태그는 그대로 쓴다(모델이 BCP-47도 이해한다).
pub fn language_name(tag: &str) -> &str {
    let primary = tag.split('-').next().unwrap_or(tag);
    match (primary, tag) {
        (_, "zh-TW" | "zh-Hant") => "Traditional Chinese",
        ("zh", _) => "Simplified Chinese",
        ("ko", _) => "Korean",
        ("ja", _) => "Japanese",
        ("en", _) => "English",
        ("es", _) => "Spanish",
        ("fr", _) => "French",
        ("de", _) => "German",
        ("vi", _) => "Vietnamese",
        ("th", _) => "Thai",
        _ => tag,
    }
}

#[cfg(test)]
pub mod testing {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::stream;

    use super::*;

    /// 호출마다 정해 둔 지연 뒤에 `[tgt] text`를 돌려주는 가짜 번역기.
    pub struct Fake {
        pub delays: Mutex<Vec<Duration>>,
        pub calls: AtomicUsize,
        pub requests: Mutex<Vec<TranslateRequest>>,
        pub fail: bool,
    }

    impl Fake {
        pub fn new(delays: Vec<Duration>) -> Self {
            Fake {
                delays: Mutex::new(delays),
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                fail: false,
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Translator for Fake {
        fn translate(
            &self,
            req: TranslateRequest,
        ) -> BoxStream<'static, Result<String, TranslateError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let delay = {
                let mut d = self.delays.lock().unwrap();
                if d.len() > 1 {
                    d.remove(0)
                } else {
                    d.first().copied().unwrap_or_default()
                }
            };
            let out = if self.fail {
                Err(TranslateError("fake failure".into()))
            } else {
                Ok(format!("[{}] {}", req.tgt_lang, req.text))
            };
            self.requests.lock().unwrap().push(req);
            stream::once(async move {
                tokio::time::sleep(delay).await;
                out
            })
            .boxed()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Fake;
    use super::*;

    fn req() -> TranslateRequest {
        TranslateRequest {
            src_lang: "ko".into(),
            tgt_lang: "ja".into(),
            text: "안녕".into(),
            complete: true,
            context: Vec::new(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fast_first_request_is_not_hedged() {
        let fake = Arc::new(Fake::new(vec![Duration::from_millis(900)]));
        let t = translate_hedged(fake.clone(), req(), Duration::from_millis(1500), |_| {})
            .await
            .unwrap();
        assert_eq!(t.text, "[ja] 안녕");
        assert!(!t.hedged);
        assert_eq!(fake.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_first_request_is_hedged_and_second_wins() {
        let fake = Arc::new(Fake::new(vec![
            Duration::from_secs(20),
            Duration::from_millis(800),
        ]));
        let t = translate_hedged(fake.clone(), req(), Duration::from_millis(1500), |_| {})
            .await
            .unwrap();
        assert!(t.hedged);
        assert_eq!(fake.calls(), 2);
        assert_eq!(t.first_token, Duration::from_millis(2300));
    }

    #[tokio::test(start_paused = true)]
    async fn failure_is_reported() {
        let mut fake = Fake::new(vec![Duration::ZERO]);
        fake.fail = true;
        assert!(
            translate_hedged(Arc::new(fake), req(), Duration::from_millis(1500), |_| {})
                .await
                .is_err()
        );
    }

    struct Scripted {
        streams: std::sync::Mutex<
            std::collections::VecDeque<BoxStream<'static, Result<String, TranslateError>>>,
        >,
    }

    impl Translator for Scripted {
        fn translate(
            &self,
            _: TranslateRequest,
        ) -> BoxStream<'static, Result<String, TranslateError>> {
            self.streams.lock().unwrap().pop_front().unwrap()
        }
    }

    struct DropMark(Arc<std::sync::atomic::AtomicUsize>);

    impl Drop for DropMark {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn scripted(
        chunks: Vec<(u64, Result<&str, &str>)>,
        drops: Arc<std::sync::atomic::AtomicUsize>,
    ) -> BoxStream<'static, Result<String, TranslateError>> {
        let chunks: std::collections::VecDeque<_> = chunks
            .into_iter()
            .map(|(ms, text)| {
                (
                    ms,
                    text.map(str::to_owned)
                        .map_err(|e| TranslateError(e.into())),
                )
            })
            .collect();
        futures_util::stream::unfold(
            (chunks, DropMark(drops)),
            |(mut chunks, marker)| async move {
                let (ms, chunk) = chunks.pop_front()?;
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Some((chunk, (chunks, marker)))
            },
        )
        .boxed()
    }

    #[tokio::test(start_paused = true)]
    async fn either_hedged_failure_preserves_the_other_success() {
        for (first, second) in [
            ((2000, Ok("success")), (10, Err("HTTP 429"))),
            ((1550, Err("HTTP 500")), (100, Ok("success"))),
        ] {
            let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let translator = Arc::new(Scripted {
                streams: std::sync::Mutex::new(
                    [
                        scripted(vec![first], drops.clone()),
                        scripted(vec![second], drops),
                    ]
                    .into(),
                ),
            });
            let result = translate_hedged(translator, req(), Duration::from_millis(1500), |_| {})
                .await
                .unwrap();
            assert_eq!(result.text, "success");
            assert!(result.hedged);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn streamed_winner_is_visible_before_completion_and_loser_is_cancelled() {
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let translator = Arc::new(Scripted {
            streams: std::sync::Mutex::new(
                [
                    scripted(vec![(20_000, Ok("loser"))], drops.clone()),
                    scripted(vec![(10, Ok("a")), (1000, Ok("b"))], drops.clone()),
                ]
                .into(),
            ),
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            translate_hedged(translator, req(), Duration::from_millis(1500), |text| {
                tx.send(text.to_owned()).unwrap();
            })
            .await
            .unwrap()
        });
        assert_eq!(rx.recv().await.as_deref(), Some("a"));
        assert!(!task.is_finished());
        assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(rx.recv().await.as_deref(), Some("ab"));
        let result = task.await.unwrap();
        assert_eq!(result.text, "ab");
        assert_eq!(result.first_token, Duration::from_millis(1510));
        assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn language_names() {
        assert_eq!(language_name("ko"), "Korean");
        assert_eq!(language_name("zh-CN"), "Simplified Chinese");
        assert_eq!(language_name("zh-TW"), "Traditional Chinese");
        assert_eq!(language_name("th"), "Thai");
        // 모르는 태그는 그대로 넘긴다.
        assert_eq!(language_name("sw"), "sw");
    }
}
