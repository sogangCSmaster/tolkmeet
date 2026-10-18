//! Gemini 하루 지출 차단기(docs/LAUNCH_PLAN.md 4절).
//!
//! 주기 작업(1분)마다 그사이 쓴 양을 UTC 날짜·종류별로 `gemini_usage`에 더하고, 오늘 추정 지출을 계산한다.
//! - 음성 인식: 브라우저가 Gemini Live에 직접 연결해 서버는 오디오 토큰을 못 본다. 잰 번역 시간(누군가 말한 시간,
//!   함께 말하면 사람 수만큼)으로 추정한다. 토큰 발급 수도 기록만 한다.
//! - 번역: 잰 시간 × 번역 언어 수로 추정한 값과 응답의 토큰 사용량으로 계산한 값 중 큰 쪽(적게 잡지 않게).
//!
//! `DAILY_SPEND_LIMIT_USD`를 넘으면 모든 방의 새 음성 인식 토큰과 번역을 멈추고(`Rooms::set_spend_paused`)
//! 운영자에게 메일을 한 번 보낸다. 다음 UTC 날짜에는 오늘 지출이 0부터 다시 쌓이므로 저절로 풀린다. 상한이 없으면 꺼져 있다.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Db;
use crate::mail::{Copy, Lang, Mailer};
use crate::room::{Metered, Rooms};

/// 단가(USD). 기본값은 docs/ARCHITECTURE.md의 실측(말하는 1분당 STT $0.009 + 번역 $0.003)과
/// docs/benchmarks/2026-10-16-gemini.md의 호출당 비용(입력 ~97·출력 ~19토큰에 $0.000076)이다.
#[derive(Clone, Copy, Debug)]
pub struct Prices {
    pub stt_per_min: f64,
    pub translate_per_lang_min: f64,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

impl Default for Prices {
    fn default() -> Self {
        Prices { stt_per_min: 0.009, translate_per_lang_min: 0.003, input_per_mtok: 0.40, output_per_mtok: 2.00 }
    }
}

/// 번역 응답의 토큰 사용량. 번역기가 응답마다 더하고 주기 작업이 가져간다(잠금 없이).
#[derive(Default)]
pub struct TokenMeter {
    input: AtomicU64,
    output: AtomicU64,
}

impl TokenMeter {
    pub fn add(&self, input: u64, output: u64) {
        self.input.fetch_add(input, Ordering::Relaxed);
        self.output.fetch_add(output, Ordering::Relaxed);
    }

    pub(crate) fn take(&self) -> (u64, u64) {
        (self.input.swap(0, Ordering::Relaxed), self.output.swap(0, Ordering::Relaxed))
    }
}

/// 하루(또는 아직 기록하지 않은) 사용량.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub stt_seconds: i64,
    pub translate_lang_seconds: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub stt_sessions: i64,
}

impl Usage {
    fn fields(&self) -> [(&'static str, i64); 5] {
        [
            ("stt_seconds", self.stt_seconds),
            ("translate_lang_seconds", self.translate_lang_seconds),
            ("input_tokens", self.input_tokens),
            ("output_tokens", self.output_tokens),
            ("stt_sessions", self.stt_sessions),
        ]
    }

    fn add(&mut self, o: &Usage) {
        self.stt_seconds += o.stt_seconds;
        self.translate_lang_seconds += o.translate_lang_seconds;
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.stt_sessions += o.stt_sessions;
    }

    pub fn cost(&self, p: &Prices) -> f64 {
        let stt = self.stt_seconds as f64 / 60.0 * p.stt_per_min;
        let by_time = self.translate_lang_seconds as f64 / 60.0 * p.translate_per_lang_min;
        let by_tokens =
            (self.input_tokens as f64 * p.input_per_mtok + self.output_tokens as f64 * p.output_per_mtok) / 1e6;
        stt + by_time.max(by_tokens)
    }
}

pub struct Budget {
    /// 하루 상한(USD). None이면 차단기는 꺼져 있다(기록·로그는 한다).
    limit: Option<f64>,
    prices: Prices,
    pub tokens: std::sync::Arc<TokenMeter>,
    /// 아직 DB에 더하지 못한 양(기록이 실패하면 다음 주기에 다시 더한다)
    pending: Mutex<Usage>,
    /// 마지막으로 기록한 UTC 날짜와 그날 합계. 날짜가 바뀌면 전날 합계를 로그로 남기고,
    /// DB 기록이 실패하면 이것 + 기록 못 한 양으로 판단한다(실패한다고 차단기가 풀리지 않게).
    day: Mutex<Option<(String, Usage)>>,
}

impl Budget {
    pub fn new(limit: Option<f64>, prices: Prices) -> Self {
        Budget {
            limit,
            prices,
            tokens: Default::default(),
            pending: Mutex::default(),
            day: Mutex::default(),
        }
    }

