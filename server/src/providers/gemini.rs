//! Gemini `streamGenerateContent`(SSE) 번역. 설정은 docs/benchmarks/2026-10-16-gemini.md에서 측정으로 정했다:
//! `gemini-3.5-flash-lite` + `thinkingLevel: minimal`, 유료 Standard에서 첫 토큰 p50 1.0s.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};
use serde_json::{Value, json};

use super::{TranslateError, TranslateRequest, Translator, language_name};
use crate::spend::TokenMeter;

const API: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const DEFAULT_MODEL: &str = "gemini-3.5-flash-lite";

const SYSTEM: &str = "You translate live meeting captions. The speaker may still be mid-sentence. \
Translate the CURRENT utterance from {src} to {tgt} naturally, using the previous lines only as context. \
If it is incomplete, translate what is there without guessing the rest. Output only the translation.";

pub struct Gemini {
    client: reqwest::Client,
    key: String,
    model: String,
    /// 응답의 토큰 사용량을 여기에 더한다(하루 지출 추정).
    meter: Arc<TokenMeter>,
}

impl Gemini {
    pub fn new(key: String, model: String, meter: Arc<TokenMeter>) -> Self {
        let client = reqwest::Client::builder()
            // TLS 연결 수립(약 0.1s)을 요청마다 반복하지 않도록 연결을 오래 살려 둔다.
            .pool_idle_timeout(None)
            .http2_keep_alive_interval(Duration::from_secs(30))
            .http2_keep_alive_while_idle(true)
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client");
        Gemini { client, key, model, meter }
    }

    /// 같은 호스트로 가는 다른 요청(음성 인식 토큰)이 열린 연결을 재사용하도록 공유한다.
    pub fn client(&self) -> reqwest::Client {
        self.client.clone()
    }

    /// 첫 번역 요청이 TLS 수립 시간을 물지 않도록 시작할 때 연결을 미리 열어 둔다.
    pub async fn warm_up(&self) {
        let res = self
            .client
            .get(format!("{API}/models/{}", self.model))
            .header("x-goog-api-key", &self.key)
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {
                tracing::info!(model = %self.model, "translator ready")
            }
            Ok(r) => {
                tracing::warn!(status = %r.status(), model = %self.model, "translator warm-up failed")
            }
            Err(err) => tracing::warn!(%err, "translator warm-up failed"),
        }
    }

    fn body(req: &TranslateRequest) -> Value {
        let system = SYSTEM
            .replace("{src}", language_name(&req.src_lang))
            .replace("{tgt}", language_name(&req.tgt_lang));
        let mut user = String::new();
        if !req.context.is_empty() {
            user.push_str("Previous lines:\n");
            for (src, tgt) in &req.context {
                user.push_str(&format!("{src} => {tgt}\n"));
            }
            user.push('\n');
        }
        user.push_str(&format!(
            "Current utterance ({}):\n{}",
            if req.complete {
                "complete"
            } else {
                "in progress"
            },
            req.text
        ));
        json!({
            "systemInstruction": { "parts": [{ "text": system }] },
            "contents": [{ "role": "user", "parts": [{ "text": user }] }],
            "generationConfig": {
                "temperature": 0.2,
                "maxOutputTokens": 2048,
                "thinkingConfig": { "thinkingLevel": "minimal" },
            },
        })
    }
}

enum State {
    Start(reqwest::RequestBuilder, Arc<TokenMeter>),
    Body {
        bytes: BoxStream<'static, reqwest::Result<bytes::Bytes>>,
        decoder: SseDecoder,
    },
    Done,
}

