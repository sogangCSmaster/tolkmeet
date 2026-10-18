//! 브라우저 음성 인식(Gemini Live)용 ephemeral token 발급.
//!
//! 브라우저는 서버 API 키 대신 이 토큰으로 `BidiGenerateContentConstrained`에 직접 연결한다.
//! 토큰은 1회용이고, 모델·전사 설정·수동 활동 모드를 `fieldMask`로 잠가서 클라이언트가 setup으로
//! 다른 모델이나 출력(오디오 생성 등)을 고를 수 없다(실측: 다른 모델·AUDIO를 요청해도 잠긴 값으로 동작).
//! 시스템 지시·도구도 비운 채로 잠근다. transcribe 모델은 텍스트 요청에 답하지 않지만(실측), 겹으로 막는다.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

const API: &str = "https://generativelanguage.googleapis.com/v1alpha";
pub const DEFAULT_MODEL: &str = "gemini-3.5-transcribe-live";
/// 토큰을 받은 뒤 이 안에 세션을 열어야 한다. 브라우저는 받자마자 연결한다.
const NEW_SESSION_WITHIN: Duration = Duration::from_secs(60);
/// 세션이 메시지를 보낼 수 있는 기간. 오디오 세션 상한(15분)과 같게 두고, 클라이언트는 그 전에 교체한다.
const SESSION_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// 토큰의 setup에서 클라이언트가 바꿀 수 없게 잠그는 필드
const LOCKED_FIELDS: &str = "model,generationConfig.responseModalities,inputAudioTranscription,\
realtimeInputConfig,systemInstruction,tools";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SttGrant {
    /// `auth_tokens/...`. 브라우저가 `access_token` 쿼리로 쓴다.
    pub token: String,
    /// `models/...`. 브라우저 setup에 그대로 넣는다(잠겨 있어 다른 값은 무시된다).
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct TokenError(pub String);

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait SttTokens: Send + Sync {
    /// `langs`(BCP-47, 내 언어 먼저)로 인식하는 1회용 세션 토큰을 만든다.
    fn issue(&self, langs: &[String]) -> BoxFuture<'static, Result<SttGrant, TokenError>>;
}

pub struct GeminiLive {
    client: reqwest::Client,
    key: String,
    model: String,
}

impl GeminiLive {
    /// 번역기와 같은 `client`를 넘기면 이미 열린 HTTP/2 연결을 재사용해 발급 지연이 줄어든다.
    pub fn new(client: reqwest::Client, key: String, model: String) -> Self {
        let model = if model.starts_with("models/") {
            model
        } else {
            format!("models/{model}")
        };
        GeminiLive { client, key, model }
    }

    fn body(&self, langs: &[String], now: SystemTime) -> Value {
        json!({
            "uses": 1,
            "expireTime": rfc3339(now + SESSION_LIFETIME),
            "newSessionExpireTime": rfc3339(now + NEW_SESSION_WITHIN),
            "bidiGenerateContentSetup": {
                "model": self.model,
                "generationConfig": { "responseModalities": ["TEXT"] },
                // 실측(2026-10-17): 내 언어 하나로만 잠그면 다른 언어로 한 말(한국식 발음의 일본어·영어)을 내 언어
                // 글자로 소리 나는 대로 적는다("요로시쿠 오네가이시마스"). 상대 언어까지 함께 주면 그 언어 글자로 적고,
                // 내 언어 인식은 그대로다.
                "inputAudioTranscription": { "languageCodes": langs },
                // 클라이언트 VAD가 activityStart/End를 보낸다. 자동 모드는 침묵을 보내지 않으면 확정이 오지 않는다.
                "realtimeInputConfig": { "automaticActivityDetection": { "disabled": true } },
            },
            "fieldMask": LOCKED_FIELDS,
        })
    }
}

impl SttTokens for GeminiLive {
    fn issue(&self, langs: &[String]) -> BoxFuture<'static, Result<SttGrant, TokenError>> {
        let request = self
            .client
            .post(format!("{API}/auth_tokens"))
            .header("x-goog-api-key", &self.key)
            .timeout(Duration::from_secs(10))
            .json(&self.body(langs, SystemTime::now()));
        let model = self.model.clone();
        async move {
            let res = request
                .send()
                .await
                .map_err(|e| TokenError(e.without_url().to_string()))?;
            if !res.status().is_success() {
                // 본문은 로그에 남기지 않는다.
                return Err(TokenError(format!("HTTP {}", res.status())));
            }
            let body: Value = res
                .json()
                .await
                .map_err(|e| TokenError(e.without_url().to_string()))?;
            let token = body
                .get("name")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| TokenError("response has no token".into()))?;
            Ok(SttGrant {
                token: token.to_owned(),
                model,
            })
        }
        .boxed()
    }
}

/// `2026-10-16T09:30:00Z` 형식(UTC, 초 단위). 날짜 라이브러리 없이 일 수 → 연월일 변환만 한다.
pub(crate) fn rfc3339(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant의 civil_from_days
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

#[cfg(test)]
pub mod testing {
    use std::sync::Mutex;

    use super::*;

    /// 요청 언어를 기록하고 `fail`이면 실패하는 가짜 발급기.
    #[derive(Default)]
    pub struct FakeTokens {
        pub langs: Mutex<Vec<String>>,
        pub fail: bool,
    }

    impl SttTokens for FakeTokens {
        fn issue(&self, langs: &[String]) -> BoxFuture<'static, Result<SttGrant, TokenError>> {
            let lang = langs.join("+");
            self.langs.lock().unwrap().push(lang.clone());
            let result = if self.fail {
                Err(TokenError("fake failure".into()))
            } else {
                Ok(SttGrant {
                    token: format!("auth_tokens/fake-{lang}"),
                    model: format!("models/{DEFAULT_MODEL}"),
                })
            };
            async move { result }.boxed()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(1_792_142_999)),
            "2026-10-16T09:29:59Z"
        );
    }

    #[test]
    fn token_locks_model_language_and_manual_activity() {
        let live = GeminiLive::new(reqwest::Client::new(), "k".into(), DEFAULT_MODEL.into());
        let now = UNIX_EPOCH + Duration::from_secs(1_792_142_999);
        let body = live.body(&["ja".to_owned(), "ko".to_owned()], now);
        assert_eq!(body["uses"], 1);
        assert_eq!(body["newSessionExpireTime"], "2026-10-16T09:30:59Z");
        assert_eq!(body["expireTime"], "2026-10-16T09:44:59Z");
        let setup = &body["bidiGenerateContentSetup"];
        assert_eq!(setup["model"], "models/gemini-3.5-transcribe-live");
        assert_eq!(setup["inputAudioTranscription"]["languageCodes"], json!(["ja", "ko"]));
        assert_eq!(
            setup["realtimeInputConfig"]["automaticActivityDetection"]["disabled"],
            true
        );
        let locked: Vec<&str> = body["fieldMask"].as_str().unwrap().split(',').collect();
        for field in [
            "model",
            "inputAudioTranscription",
            "realtimeInputConfig",
            "systemInstruction",
            "tools",
        ] {
            assert!(locked.contains(&field), "{field} must be locked");
        }
        // 잠그는 필드 중 값을 주는 것은 모두 setup에 있어야 한다(시스템 지시·도구는 비운 채로 잠금).
        for field in &locked {
            let top = field.split('.').next().unwrap();
            assert!(
                setup.get(top).is_some() || ["systemInstruction", "tools"].contains(&top),
                "{field} is locked but not set"
            );
        }
    }
}