    /// `DAILY_SPEND_LIMIT_USD`, `GEMINI_PRICE_*`. 잘못된 값은 시작을 멈춘다(상한이 조용히 꺼지지 않게).
    pub fn from_env() -> Self {
        let num = |k: &str| -> Option<f64> {
            let v = std::env::var(k).ok().filter(|v| !v.trim().is_empty())?;
            match v.trim().parse::<f64>() {
                Ok(n) if n.is_finite() && n >= 0.0 => Some(n),
                _ => panic!("{k} must be a non-negative number"),
            }
        };
        let d = Prices::default();
        let prices = Prices {
            stt_per_min: num("GEMINI_PRICE_STT_PER_MIN").unwrap_or(d.stt_per_min),
            translate_per_lang_min: num("GEMINI_PRICE_TRANSLATE_PER_MIN").unwrap_or(d.translate_per_lang_min),
            input_per_mtok: num("GEMINI_PRICE_INPUT_PER_MTOK").unwrap_or(d.input_per_mtok),
            output_per_mtok: num("GEMINI_PRICE_OUTPUT_PER_MTOK").unwrap_or(d.output_per_mtok),
        };
        let limit = num("DAILY_SPEND_LIMIT_USD");
        match limit {
            Some(l) => tracing::info!(limit_usd = l, ?prices, "daily Gemini spend limit on"),
            None => tracing::warn!("DAILY_SPEND_LIMIT_USD is not set: no daily Gemini spend limit"),
        }
        Budget::new(limit, prices)
    }

    /// 음성 인식 토큰을 하나 발급했다(기록용).
    pub fn stt_issued(&self) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).stt_sessions += 1;
    }

    /// 주기 작업: 잰 시간과 토큰을 기록하고, 오늘 지출이 상한을 넘었으면 멈추고 아니면 푼다.
    pub async fn tick(&self, db: &Db, rooms: &Rooms, mailer: &Mailer, metered: &[Metered], now: i64) {
        let (input, output) = self.tokens.take();
        let delta = {
            let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            p.add(&measured(metered));
            p.input_tokens += input as i64;
            p.output_tokens += output as i64;
            std::mem::take(&mut *p)
        };
        let day = utc_day(now);
        let last = self.day.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let prev = last.as_ref().map(|(d, _)| d.clone()).filter(|d| *d != day);
        let limit = self.limit;
        let prices = self.prices;
        let d = day.clone();
        let res = db
            .call(move |c| {
                let tx = c.transaction()?;
                add_usage(&tx, &d, &delta)?;
                let today = load(&tx, &d)?;
                let yesterday = prev.map(|p| load(&tx, &p).map(|u| (p, u))).transpose()?;
                // 상한마다 한 번만 알린다(같은 날 상한을 바꿔 다시 넘으면 다시 알린다).
                let alert = match limit {
                    Some(l) if today.cost(&prices) >= l => {
                        // 같은 날 상한을 바꿨는지 알아보는 열쇠로만 쓴다.
                        let cents = (l * 100.0).round() as i64;
                        let changed = tx.execute(
                            "INSERT INTO gemini_usage (day, kind, amount) VALUES (?1, 'alerted', ?2)
                             ON CONFLICT(day, kind) DO UPDATE SET amount = excluded.amount
                             WHERE amount != excluded.amount",
                            params![d, cents],
                        )?;
                        if changed == 1 { Some(operators(&tx)?) } else { None }
                    }
                    _ => None,
                };
                tx.commit()?;
                Ok((today, yesterday, alert))
            })
            .await;
        let (today, yesterday, alert) = match res {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "gemini usage not recorded");
                let mut total = {
                    let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
                    p.add(&delta);
                    *p
                };
                match &last {
                    Some((d, u)) if *d == day => total.add(u),
                    // 다시 시작한 직후라 아직 모르면 읽기라도 해 본다(쓰기만 실패하는 경우: 디스크가 가득 참).
                    _ => {
                        let d = day.clone();
                        if let Ok(u) = db.call(move |c| load(c, &d)).await {
                            total.add(&u);
                        }
                    }
                }
                self.pause_if_over(rooms, &day, total.cost(&self.prices));
                return;
            }
        };
        *self.day.lock().unwrap_or_else(|e| e.into_inner()) = Some((day.clone(), today));
        if let Some((d, u)) = yesterday {
            log_day(&d, &u, &self.prices, self.limit);
        }
        let spent = today.cost(&self.prices);
        self.pause_if_over(rooms, &day, spent);
        if let (Some(to), Some(limit)) = (alert, self.limit) {
            if to.is_empty() {
                tracing::warn!("no operator account to tell about the spend limit");
            }
            for (email, locale) in to {
                mailer.send(Copy { lang: Lang::parse(&locale) }.spend_alert(&email, &day, &usd(spent), &usd(limit)));
            }
        }
    }
}