impl Translator for Gemini {
    fn translate(
        &self,
        req: TranslateRequest,
    ) -> BoxStream<'static, Result<String, TranslateError>> {
        let request = self
            .client
            .post(format!(
                "{API}/models/{}:streamGenerateContent?alt=sse",
                self.model
            ))
            .header("x-goog-api-key", &self.key)
            .timeout(Duration::from_secs(30))
            .json(&Self::body(&req));
        stream::unfold(State::Start(request, self.meter.clone()), |mut state| async move {
            loop {
                match state {
                    State::Done => return None,
                    State::Start(request, meter) => {
                        let res = match request.send().await {
                            Ok(res) => res,
                            Err(err) => return Some((Err(error(err)), State::Done)),
                        };
                        if !res.status().is_success() {
                            // 응답 본문은 로그에 남기지 않는다(요청 내용이 되돌아올 수 있다).
                            let err = TranslateError(format!("HTTP {}", res.status()));
                            return Some((Err(err), State::Done));
                        }
                        state = State::Body {
                            bytes: res.bytes_stream().boxed(),
                            decoder: SseDecoder::metered(meter),
                        };
                    }
                    State::Body {
                        mut bytes,
                        mut decoder,
                    } => {
                        if let Some(text) = decoder.ready.pop_front() {
                            return Some((Ok(text), State::Body { bytes, decoder }));
                        }
                        match bytes.next().await {
                            Some(Ok(chunk)) => {
                                if let Err(err) = decoder.push(&chunk) {
                                    return Some((Err(err), State::Done));
                                }
                                state = State::Body { bytes, decoder };
                            }
                            Some(Err(err)) => return Some((Err(error(err)), State::Done)),
                            None => {
                                return decoder.finish().err().map(|err| (Err(err), State::Done));
                            }
                        }
                    }
                }
            }
        })
        .boxed()
    }
}

fn error(err: reqwest::Error) -> TranslateError {
    // URL에는 키가 없지만(헤더로 보낸다) 혹시 모를 노출을 막기 위해 URL을 지운다.
    TranslateError(err.without_url().to_string())
}

/// STOP을 확인하기 전까지는 HTTP EOF만으로 번역 성공을 판단하지 않는다.
#[derive(Default)]
struct SseDecoder {
    buf: Vec<u8>,
    ready: VecDeque<String>,
    finished: bool,
    /// 지금까지 받은 마지막 `usageMetadata`(입력, 출력+생각). 매 이벤트에 누적값으로 온다.
    usage: Option<(u64, u64)>,
    meter: Option<Arc<TokenMeter>>,
}

impl Drop for SseDecoder {
    /// 끝까지 받았든, 실패했든, 헤징에서 져서 버려졌든 본 사용량은 한 번 기록한다.
    fn drop(&mut self) {
        if let (Some(meter), Some((input, output))) = (&self.meter, self.usage) {
            meter.add(input, output);
        }
    }
}

impl SseDecoder {
    fn metered(meter: Arc<TokenMeter>) -> Self {
        let mut d = SseDecoder::default();
        d.meter = Some(meter);
        d
    }

    fn push(&mut self, chunk: &[u8]) -> Result<(), TranslateError> {
        self.buf.extend_from_slice(chunk);
        while let Some((end, sep)) = find_event_end(&self.buf) {
            let event: Vec<u8> = self.buf.drain(..end + sep).collect();
            let event = std::str::from_utf8(&event[..end])
                .map_err(|_| TranslateError("invalid translation event encoding".into()))?;
            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| line.strip_prefix(' ').unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&data)
                .map_err(|_| TranslateError("invalid translation event JSON".into()))?;
            if let Some(u) = value.get("usageMetadata") {
                let count = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                self.usage = Some((
                    count("promptTokenCount"),
                    count("candidatesTokenCount") + count("thoughtsTokenCount"),
                ));
            }
            if value.get("error").is_some()
                || value.pointer("/promptFeedback/blockReason").is_some()
            {
                return Err(TranslateError(
                    "translation provider rejected the request".into(),
                ));
            }
            let candidate = value.pointer("/candidates/0");
            if let Some(reason) = candidate.and_then(|c| c.get("finishReason")) {
                if reason.as_str() != Some("STOP") {
                    // 공급자 메시지는 원문을 포함할 수 있어 오류에 넣지 않는다.
                    return Err(TranslateError(
                        "translation ended without completing".into(),
                    ));
                }
                self.finished = true;
            }
            let parts = candidate
                .and_then(|c| c.pointer("/content/parts"))
                .and_then(Value::as_array);
            for part in parts.into_iter().flatten() {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                if let Some(text) = part
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    self.ready.push_back(text.to_owned());
                }
            }
        }
        Ok(())
    }

    fn finish(&self) -> Result<(), TranslateError> {
        if !self.finished || self.buf.iter().any(|b| !b.is_ascii_whitespace()) {
            return Err(TranslateError("incomplete translation stream".into()));
        }
        Ok(())
    }
}

fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_events_across_chunks() {
        let mut decoder = SseDecoder::default();
        let event = |t: &str| {
            format!(r#"data: {{"candidates":[{{"content":{{"parts":[{{"text":"{t}"}}]}}}}]}}"#)
        };
        let raw = format!(
            "{}\n\ndata: {{\"candidates\":[{{\"content\":{{\"parts\":[{{\"text\":\"skip\",\"thought\":true}}]}}}}]}}\r\n\r\n{}\n\n",
            event("저희 "),
            event("투자팀은")
        );
        let bytes = raw.as_bytes();
        // 멀티바이트 문자 중간에서 잘린 조각도 이어 붙여 해석해야 한다.
        let cut = bytes.iter().position(|&b| b >= 0x80).unwrap() + 1;
        decoder.push(&bytes[..cut]).unwrap();
        assert!(decoder.ready.is_empty());
        decoder.push(&bytes[cut..]).unwrap();
        assert_eq!(decoder.ready, ["저희 ", "투자팀은"]);
        assert!(decoder.buf.is_empty());
    }

    fn event(value: Value) -> Vec<u8> {
        format!("data: {value}\n\n").into_bytes()
    }

    #[test]
    fn only_a_complete_stop_is_successful() {
        let mut decoder = SseDecoder::default();
        decoder
            .push(&event(
                json!({"candidates": [{"content": {"parts": [{"text": "Hello"}]}}]}),
            ))
            .unwrap();
        assert!(decoder.finish().is_err());
        decoder
            .push(&event(json!({"candidates": [{"finishReason": "STOP"}]})))
            .unwrap();
        decoder.finish().unwrap();
        assert_eq!(decoder.ready, ["Hello"]);
        decoder.push(b"data: {\"candidates\":").unwrap();
        assert!(decoder.finish().is_err());
    }

    #[test]
    fn partial_text_cannot_hide_provider_or_event_failures() {
        for tail in [
            event(json!({"candidates": [{"finishReason": "MAX_TOKENS"}]})),
            event(json!({"candidates": [{"finishReason": "SAFETY"}]})),
            event(json!({"error": {"message": "private source text"}})),
            event(json!({"promptFeedback": {"blockReason": "SAFETY"}})),
            b"data: not JSON\n\n".to_vec(),
        ] {
            let mut decoder = SseDecoder::default();
            decoder
                .push(&event(
                    json!({"candidates": [{"content": {"parts": [{"text": "partial"}]}}]}),
                ))
                .unwrap();
            let err = decoder.push(&tail).unwrap_err();
            assert!(!err.0.contains("private source text"));
        }
    }

    #[test]
    fn usage_is_recorded_once_even_when_the_stream_is_dropped() {
        let meter = Arc::new(TokenMeter::default());
        let mut decoder = SseDecoder::metered(meter.clone());
        let usage = |out: u64| json!({"promptTokenCount": 97, "candidatesTokenCount": out, "thoughtsTokenCount": 1});
        decoder
            .push(&event(json!({"candidates": [{"content": {"parts": [{"text": "Hi"}]}}], "usageMetadata": usage(5)})))
            .unwrap();
        // 누적값이라 마지막 것만 센다.
        decoder.push(&event(json!({"candidates": [{"finishReason": "STOP"}], "usageMetadata": usage(19)}))).unwrap();
        drop(decoder);
        assert_eq!(meter.take(), (97, 20));
    }

    #[test]
    fn prompt_includes_context_and_progress() {
        let body = Gemini::body(&TranslateRequest {
            src_lang: "ja".into(),
            tgt_lang: "ko".into(),
            text: "主にAI".into(),
            complete: false,
            context: vec![("はい。".into(), "네.".into())],
        });
        let system = body
            .pointer("/systemInstruction/parts/0/text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(system.contains("from Japanese to Korean"));
        let user = body
            .pointer("/contents/0/parts/0/text")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(
            user,
            "Previous lines:\nはい。 => 네.\n\nCurrent utterance (in progress):\n主にAI"
        );
        assert_eq!(
            body.pointer("/generationConfig/thinkingConfig/thinkingLevel")
                .unwrap(),
            "minimal"
        );
    }
}