impl Budget {
    fn pause_if_over(&self, rooms: &Rooms, day: &str, spent: f64) {
        let over = self.limit.is_some_and(|l| spent >= l);
        if rooms.set_spend_paused(over) {
            if over {
                tracing::warn!(day, spent_usd = spent, limit_usd = self.limit, "daily Gemini spend limit reached: captions paused");
            } else {
                tracing::info!(day, "daily Gemini spend limit cleared: captions resumed");
            }
        }
    }
}

/// 잰 번역 시간 → 음성 인식 초, 번역 언어·초. 언어가 하나뿐인 방도 번역이 켜져 있으면 한 언어로 센다.
fn measured(metered: &[Metered]) -> Usage {
    let mut u = Usage::default();
    for m in metered {
        u.stt_seconds += m.seconds;
        u.translate_lang_seconds += m.seconds * m.langs.saturating_sub(1).max(1) as i64;
    }
    u
}

fn add_usage(c: &Connection, day: &str, u: &Usage) -> rusqlite::Result<()> {
    for (kind, amount) in u.fields() {
        if amount != 0 {
            c.execute(
                "INSERT INTO gemini_usage (day, kind, amount) VALUES (?1, ?2, ?3)
                 ON CONFLICT(day, kind) DO UPDATE SET amount = amount + excluded.amount",
                params![day, kind, amount],
            )?;
        }
    }
    Ok(())
}

fn load(c: &Connection, day: &str) -> rusqlite::Result<Usage> {
    let get = |kind: &str| -> rusqlite::Result<i64> {
        c.query_row(
            "SELECT amount FROM gemini_usage WHERE day = ?1 AND kind = ?2",
            params![day, kind],
            |r| r.get(0),
        )
        .optional()
        .map(|v| v.unwrap_or(0))
    };
    Ok(Usage {
        stt_seconds: get("stt_seconds")?,
        translate_lang_seconds: get("translate_lang_seconds")?,
        input_tokens: get("input_tokens")?,
        output_tokens: get("output_tokens")?,
        stt_sessions: get("stt_sessions")?,
    })
}

fn operators(c: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = c.prepare("SELECT email, locale FROM users WHERE operator = 1")?;
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

fn log_day(day: &str, u: &Usage, p: &Prices, limit: Option<f64>) {
    tracing::info!(
        day,
        spent_usd = u.cost(p),
        limit_usd = limit,
        stt_minutes = u.stt_seconds / 60,
        translate_lang_minutes = u.translate_lang_seconds / 60,
        input_tokens = u.input_tokens,
        output_tokens = u.output_tokens,
        stt_sessions = u.stt_sessions,
        "daily Gemini usage"
    );
}

fn usd(v: f64) -> String {
    format!("${v:.2}")
}

/// `2026-10-18`(UTC)
fn utc_day(now: i64) -> String {
    let t = UNIX_EPOCH + Duration::from_secs(now.max(0) as u64);
    crate::providers::live::rfc3339(t)[..10].to_owned()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::protocol::{NoticeCode, RoomSettings};

    const DAY: i64 = 86_400;
    /// 2026-10-18T12:00:00Z
    const NOW: i64 = 1_792_324_800;

    fn metered(code: &str, seconds: i64, langs: usize) -> Vec<Metered> {
        vec![Metered { host: "1".into(), code: code.into(), seconds, peak: 2, langs }]
    }

    #[test]
    fn cost_uses_the_larger_translation_estimate() {
        let p = Prices::default();
        let by_time = Usage { stt_seconds: 600, translate_lang_seconds: 600, ..Default::default() };
        assert!((by_time.cost(&p) - 0.12).abs() < 1e-9);
        // 토큰으로 센 번역이 더 크면 그쪽을 쓴다.
        let tokens = Usage { output_tokens: 1_000_000, ..by_time };
        assert!((tokens.cost(&p) - (0.09 + 2.0)).abs() < 1e-9);
        assert_eq!(measured(&metered("a", 60, 3)).translate_lang_seconds, 120);
        assert_eq!(measured(&metered("a", 60, 1)).translate_lang_seconds, 60);
        assert_eq!(utc_day(NOW), "2026-10-18");
    }

    #[tokio::test]
    async fn over_the_limit_denies_new_stt_tokens_until_the_next_utc_day() {
        let db = Db::memory();
        db.blocking(|c| {
            c.execute(
                "INSERT INTO users (email, operator, locale, created_at) VALUES ('op@tolkmeet.test', 1, 'ko', 1)",
                [],
            )
        })
        .unwrap();
        let rooms = Rooms::default();
        let code = rooms.create("1", RoomSettings { translation: true }).unwrap();
        // 상한 $0.10: 말하는 1분 $0.012.
        let budget = Budget::new(Some(0.10), Prices::default());
        let mailer = Mailer::off();

        budget.tick(&db, &rooms, &mailer, &metered(&code, 5 * 60, 2), NOW).await;
        assert!(!rooms.spend_paused());
        assert_eq!(rooms.stt_blocked(&code, Instant::now()), None);

        // 번역 토큰까지 더해져 $0.10을 넘는다.
        budget.tokens.add(0, 30_000);
        budget.tick(&db, &rooms, &mailer, &metered(&code, 3 * 60, 2), NOW + 60).await;
        assert!(rooms.spend_paused());
        assert_eq!(rooms.stt_blocked(&code, Instant::now()), Some(NoticeCode::SpendLimit));
        let alerted = |d: &'static str| {
            db.blocking(move |c| {
                c.query_row("SELECT count(*) FROM gemini_usage WHERE day = ?1 AND kind = 'alerted'", params![d], |r| {
                    r.get::<_, i64>(0)
                })
            })
            .unwrap()
        };
        assert_eq!(alerted("2026-10-18"), 1);

        // 같은 날에는 계속 멈춰 있고 메일은 다시 보내지 않는다(알림 표시를 다시 쓰지 않는다).
        budget.tick(&db, &rooms, &mailer, &[], NOW + 120).await;
        assert!(rooms.spend_paused());

        // 다음 UTC 날짜에는 풀린다.
        budget.tick(&db, &rooms, &mailer, &[], NOW + DAY).await;
        assert!(!rooms.spend_paused());
        assert_eq!(rooms.stt_blocked(&code, Instant::now()), None);
        assert_eq!(alerted("2026-10-19"), 0);
        let (today, yesterday) = db
            .blocking(|c| Ok((load(c, "2026-10-19")?, load(c, "2026-10-18")?)))
            .unwrap();
        assert_eq!(today, Usage::default());
        assert_eq!(yesterday.stt_seconds, 8 * 60);
        assert_eq!(yesterday.output_tokens, 30_000);
    }

    #[tokio::test]
    async fn failed_writes_still_pause_and_a_restart_does_not_mail_again() {
        let db = Db::memory();
        let rooms = Rooms::default();
        let budget = Budget::new(Some(0.10), Prices::default());
        budget.tick(&db, &rooms, &Mailer::off(), &metered("x", 5 * 60, 2), NOW).await;
        assert!(!rooms.spend_paused());
        // 기록이 실패해도 마지막 합계 + 기록 못 한 양으로 판단해 멈춘다.
        db.blocking(|c| c.execute_batch("ALTER TABLE gemini_usage RENAME TO broken")).unwrap();
        budget.tick(&db, &rooms, &Mailer::off(), &metered("x", 5 * 60, 2), NOW + 60).await;
        assert!(rooms.spend_paused());
        db.blocking(|c| c.execute_batch("ALTER TABLE broken RENAME TO gemini_usage")).unwrap();
        budget.tick(&db, &rooms, &Mailer::off(), &[], NOW + 120).await;
        assert!(rooms.spend_paused());
        // 다시 시작해도(새 Budget) 첫 주기에 바로 멈추고, 알림 표시는 그대로라 메일을 다시 보내지 않는다.
        let rooms = Rooms::default();
        let restarted = Budget::new(Some(0.10), Prices::default());
        restarted.tick(&db, &rooms, &Mailer::off(), &[], NOW + 180).await;
        assert!(rooms.spend_paused());
        let alerts: i64 = db
            .blocking(|c| c.query_row("SELECT amount FROM gemini_usage WHERE kind = 'alerted'", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(alerts, 10);
    }

    #[tokio::test]
    async fn no_limit_never_pauses_and_failed_writes_are_retried() {
        let db = Db::memory();
        let rooms = Rooms::default();
        let budget = Budget::new(None, Prices::default());
        budget.stt_issued();
        budget.tick(&db, &rooms, &Mailer::off(), &metered("x", 100 * 3600, 2), NOW).await;
        assert!(!rooms.spend_paused());
        // 기록이 실패하면 다음 주기에 다시 더한다.
        db.blocking(|c| c.execute_batch("DROP TABLE gemini_usage")).unwrap();
        budget.tick(&db, &rooms, &Mailer::off(), &metered("x", 60, 2), NOW + 60).await;
        assert_eq!(budget.pending.lock().unwrap().stt_seconds, 60);
    }
}
