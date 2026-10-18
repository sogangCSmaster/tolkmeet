//! 결제(Paddle, 판매 대행). 결제창은 브라우저의 Paddle.js가 열고, 서버는 웹훅으로 구독 상태를 받아
//! 계정의 요금제를 바꾼다. 해지·요금제 변경·결제 수단 관리는 Paddle API로 한다.
//! 키(PADDLE_*)가 없으면 결제 기능은 꺼진다. 개발 모드에서는 결제 없이 요금제를 켜 보는 경로를 둔다.

use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::{Value, json};

use crate::db::{Db, OptionalExtension, now_secs, params};

/// 요금제: 결제 기간(한 달)마다 포함되는 번역 시간(초). 요금제가 없으면 0(사 둔 추가 시간만 쓴다).
pub fn included_seconds(plan: &str) -> i64 {
    match plan {
        "basic" => 8 * 3600,
        "pro" => 20 * 3600,
        _ => 0,
    }
}

/// 일일권 하나에 든 번역 시간과 쓸 수 있는 기간(산 때부터).
pub const DAY_PASS_SECONDS: i64 = 3 * 3600;
pub const DAY_PASS_LENGTH: i64 = 86_400;

/// 화면·API에 보이는 요금제: 구독이 없어도 일일권이 남아 있으면 "day". 구독 상태(`users.plan`)는 그대로 둔다.
/// `users` 표의 열 이름을 그대로 쓰는 SQL 식이다.
pub const EFFECTIVE_PLAN_SQL: &str = "CASE WHEN plan = 'none' AND day_until > unixepoch() THEN 'day' ELSE plan END";

/// 결제 기간을 모를 때(개발 모드, 기간이 빠진 웹훅) 쓰는 한 기간의 길이.
pub const DEFAULT_PERIOD: i64 = 30 * 86_400;

/// 같은 결제 기간 안에서 요금제를 바꾼 뒤 이번 기간에 포함된 시간. 올리면 남은 기간의 비율만큼 두 요금제의
/// 차이를 더한다(Paddle이 남은 기간만큼 차액을 받는다). 내리면 그대로 둔다(이번 기간은 이미 비싼 요금제로 냈다).
pub fn included_after_change(current: i64, old_plan: &str, new_plan: &str, period: (i64, i64), at: i64) -> i64 {
    let diff = included_seconds(new_plan) - included_seconds(old_plan);
    let (start, end) = period;
    if diff <= 0 || end <= start {
        return current;
    }
    current + diff * (end - at).clamp(0, end - start) / (end - start)
}

pub struct Billing {
    api_key: String,
    pub client_token: String,
    webhook_secret: String,
    pub sandbox: bool,
    pub price_basic: String,
    pub price_pro: String,
    pub price_hour: String,
    pub price_day: String,
    http: reqwest::Client,
}

impl Billing {
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        // 몇 개만 있으면 결제가 조용히 꺼진다(구독 웹훅도 반영하지 않는다). 빠진 이름을 크게 남긴다.
        const KEYS: [&str; 7] = [
            "PADDLE_API_KEY",
            "PADDLE_CLIENT_TOKEN",
            "PADDLE_WEBHOOK_SECRET",
            "PADDLE_PRICE_BASIC",
            "PADDLE_PRICE_PRO",
            "PADDLE_PRICE_HOUR",
            "PADDLE_PRICE_DAY",
        ];
        let missing: Vec<&str> = KEYS.into_iter().filter(|k| var(k).is_none()).collect();
        if !missing.is_empty() && missing.len() < KEYS.len() {
            tracing::error!(?missing, "some PADDLE_* settings are missing: billing is off");
        }
        let billing = Self {
            api_key: var("PADDLE_API_KEY")?,
            client_token: var("PADDLE_CLIENT_TOKEN")?,
            webhook_secret: var("PADDLE_WEBHOOK_SECRET")?,
            sandbox: var("PADDLE_ENV").as_deref() != Some("production"),
            price_basic: var("PADDLE_PRICE_BASIC")?,
            price_pro: var("PADDLE_PRICE_PRO")?,
            price_hour: var("PADDLE_PRICE_HOUR")?,
            price_day: var("PADDLE_PRICE_DAY")?,
            http: reqwest::Client::new(),
        };
        Some(billing)
    }

    fn api(&self) -> &'static str {
        if self.sandbox {
            "https://sandbox-api.paddle.com"
        } else {
            "https://api.paddle.com"
        }
    }

    pub fn price_for(&self, plan: &str) -> Option<&str> {
        match plan {
            "basic" => Some(&self.price_basic),
            "pro" => Some(&self.price_pro),
            _ => None,
        }
    }

    fn plan_for_price(&self, price: &str) -> Option<&'static str> {
        if price == self.price_basic {
            Some("basic")
        } else if price == self.price_pro {
            Some("pro")
        } else {
            None
        }
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.api()))
            .bearer_auth(&self.api_key)
            .timeout(Duration::from_secs(20));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.map_err(|_| ApiError {
            status: 0,
            code: "network".into(),
        })?;
        let status = res.status();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(body)
        } else {
            Err(ApiError {
                status: status.as_u16(),
                code: body["error"]["code"].as_str().unwrap_or("").to_owned(),
            })
        }
    }

    /// 결제창에 넘길 사용자 서명(웹훅에서 사용자 id를 믿을 수 있게).
    pub fn sign_user(&self, uid: i64) -> String {
        let mut mac = Hmac::<Sha256v10>::new_from_slice(self.webhook_secret.as_bytes())
            .expect("any key length");
        mac.update(format!("tolkmeet-user:{uid}").as_bytes());
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn check_user(&self, uid: i64, sig: &str) -> bool {
        constant_eq(self.sign_user(uid).as_bytes(), sig.as_bytes())
    }

    /// 바로 해지(남은 기간 없이). 계정 삭제·새 구독에 밀린 예전 구독·환불된 구독.
    /// 이미 해지됐거나 없는 구독이면 할 일이 없으므로 성공으로 본다(다시 구독할 때·계정을 지울 때 막히지 않게).
    pub async fn cancel_now(&self, subscription: &str) -> Result<(), String> {
        match self
            .call(
                reqwest::Method::POST,
                &format!("/subscriptions/{subscription}/cancel"),
                Some(json!({ "effective_from": "immediately" })),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.already_gone() => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// 이번 결제 기간이 끝나면 해지.
    pub async fn cancel_at_period_end(&self, subscription: &str) -> Result<(), ApiError> {
        self.call(
            reqwest::Method::POST,
            &format!("/subscriptions/{subscription}/cancel"),
            Some(json!({ "effective_from": "next_billing_period" })),
        )
        .await
        .map(|_| ())
    }

    /// 예약된 해지를 취소.
    pub async fn resume(&self, subscription: &str) -> Result<(), ApiError> {
        self.call(
            reqwest::Method::PATCH,
            &format!("/subscriptions/{subscription}"),
            Some(json!({ "scheduled_change": null })),
        )
        .await
        .map(|_| ())
    }

    /// 요금제 바꾸기. 올리면 남은 기간만큼 차액을 바로 받고(포함 시간도 그 비율만큼 더한다), 내리면 이번 기간은
    /// 청구도 환불도 하지 않고 다음 결제부터 싼 요금을 받는다(이번 기간의 포함 시간은 그대로).
    pub async fn change_plan(&self, subscription: &str, plan: &str, upgrade: bool) -> Result<(), ApiError> {
        let price = self.price_for(plan).ok_or(ApiError {
            status: 400,
            code: "plan".into(),
        })?;
        let mode = if upgrade { "prorated_immediately" } else { "do_not_bill" };
        self.call(
            reqwest::Method::PATCH,
            &format!("/subscriptions/{subscription}"),
            Some(json!({ "items": [{ "price_id": price, "quantity": 1 }], "proration_billing_mode": mode })),
        )
        .await
        .map(|_| ())
    }

    /// 결제창에 넘길 거래를 만든다. 사용자 서명은 서버가 거래에 넣는다(구독은 거래의 custom_data를 이어 받는다).
    /// 고객·주소가 없으므로 draft로 만들어지고, 결제창이 나머지를 받는다.
    pub async fn create_transaction(&self, price: &str, quantity: i64, uid: i64) -> Result<String, ApiError> {
        let body = self
            .call(
                reqwest::Method::POST,
                "/transactions",
                Some(json!({
                    "items": [{ "price_id": price, "quantity": quantity }],
                    "custom_data": { "user_id": uid.to_string(), "sig": self.sign_user(uid) },
                })),
            )
            .await?;
        body["data"]["id"].as_str().map(str::to_owned).ok_or(ApiError {
            status: 200,
            code: "missing_id".into(),
        })
    }

    /// 거래의 상태와 첫 항목의 가격 id.
    pub async fn transaction_status(&self, id: &str) -> Result<(String, Option<String>), ApiError> {
        let body = self
            .call(reqwest::Method::GET, &format!("/transactions/{id}"), None)
            .await?;
        let data = &body["data"];
        Ok((
            data["status"].as_str().unwrap_or("").to_owned(),
            data["items"][0]["price"]["id"].as_str().map(str::to_owned),
        ))
    }

    /// 아직 결제하지 않은(draft·ready) 거래의 요금제를 바꾼다.
    pub async fn set_transaction_price(&self, id: &str, price: &str) -> Result<(), ApiError> {
        self.call(
            reqwest::Method::PATCH,
            &format!("/transactions/{id}"),
            Some(json!({ "items": [{ "price_id": price, "quantity": 1 }] })),
        )
        .await
        .map(|_| ())
    }

    /// 아직 결제하지 않은 거래를 취소한다(예전 결제창에서 결제되어 구독이 둘 생기지 않게).
    pub async fn cancel_transaction(&self, id: &str) -> Result<(), ApiError> {
        self.call(
            reqwest::Method::PATCH,
            &format!("/transactions/{id}"),
            Some(json!({ "status": "canceled" })),
        )
        .await
        .map(|_| ())
    }

    /// 해지를 기다리는 구독(`queue_cancellation`)을 Paddle에서 해지한다. 웹훅에 답한 뒤와 주기 작업마다 돌고,
    /// Paddle이 확인할 때까지(이미 해지됨 포함) 다음 주기에 다시 시도한다. 한 번에 하나만 돈다.
    pub async fn process_cancellations(&self, db: &Db) {
        static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let Ok(_one_at_a_time) = RUNNING.try_lock() else {
            return;
        };
        let pending = db
            .call(|c| {
                let mut s = c.prepare(
                    "SELECT subscription_id, attempts FROM paddle_cancellations WHERE done = 0 ORDER BY created_at LIMIT 50",
                )?;
                s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await;
        let pending = match pending {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "could not read pending cancellations");
                return;
            }
        };
        for (sub, attempts) in pending {
            let result = self.cancel_now(&sub).await;
            // 처음 실패와 그 뒤 한 시간(60번)마다 운영자가 보게 남긴다.
            if let Err(e) = &result
                && attempts % 60 == 0
            {
                tracing::error!(error = %e, subscription = %sub, attempts, "could not cancel a subscription: will retry");
            }
            let done = result.is_ok();
            let saved = db
                .call(move |c| {
                    if done {
                        confirm_cancellation(c, Some(&sub))
                    } else {
                        c.execute(
                            "UPDATE paddle_cancellations SET attempts = attempts + 1 WHERE subscription_id = ?1",
                            params![sub],
                        )
                        .map(|_| ())
                    }
                })
                .await;
            if let Err(e) = saved {
                tracing::warn!(error = %e, "could not record a cancellation attempt");
            }
        }
    }

    /// 결제 수단·영수증을 보는 Paddle 고객 포털 주소.
    pub async fn portal_url(&self, customer: &str) -> Result<String, String> {
        let body = self
            .call(
                reqwest::Method::POST,
                &format!("/customers/{customer}/portal-sessions"),
                Some(json!({})),
            )
            .await
            .map_err(|e| e.to_string())?;
        body["data"]["urls"]["general"]["overview"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "portal".into())
    }

    /// `Paddle-Signature: ts=...;h1=...` 확인. 서명 대상은 "ts:원문".
    pub fn verify_signature(&self, header: &str, body: &[u8], now: i64) -> bool {
        verify_signature(&self.webhook_secret, header, body, now)
    }

    /// 웹훅 하나를 반영한다. 돌려주는 값: 보낼 영수증, 해지해야 할 구독.
    pub async fn apply_event(&self, db: &Db, event: Value) -> Result<Applied, String> {
        let event = self.parse(&event)?;
        db.call(move |c| event.apply(c, now_secs()))
            .await
            .map_err(|e| e.to_string())
    }

    fn parse(&self, event: &Value) -> Result<Event, String> {
        let data = &event["data"];
        let time = |v: &Value| v.as_str().and_then(parse_rfc3339);
        let text = |v: &Value| v.as_str().map(str::to_owned);
        let minor = |v: &Value| v.as_str().and_then(|s| s.parse::<i64>().ok());
        let items: Vec<&Value> = data["items"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
        let canceling = data["scheduled_change"]["action"].as_str() == Some("cancel");
        Ok(Event {
            id: event["event_id"].as_str().ok_or("event id")?.to_owned(),
            kind: event["event_type"].as_str().unwrap_or("").to_owned(),
            // Paddle은 이벤트 순서를 보장하지 않는다. 일어난 시각(밀리초)으로 늦게 온 옛 이벤트를 가린다.
            occurred_ms: event["occurred_at"].as_str().and_then(parse_rfc3339_ms).unwrap_or(0),
            // 결제창이 넘긴 사용자 id는 브라우저가 바꿀 수 있으므로 서버 서명이 맞을 때만 믿는다.
            custom_user: data["custom_data"]["user_id"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|uid| self.check_user(*uid, data["custom_data"]["sig"].as_str().unwrap_or(""))),
            entity: text(&data["id"]),
            subscription_ref: text(&data["subscription_id"]),
            customer: text(&data["customer_id"]),
            status: data["status"].as_str().unwrap_or("").to_owned(),
            plan: items
                .iter()
                .find_map(|i| i["price"]["id"].as_str().and_then(|p| self.plan_for_price(p))),
            hours: items
                .iter()
                .filter(|i| i["price"]["id"].as_str() == Some(self.price_hour.as_str()))
                .map(|i| i["quantity"].as_i64().unwrap_or(1))
                .sum(),
            day_pass: items.iter().any(|i| i["price"]["id"].as_str() == Some(self.price_day.as_str())),
            // 해지를 예약하면 Paddle은 next_billed_at을 비운다. 그때는 해지되는 날을 끝나는 날로 둔다.
            renews: time(&data["next_billed_at"])
                .or_else(|| canceling.then(|| time(&data["scheduled_change"]["effective_at"])).flatten()),
            canceling,
            period: match (
                time(&data["current_billing_period"]["starts_at"]),
                time(&data["current_billing_period"]["ends_at"]),
            ) {
                (Some(s), Some(e)) if e > s => Some((s, e)),
                _ => None,
            },
            billing_period: match (time(&data["billing_period"]["starts_at"]), time(&data["billing_period"]["ends_at"])) {
                (Some(s), Some(e)) if e > s => Some((s, e)),
                _ => None,
            },
            upgrade_grant: self.upgrade_grant(data),
            total: minor(&data["details"]["totals"]["grand_total"]),
            currency: data["currency_code"].as_str().unwrap_or("USD").to_owned(),
            origin: data["origin"].as_str().unwrap_or("").to_owned(),
            action: data["action"].as_str().unwrap_or("").to_owned(),
            adjusted_transaction: text(&data["transaction_id"]),
            adjusted_full: data["type"].as_str() == Some("full"),
            adjusted_amount: minor(&data["totals"]["total"]).unwrap_or(0),
            compensation: data["reason"].as_str().is_some_and(|r| r.trim_start().to_ascii_lowercase().starts_with("outage")),
        })
    }

    /// 요금제를 올린 차액 거래(origin subscription_update)로 이번 기간에 더할 포함 시간.
    /// Paddle은 새 요금을 남은 기간 비율(proration.rate)만큼 받고 예전 요금을 음수 줄로 돌려준다:
    /// 더할 시간 = Σ(새 요금제 시간 × 비율) − Σ(예전 요금제 시간 × 비율).
    fn upgrade_grant(&self, data: &Value) -> i64 {
        let lines = data["details"]["line_items"]
            .as_array()
            .filter(|a| !a.is_empty())
            .or_else(|| data["items"].as_array());
        let (mut grant, mut credited, mut charged) = (0.0f64, false, None);
        for line in lines.into_iter().flatten() {
            let price = line["price_id"].as_str().or_else(|| line["price"]["id"].as_str());
            let Some(plan) = price.and_then(|p| self.plan_for_price(p)) else {
                continue;
            };
            let negative = line["quantity"].as_i64().is_some_and(|q| q < 0)
                || line["totals"]["subtotal"]
                    .as_str()
                    .and_then(|t| t.parse::<i64>().ok())
                    .is_some_and(|t| t < 0);
            let rate = line["proration"]["rate"]
                .as_str()
                .and_then(|r| r.parse::<f64>().ok())
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            let seconds = included_seconds(plan) as f64 * rate;
            if negative {
                grant -= seconds;
                credited = true;
            } else {
                grant += seconds;
                charged.get_or_insert(rate);
            }
        }
        // 예전 요금 줄이 없으면: 요금제가 둘뿐이라 올리기는 언제나 Basic → Pro다.
        if let (false, Some(rate)) = (credited, charged) {
            grant -= included_seconds("basic") as f64 * rate;
        }
        grant.max(0.0) as i64
    }
}

/// Paddle API 실패. `code`는 Paddle 오류 코드(`error.code`, 예: subscription_update_when_past_due), 연결 실패면 "network".
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
}

impl ApiError {
    /// 이미 해지됐거나 Paddle에 없는 구독
    fn already_gone(&self) -> bool {
        matches!(
            self.code.as_str(),
            "subscription_is_canceled_action_invalid" | "subscription_update_when_canceled" | "not_found"
        )
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "paddle {} {}", self.status, self.code)
    }
}

/// 기다리는 구독 결제 거래를 어떻게 할지(`pending_checkout`).
#[derive(Debug, PartialEq, Eq)]
pub enum PendingCheckout {
    /// 아직 결제하지 않았다: 같은 거래로 결제창을 다시 연다(어느 창에서 결제해도 구독은 하나).
    Reuse,
    /// 이미 결제했다: 웹훅으로 구독이 반영될 때까지 새 결제를 받지 않는다.
    Paid,
    /// 취소됐거나 쓸 수 없다: 새 거래를 만든다.
    Gone,
}

pub fn pending_checkout(status: &str) -> PendingCheckout {
    match status {
        "draft" | "ready" => PendingCheckout::Reuse,
        "billed" | "paid" | "completed" => PendingCheckout::Paid,
        _ => PendingCheckout::Gone,
    }
}

/// 웹훅 한 건에서 반영에 쓰는 값. DB 작업(블로킹 스레드) 전에 읽어 둔다.
struct Event {
    id: String,
    kind: String,
    occurred_ms: i64,
    custom_user: Option<i64>,
    /// data.id: 구독·거래·조정 id
    entity: Option<String>,
    subscription_ref: Option<String>,
    customer: Option<String>,
    status: String,
    plan: Option<&'static str>,
    hours: i64,
    /// 일일권 결제(가격 `price_day`가 들어 있다)
    day_pass: bool,
    renews: Option<i64>,
    canceling: bool,
    /// 구독의 current_billing_period
    period: Option<(i64, i64)>,
    /// 거래의 billing_period(구독 결제면 그 결제로 낸 기간, 끝은 다음 결제일)
    billing_period: Option<(i64, i64)>,
    /// 차액 거래면 이번 기간에 더할 포함 시간(`Billing::upgrade_grant`)
    upgrade_grant: i64,
    /// 거래 금액(가장 작은 화폐 단위)
    total: Option<i64>,
    currency: String,
    origin: String,
    /// 조정(환불·차지백)
    action: String,
    adjusted_transaction: Option<String>,
    adjusted_full: bool,
    adjusted_amount: i64,
    /// 장애 보상 환불(조정 사유가 "outage"로 시작). 운영자가 Paddle에서 환불할 때 사유로 표시한다.
    compensation: bool,
}

impl Event {
    fn apply(self, c: &mut rusqlite::Connection, now: i64) -> rusqlite::Result<Applied> {
        let tx = c.transaction()?;
        // 같은 이벤트가 다시 와도(Paddle 재시도) 한 번만 반영한다.
        if tx.execute(
            "INSERT OR IGNORE INTO paddle_events (id, received_at) VALUES (?1, ?2)",
            params![self.id, now],
        )? == 0
        {
            return Ok(Applied::default());
        }
        // 반영하지 않고 넘기는 이벤트도 받은 것으로 남긴다(재시도 반복 방지).
        let out = if self.kind.starts_with("adjustment.") {
            self.adjust(&tx, now)?
        } else {
            self.update(&tx, now)?
        };
        // 나중에 다시 받을 이벤트는 받은 것으로 남기지 않는다(트랜잭션을 버린다).
        if !out.retry {
            tx.commit()?;
        }
        Ok(out)
    }

    /// 이벤트가 일어난 시각(초). 없으면 받은 시각.
    fn at(&self, now: i64) -> i64 {
        if self.occurred_ms > 0 { self.occurred_ms.div_euclid(1000) } else { now }
    }

    fn live(&self) -> bool {
        matches!(self.status.as_str(), "active" | "trialing" | "past_due")
    }

    fn update(&self, tx: &rusqlite::Transaction, now: i64) -> rusqlite::Result<Applied> {
        // 해지된 구독은 표시해 둔다: 우리가 해지하던 것이면 확인됐고(지금 구독이 아니어도, 계정을 지웠어도),
        // 그 구독의 늦은 이벤트는 더 반영하지 않는다(지운 지금 구독 자리에 다시 들어오지 않게).
        if self.kind == "subscription.canceled"
            && let Some(sub) = &self.entity
        {
            tx.execute(
                "INSERT INTO paddle_cancellations (subscription_id, revoked, done, created_at) VALUES (?1, 1, 1, ?2)
                 ON CONFLICT(subscription_id) DO UPDATE SET revoked = 1, done = 1",
                params![sub, now],
            )?;
        }
        // 사용자: 서버가 서명한 custom_data가 기준이다. 서명이 없으면 구독 이벤트는 구독 id로만,
        // 결제(추가 시간) 이벤트는 고객 id로 이미 묶인 사용자를 찾는다.
        let is_subscription = self.kind.starts_with("subscription.");
        let linked: Option<i64> = if is_subscription {
            tx.query_row(
                "SELECT id FROM users WHERE paddle_subscription_id = ?1",
                params![self.entity],
                |r| r.get(0),
            )
            .optional()?
        } else {
            tx.query_row(
                "SELECT id FROM users WHERE paddle_customer_id = ?1 OR paddle_subscription_id = ?2 LIMIT 1",
                params![self.customer, self.subscription_ref],
                |r| r.get(0),
            )
            .optional()?
        };
        let user = match (self.custom_user, linked) {
            // 이 구독이 이미 다른 사용자에게 묶여 있으면 반영하지 않는다(가로채기 방지).
            (Some(u), Some(l)) if u != l && is_subscription => {
                tracing::warn!(event = %self.id, "paddle event user mismatch: ignored");
                None
            }
            // 결제는 서버가 서명한 사용자가 한 것이다. 같은 Paddle 고객(같은 이메일 등)이 다른 계정에 묶여 있어도 산 계정에 준다.
            (Some(u), Some(l)) if u != l => {
                tracing::warn!(event = %self.id, "paddle customer is linked to another account: credited the signed user");
                Some(u)
            }
            (Some(u), _) => Some(u),
            (None, l) => l,
        };
        let Some(user) = user else {
            if self.kind == "transaction.completed" {
                skip_payment(tx, self.entity.as_deref(), now)?;
            }
            return Ok(Applied::default());
        };
        // 서명된 사용자가 그사이 계정을 지웠으면 반영할 곳이 없다.
        type Account = (Option<String>, i64, String, Option<i64>, Option<i64>, i64, Option<i64>);
        let account: Option<Account> = tx
            .query_row(
                "SELECT paddle_subscription_id, paddle_event_at, plan, period_start, period_end, period_included, period_paid_start
                 FROM users WHERE id = ?1",
                params![user],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .optional()?;
        let Some((current, last_at, old_plan, old_start, old_end, old_included, paid_start)) = account else {
            // 계정을 지운 뒤에 생긴 구독(다른 탭에 열어 둔 결제창, 늦게 끝난 결제)은 결제가 이어지지 않게 해지한다.
            if is_subscription && self.kind != "subscription.canceled" && self.live() && let Some(sub) = &self.entity {
                tracing::warn!(event = %self.id, subscription = %sub, "subscription for a deleted account: queued for cancellation");
                queue_cancellation(tx, sub, false, now)?;
            } else if self.kind == "transaction.completed" {
                // 돌려줄 곳이 없는 결제: 운영자가 Paddle에서 환불한다. 어떤 결제든 표시해 두어 그 환불을 다시 받지 않게 한다.
                if self.hours > 0 || self.plan.is_some() || self.day_pass {
                    tracing::error!(event = %self.id, transaction = ?self.entity, "payment for a deleted account: refund it in Paddle");
                }
                skip_payment(tx, self.entity.as_deref(), now)?;
            } else {
                tracing::info!(event = %self.id, "paddle event for a deleted account: recorded and skipped");
            }
            return Ok(Applied::default());
        };
        let mut out = Applied::default();
        // 구독 이벤트는 그 사용자의 지금 구독에만 반영한다. 새 구독(created·activated)만 지금 구독을 바꿀 수 있고,
        // 이미 반영한 것보다 오래된 이벤트는 버린다.
        if is_subscription {
            // 우리가 해지하는 중이거나 해지한(새 구독에 밀림·환불·계정 삭제) 구독은 다시 살리지 않는다. 해지 확인만 받는다.
            let canceling: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM paddle_cancellations WHERE subscription_id = ?1)",
                params![self.entity],
                |r| r.get(0),
            )?;
            if canceling && self.kind != "subscription.canceled" {
                tracing::info!(event = %self.id, "paddle event for a subscription we cancel: ignored");
                return Ok(Applied::default());
            }
            // 이벤트 순서(`paddle_event_at`)는 지금 구독 안에서만 비교한다(다른 구독의 마지막 이벤트와 비교하면 새 구독을 버린다).
            let same = current.is_some() && current == self.entity;
            let newer = self.occurred_ms >= last_at;
            let starts_new = matches!(
                self.kind.as_str(),
                "subscription.created" | "subscription.activated" | "subscription.trialing"
            );
            // 지금 구독이 없으면(처음이거나 예전 구독이 해지됨) 쓰는 중인 구독을 받는다. 해지된 구독의 늦은 이벤트는
            // 위에서(해지 표시) 이미 걸렀다.
            let adopts = current.is_none() && self.live();
            // 지금 구독이 끝난 상태(환불로 끝냄·멈춤·해지 대기)면 다른 구독의 쓰는 중 이벤트(서명된 것만 여기 온다)는
            // 어느 것이든 그 구독으로 바꾼다(새 구독의 updated가 created보다 먼저 와도 잃지 않게). 그 뒤 같은 구독 안에서는 순서대로.
            let current_ended = current.is_some()
                && (old_plan == "none"
                    || tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM paddle_cancellations WHERE subscription_id = ?1)",
                        params![current],
                        |r| r.get::<_, bool>(0),
                    )?);
            // 쓰는 중인 구독이 둘이면(결제창을 두 번 열어 둔 경우 등) 하나만 둔다: 지금 구독보다 나중에 생긴 쪽으로 바꾸고
            // 예전 것을 해지하며, 먼저 생긴 것이 늦게 도착하면 그것을 해지한다.
            let rival = !same && current.is_some() && self.live() && (starts_new || current_ended);
            let switches = rival && (newer || current_ended);
            // 지금 구독이 이미 끝났으면 먼저 생긴 구독이라도 해지하지 않고 그것으로 바꾼다(결제한 구독을 잃지 않게).
            if rival && !newer && !current_ended && let Some(sub) = &self.entity {
                tracing::warn!(event = %self.id, subscription = %sub, "a second, older subscription: queued for cancellation");
                queue_cancellation(tx, sub, false, now)?;
                return Ok(Applied { replaced: Some(sub.clone()), ..Applied::default() });
            }
            if !((same && newer) || adopts || switches) {
                tracing::info!(event = %self.id, "paddle event for an old subscription or out of order: ignored");
                return Ok(Applied::default());
            }
            // 새 구독으로 바뀌면 예전 구독은 더 이상 추적하지 않으므로 해지해야 한다(두 번 결제되지 않게).
            // 해지된 구독은 canceled 웹훅에서 지워 두므로 여기 남은 것은 아직 쓰는 중이거나 멈춘 구독이다.
            if switches && let Some(old) = &current {
                queue_cancellation(tx, old, false, now)?;
                out.replaced = Some(old.clone());
            }
            tx.execute(
                "UPDATE users SET paddle_event_at = ?2 WHERE id = ?1",
                params![user, self.occurred_ms],
            )?;
        }
        match self.kind.as_str() {
            // Paddle은 상태가 바뀌면 그 상태의 이벤트(activated·past_due·paused·resumed·trialing)를, 그 밖의 변경은 updated를 보낸다.
            // 모두 같은 데이터(구독)이므로 한 곳에서 요금제·기간·상태를 반영한다(멈춤·밀림도 지금 상태로 남긴다).
            "subscription.created" | "subscription.activated" | "subscription.updated" | "subscription.resumed"
            | "subscription.past_due" | "subscription.paused" | "subscription.trialing" | "subscription.imported" => {
                let live = self.live();
                let new_plan: String = match (live, self.plan) {
                    (false, _) => "none".into(),
                    (true, Some(p)) => p.into(),
                    // 모르는 가격(대시보드에서 가격을 바꾸고 서버 설정은 그대로 둔 경우 등): 결제는 이어지므로
                    // 지금 요금제를 그대로 두고 운영자가 보게 남긴다.
                    (true, None) => {
                        tracing::error!(event = %self.id, subscription = ?self.entity, "active subscription with an unknown price: plan kept");
                        old_plan.clone()
                    }
                };
                // 구독 이벤트는 기간만 옮긴다. 포함 시간은 그 기간의 결제가 끝났을 때(transaction.completed) 준다:
                // Paddle은 갱신 결제 전에 기간부터 옮기고, 결제가 밀려도(past_due) 구독은 이어진다.
                let (start, end, included, paid) = if new_plan == "none" {
                    (old_start, old_end, old_included, paid_start)
                } else {
                    let new_sub = current != self.entity;
                    let at = self.at(now);
                    let ours = match (old_start, old_end) {
                        (Some(s), Some(e)) if !new_sub && old_plan != "none" => Some((s, e)),
                        _ => None,
                    };
                    let period = self.period.or(ours).unwrap_or_else(|| {
                        (at, self.renews.filter(|r| *r > at).unwrap_or(at + DEFAULT_PERIOD))
                    });
                    // 마이그레이션이 이번 달로 둔 기간(결제 기록 없이 낸 것으로 둔 기간)
                    let synthetic = ours.is_some()
                        && paid_start == old_start
                        && !tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM paddle_transactions WHERE user_id = ?1 AND kind = 'subscription')",
                            params![user],
                            |r| r.get::<_, bool>(0),
                        )?;
                    match ours {
                        // 같은 구독인데 지금보다 앞선 기간(늦게 온 옛 이벤트, 마이그레이션한 달보다 먼저 시작한 Paddle 기간):
                        // 지금 기간과 시간을 둔다(지난 기간의 사용량을 다시 세지 않게).
                        Some((s, e)) if period.0 < s => (Some(s), Some(e), old_included, paid_start),
                        // 같은 기간(요금제 변경·해지 예약·결제 밀림 등): 시간과 결제 여부를 그대로 둔다.
                        Some((s, _)) if period.0 == s => (Some(s), Some(period.1), old_included, paid_start),
                        // 마이그레이션한 기간 뒤 처음 보는 Paddle 기간이 마이그레이션 전에 시작했으면(그 갱신 웹훅을 그 전에 받았다)
                        // 이미 낸 기간이다: 그 기간을 낸 요금제(이 이벤트가 바꾸기 전 요금제)의 시간을 준다.
                        Some(_) if synthetic && period.0 * 1000 <= last_at => {
                            (Some(period.0), Some(period.1), included_seconds(&old_plan), Some(period.0))
                        }
                        // 지금 기간 안에서 맞춰진 기간: 시간과 결제 여부를 그대로 둔다.
                        Some((s, e)) if period.0 < e && !synthetic => (
                            Some(period.0),
                            Some(period.1),
                            old_included,
                            if paid_start == Some(s) { Some(period.0) } else { paid_start },
                        ),
                        // 새 구독·다음 기간: 그 기간에 이미 끝난 결제(먼저 온 갱신·차액 거래)만큼, 없으면 결제가 올 때까지 0.
                        _ => {
                            let (hours, has_base) = granted(tx, user, self.entity.as_deref(), period)?;
                            let paid = if has_base { Some(period.0) } else if new_sub { None } else { paid_start };
                            (Some(period.0), Some(period.1), hours, paid)
                        }
                    }
                };
                // 해지된 구독은 더 추적하지 않는다(canceled 웹훅과 같게, 어느 쪽이 먼저 와도).
                let subscription = self.entity.as_ref().filter(|_| self.status != "canceled");
                tx.execute(
                    "UPDATE users SET plan = ?2, plan_renews_at = ?3, plan_canceled = ?4, paddle_subscription_id = ?5,
                       paddle_customer_id = COALESCE(?6, paddle_customer_id), period_start = ?7, period_end = ?8, period_included = ?9,
                       period_paid_start = ?10, paddle_status = ?11
                     WHERE id = ?1",
                    params![user, new_plan, self.renews, self.canceling as i64, subscription, self.customer, start, end, included, paid, self.status],
                )?;
                if live {
                    // 기다리던 구독 결제가 반영됐다(`checkout`이 같은 거래를 다시 쓰지 않게).
                    tx.execute(
                        "UPDATE users SET paddle_pending_txn = NULL, paddle_pending_at = NULL WHERE id = ?1",
                        params![user],
                    )?;
                }
            }
            "subscription.canceled" => {
                // 해지된 구독은 더 추적하지 않는다(다시 구독할 때 이미 해지된 구독을 또 해지하지 않게). 고객 id는 포털에 쓰므로 둔다.
                tx.execute(
                    "UPDATE users SET plan = 'none', plan_canceled = 0, plan_renews_at = NULL, paddle_subscription_id = NULL,
                       paddle_status = 'canceled'
                     WHERE id = ?1 AND paddle_subscription_id = ?2",
                    params![user, self.entity],
                )?;
            }
            "transaction.completed" => {
                let Some(txn) = &self.entity else {
                    return Ok(out);
                };
                let at = self.at(now);
                // 지금 구독의 결제인지(아직 구독이 없으면 첫 결제). 새 구독에 밀렸거나 해지하는 중인 구독의 결제는
                // 기록만 하고 지금 기간·시간을 바꾸지 않는다(새 구독의 첫 결제는 그 구독 이벤트가 기록에서 가져간다).
                let queued: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM paddle_cancellations WHERE subscription_id = ?1)",
                    params![self.subscription_ref],
                    |r| r.get(0),
                )?;
                let ours = !queued && (current.is_none() || current == self.subscription_ref);
                let base = matches!(self.origin.as_str(), "web" | "api" | "subscription_recurring");
                // 모르는 가격의 갱신(대시보드에서 가격을 바꾸고 서버 설정은 그대로): 결제는 됐으므로 지금 요금제의 시간을 준다.
                let plan = self.plan.or_else(|| {
                    let kept = (self.hours == 0 && base && ours && current.is_some()).then(|| known_plan(&old_plan)).flatten();
                    if kept.is_some() {
                        tracing::error!(event = %self.id, transaction = %txn, "subscription payment with an unknown price: granted the current plan");
                    }
                    kept
                });
                // 거래가 준 시간: 추가 시간, 낸 기간의 포함 시간(첫 결제·갱신), 올린 요금제의 차액만큼의 포함 시간.
                #[derive(Clone, Copy)]
                enum Grant {
                    Hours(i64),
                    /// 일일권: 그 시간과 쓸 수 있는 기간의 시작(결제한 때)
                    Day(i64),
                    Period(&'static str, (i64, i64)),
                    /// 차액만큼의 시간과 그 시간이 들어갈 기간의 시작
                    Upgrade(i64, Option<i64>),
                    Nothing,
                }
                let grant = if self.hours > 0 {
                    Grant::Hours(self.hours * 3600)
                } else if self.day_pass {
                    // 24시간이 다 지난 뒤에 받은 결제(서버 장애로 Paddle이 늦게 다시 보낸 경우 등)는 받은 때부터 센다.
                    // 낸 일일권을 한 번도 못 쓰고 끝나지 않게.
                    if at + DAY_PASS_LENGTH <= now {
                        tracing::warn!(event = %self.id, transaction = %txn, "day pass payment arrived after its 24 hours: started now");
                    }
                    Grant::Day(if at + DAY_PASS_LENGTH <= now { now } else { at })
                } else if let Some(plan) = plan {
                    match self.origin.as_str() {
                        _ if base => Grant::Period(
                            plan,
                            self.billing_period
                                .or(old_start.zip(old_end))
                                .unwrap_or((at, at + DEFAULT_PERIOD)),
                        ),
                        // 차액이 낸 때(그 거래의 기간 시작). 그때가 들어 있는 기간이 그 시간을 받는다
                        // (지금 기간이면 바로, 아직 우리에게 시작하지 않은 기간이면 그 기간이 시작될 때 기록에서).
                        "subscription_update" => Grant::Upgrade(self.upgrade_grant, self.billing_period.map(|p| p.0).or(old_start)),
                        _ => Grant::Nothing,
                    }
                } else {
                    // 우리 상품이 아닌 결제(모르는 가격의 일회성 청구 등): 반영하지 않는다.
                    skip_payment(tx, Some(txn), now)?;
                    return Ok(out);
                };
                let (kind, seconds, period) = match grant {
                    Grant::Hours(s) => ("hours", s, None),
                    Grant::Day(a) => ("day", DAY_PASS_SECONDS, Some(a)),
                    Grant::Period(plan, (s, _)) => ("subscription", included_seconds(plan), Some(s)),
                    Grant::Upgrade(s, p) => ("subscription", s, p),
                    Grant::Nothing => ("subscription", 0, None),
                };
                // 환불을 되돌릴 수 있게 완료된 결제와 준 시간을 남긴다. 같은 거래는 한 번만 반영한다.
                let recorded = tx.execute(
                    "INSERT OR IGNORE INTO paddle_transactions
                       (id, user_id, kind, origin, subscription_id, seconds, total, currency, period_start, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![txn, user, kind, self.origin, self.subscription_ref, seconds, self.total.unwrap_or(0), self.currency, period, now],
                )? == 1;
                if !recorded {
                    return Ok(out);
                }
                match grant {
                    Grant::Hours(s) => {
                        tx.execute(
                            "UPDATE users SET extra_seconds = extra_seconds + ?2 WHERE id = ?1",
                            params![user, s],
                        )?;
                    }
                    // 일일권은 남은 시간이 없을 때만 판다(`checkout`). 두 결제창에서 거의 동시에 산 경우처럼 남은 시간이 있으면
                    // 낸 것이므로 합친다. 끝난 일일권의 남은 시간은 버리고, 늦게 온 옛 결제가 더 늦게 산 일일권의 끝을 당기지 않게 한다.
                    // 남은 시간이 없을 때 산 일일권은 새 잔액을 시작하고(`day_since`), 합쳐진 일일권들은 그 뒤에 산 것이다.
                    // 환불은 `day_since` 뒤에 산 일일권일 때만 지금 잔액에서 뺀다.
                    Grant::Day(a) => {
                        tx.execute(
                            "UPDATE users SET
                               day_seconds = CASE WHEN day_until > ?3 AND day_seconds > 0 THEN day_seconds ELSE 0 END + ?2,
                               day_since = CASE WHEN day_until > ?3 AND day_seconds > 0 THEN MIN(COALESCE(day_since, ?3), ?3) ELSE ?3 END,
                               day_until = MAX(COALESCE(day_until, 0), ?3 + ?4)
                             WHERE id = ?1",
                            params![user, DAY_PASS_SECONDS, a, DAY_PASS_LENGTH],
                        )?;
                    }
                    // 그 기간의 포함 시간을 준다(지난 기간의 늦은 결제는 빼고). 같은 기간이면 이미 더한 차액을 지우지 않게 더하고,
                    // 다음 기간으로 옮기면 그 기간에 먼저 끝난 결제(차액 포함)를 모두 센다.
                    Grant::Period(plan, (s, e)) if ours && old_start.is_none_or(|old| s >= old) => {
                        let included = if old_start == Some(s) {
                            if paid_start == Some(s) { old_included } else { old_included + included_seconds(plan) }
                        } else {
                            granted(tx, user, self.subscription_ref.as_deref(), (s, e))?.0
                        };
                        tx.execute(
                            "UPDATE users SET period_start = ?2, period_end = ?3, period_included = ?4, period_paid_start = ?2 WHERE id = ?1",
                            params![user, s, e, included],
                        )?;
                    }
                    // 지금 기간의 차액은 바로 더한다. 다음 기간의 것은 그 기간이 시작될 때 기록에서 센다.
                    Grant::Upgrade(s, Some(p)) if ours && s > 0 && old_start.zip(old_end).is_some_and(|(a, b)| (a..b).contains(&p)) => {
                        tx.execute(
                            "UPDATE users SET period_included = period_included + ?2 WHERE id = ?1",
                            params![user, s],
                        )?;
                    }
                    _ => {}
                }
                // 금액은 구독이 아니라 거래에 있다. 처음 구독한 결제(결제창)일 때만 영수증 메일을 보낸다(갱신·차액은 Paddle이 보낸다).
                if matches!(self.origin.as_str(), "web" | "api")
                    && let Some(plan) = self.plan
                {
                    out.receipt = Some(Receipt {
                        user,
                        plan: plan.to_owned(),
                        amount: self.total.map(|t| format_amount(t, &self.currency)).unwrap_or_default(),
                        renews: self.billing_period.map(|(_, e)| e),
                    });
                }
            }
            _ => {}
        }
        Ok(out)
    }

    /// 승인된 환불·차지백. 추가 시간은 돌려준 몫만큼 다시 빼고, 구독 결제를 모두 돌려받았거나(전액 환불) 차지백이면
    /// 요금제를 끝내고 구독을 해지한다. 올린 요금제의 차액을 돌려줬으면 그때 더한 포함 시간을 그만큼 뺀다(요금제는 그대로).
    /// 일부 환불(장애 보상 등)은 요금제를 그대로 둔다.
    fn adjust(&self, tx: &rusqlite::Transaction, now: i64) -> rusqlite::Result<Applied> {
        // 대기 중인 환불은 승인될 때 adjustment.updated로 다시 온다.
        if self.status != "approved" || !matches!(self.action.as_str(), "refund" | "chargeback") {
            return Ok(Applied::default());
        }
        let (Some(adjustment), Some(txn)) = (&self.entity, &self.adjusted_transaction) else {
            return Ok(Applied::default());
        };
        // 같은 조정(만들 때 이미 승인된 것과 나중의 상태 변경)은 한 번만 반영한다.
        if tx.execute(
            "INSERT OR IGNORE INTO paddle_adjustments (id, transaction_id, applied_at) VALUES (?1, ?2, ?3)",
            params![adjustment, txn, now],
        )? == 0
        {
            return Ok(Applied::default());
        }
        type Paid = (i64, String, String, Option<String>, i64, i64, String, i64, i64, Option<i64>);
        let paid: Option<Paid> = tx
            .query_row(
                "SELECT user_id, kind, origin, subscription_id, seconds, total, currency, refunded, seconds_back, period_start
                 FROM paddle_transactions WHERE id = ?1",
                params![txn],
                |r| {
                    Ok((
                        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?,
                        r.get(9)?,
                    ))
                },
            )
            .optional()?;
        let Some((user, kind, origin, subscription, seconds, total, currency, refunded, seconds_back, period)) = paid else {
            // 일부러 반영하지 않은 결제(지운 계정, 사용자를 모르는 결제, 우리 상품이 아닌 결제)의 환불은 반영할 것이 없다.
            if payment_skipped(tx, txn)? {
                tracing::info!(event = %self.id, transaction = %txn, "refund of a payment we skipped: nothing to take back");
                return Ok(Applied::default());
            }
            // 그 결제의 transaction.completed가 아직 안 왔을 수 있다(우리 쪽 실패로 Paddle이 다시 보내는 중).
            // 한동안은 이 웹훅도 실패로 답해 Paddle이 다시 보내게 하고(이벤트·조정 기록은 되돌린다), 그 뒤에는 넘긴다.
            if now - self.at(now) < ADJUSTMENT_RETRY {
                tracing::info!(event = %self.id, transaction = %txn, "refund for a payment not recorded yet: Paddle will retry");
                return Ok(Applied { retry: true, ..Applied::default() });
            }
            tracing::warn!(event = %self.id, transaction = %txn, "refund for a payment we did not record: skipped");
            return Ok(Applied::default());
        };
        let current_period: Option<i64> = tx
            .query_row("SELECT period_start FROM users WHERE id = ?1", params![user], |r| r.get(0))
            .optional()?
            .flatten();
        let left = (total - refunded).max(0);
        let amount = if self.adjusted_full || total <= 0 {
            left
        } else if self.currency != currency {
            tracing::warn!(event = %self.id, "refund in another currency: treated as a full refund");
            left
        } else {
            self.adjusted_amount.clamp(0, left)
        };
        let refunded = refunded + amount;
        let whole = total <= 0 || refunded >= total;
        tx.execute(
            "UPDATE paddle_transactions SET refunded = ?2 WHERE id = ?1",
            params![txn, refunded],
        )?;
        // 고객이 요청한 환불은 금액과 상관없이 그 결제로 받은 이용을 끝낸다(환불 정책: 쓴 시간과 결제 처리 비용을 뺀
        // 나머지를 돌려주므로, 남은 시간을 계속 쓰게 두면 그만큼 손해다). 장애 보상만 돌려준 비율만큼 다룬다.
        let ends_use = !self.compensation;
        // 추가 시간·일일권·차액으로 준 시간 중 돌려준 몫(누적)만큼 다시 뺀다(처음·갱신 결제의 시간은 요금제로 다룬다).
        let target = if ends_use || whole { seconds } else { seconds * refunded / total };
        let back = (target - seconds_back).max(0);
        let returns_hours = kind == "hours" || kind == "day" || origin == "subscription_update";
        if returns_hours {
            tx.execute(
                "UPDATE paddle_transactions SET seconds_back = ?2 WHERE id = ?1",
                params![txn, target],
            )?;
        }
        let mut out = Applied::default();
        if kind == "hours" {
            tx.execute(
                "UPDATE users SET extra_seconds = MAX(0, extra_seconds - ?2) WHERE id = ?1",
                params![user, back],
            )?;
        } else if kind == "day" {
            // 지금 잔액에 들어 있는 일일권(잔액이 시작된 뒤에 산 것)의 환불일 때만 뺀다. 그 전에 끝난 일일권의 시간은
            // 남아 있지 않고, 지금 남은 시간은 따로 낸 다른 일일권의 것이라 건드리지 않는다. 남은 시간이 없어지면 일일권을 끝낸다.
            tx.execute(
                "UPDATE users SET day_seconds = MAX(0, day_seconds - ?2),
                   day_until = CASE WHEN day_seconds - ?2 <= 0 THEN MIN(COALESCE(day_until, 0), ?3) ELSE day_until END
                 WHERE id = ?1 AND day_until > ?3 AND ?4 >= day_since",
                params![user, back, now, period],
            )?;
        // 차지백은 언제나, 고객 환불은 지금 기간의 결제일 때만 요금제를 끝낸다(지난 기간 결제의 환불은 따로 낸 지금 기간을 끝내지 않는다).
        } else if self.action == "chargeback"
            || (ends_use && origin != "subscription_update" && period.is_some() && period == current_period)
        {
            if let Some(sub) = subscription.or_else(|| self.subscription_ref.clone()) {
                tx.execute(
                    "UPDATE users SET plan = 'none', plan_canceled = 0, plan_renews_at = NULL WHERE id = ?1 AND paddle_subscription_id = ?2",
                    params![user, sub],
                )?;
                queue_cancellation(tx, &sub, true, now)?;
                out.revoked = Some(sub);
            }
        } else if origin == "subscription_update" {
            // 그 차액으로 더한 시간은 그 기간에만 있다(기간이 지났으면 뺄 것이 없다).
            tx.execute(
                "UPDATE users SET period_included = MAX(0, period_included - ?2) WHERE id = ?1 AND period_start <= ?3 AND ?3 < period_end",
                params![user, back, period],
            )?;
        } else {
            tracing::info!(event = %self.id, transaction = %txn, "outage credit or past-period refund of a subscription payment: plan kept");
        }
        Ok(out)
    }
}

/// 그 구독이 이 기간 [시작, 끝)에 끝낸 결제로 받은 포함 시간(처음·갱신 결제 + 그 기간에 낸 차액 − 돌려준 차액)과
/// 처음·갱신 결제가 있는지. 결제마다 낸 때(그 거래의 기간 시작)를 남겨 두고, 그때가 들어 있는 기간에 센다.
fn granted(c: &rusqlite::Connection, user: i64, subscription: Option<&str>, period: (i64, i64)) -> rusqlite::Result<(i64, bool)> {
    c.query_row(
        "SELECT COALESCE(SUM(seconds - seconds_back), 0), COALESCE(MAX(origin IN ('web', 'api', 'subscription_recurring')), 0)
         FROM paddle_transactions
         WHERE user_id = ?1 AND kind = 'subscription' AND period_start >= ?3 AND period_start < ?4
           AND (subscription_id IS ?2 OR subscription_id IS NULL)",
        params![user, subscription, period.0, period.1],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// 일부러 반영하지 않은 결제 표시(그 결제의 환불을 다시 받으려 기다리지 않게). 받은 웹훅 id 표(`paddle_events`)에
/// 겹치지 않는 머리말(`skipped:`)을 붙여 둔다(Paddle 이벤트 id는 `evt_`로 시작한다). 사용자와 묶이지 않아 계정을 지워도 남는다.
fn skip_payment(c: &rusqlite::Connection, transaction: Option<&str>, now: i64) -> rusqlite::Result<()> {
    if let Some(txn) = transaction {
        c.execute(
            "INSERT OR IGNORE INTO paddle_events (id, received_at) VALUES ('skipped:' || ?1, ?2)",
            params![txn, now],
        )?;
    }
    Ok(())
}

fn payment_skipped(c: &rusqlite::Connection, transaction: &str) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM paddle_events WHERE id = 'skipped:' || ?1)",
        params![transaction],
        |r| r.get(0),
    )
}

/// 계정을 지울 때: 그 사람의 결제 기록은 함께 지워지므로(외래 키) 나중에 온 환불을 기다리지 않게 결제 id만 표시해 둔다.
pub fn skip_payments_of(c: &rusqlite::Connection, user: i64, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO paddle_events (id, received_at) SELECT 'skipped:' || id, ?2 FROM paddle_transactions WHERE user_id = ?1",
        params![user, now],
    )?;
    Ok(())
}

fn known_plan(plan: &str) -> Option<&'static str> {
    match plan {
        "basic" => Some("basic"),
        "pro" => Some("pro"),
        _ => None,
    }
}

/// Paddle이 해지를 확인할 때까지 다시 시도할 구독(`Billing::process_cancellations`). 사용자 행과 따로 남는다(계정을 지워도).
/// 해지가 확인된 뒤에도 끝난 것(done)으로 남겨, 그 구독의 늦은 이벤트가 요금제를 되살리지 못하게 한다.
/// `revoked`는 환불·차지백, 그리고 canceled 웹훅이 온 구독이라는 기록이다.
pub fn queue_cancellation(c: &rusqlite::Connection, subscription: &str, revoked: bool, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO paddle_cancellations (subscription_id, revoked, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(subscription_id) DO UPDATE SET revoked = MAX(revoked, excluded.revoked)",
        params![subscription, revoked as i64, now],
    )?;
    Ok(())
}

/// 해지가 확인됐다(Paddle 응답이나 canceled 웹훅).
fn confirm_cancellation(c: &rusqlite::Connection, subscription: Option<&str>) -> rusqlite::Result<()> {
    // 지우지 않고 끝난 것으로 남긴다: canceled 웹훅보다 늦게 온 그 구독의 이벤트가 다시 지금 구독이 되지 않게.
    c.execute(
        "UPDATE paddle_cancellations SET done = 1 WHERE subscription_id = ?1",
        params![subscription],
    )?;
    Ok(())
}

/// 웹훅을 반영한 결과: 보낼 영수증, 새 구독에 밀려 해지해야 할 예전 구독, 환불·차지백으로 끝내야 할 구독.
#[derive(Default)]
pub struct Applied {
    pub receipt: Option<Receipt>,
    pub replaced: Option<String>,
    pub revoked: Option<String>,
    /// 지금은 반영할 수 없다(환불된 결제가 아직 기록되지 않음): 웹훅을 실패로 답해 Paddle이 다시 보내게 한다.
    pub retry: bool,
}

/// 결제 기록보다 먼저 온 환불·차지백을 Paddle이 다시 보내게 하는 기간(Paddle은 실패한 웹훅을 며칠 동안 다시 보낸다).
const ADJUSTMENT_RETRY: i64 = 72 * 3600;

pub struct Receipt {
    pub user: i64,
    pub plan: String,
    pub amount: String,
    pub renews: Option<i64>,
}

/// Paddle 금액은 가장 작은 화폐 단위의 정수다(예: 1900 USD = $19.00, 2800 JPY = 2,800 JPY).
/// 소수 자리가 없는 통화(Paddle 기준 CLP·JPY·KRW·VND)는 그대로 쓴다.
fn format_amount(minor: i64, currency: &str) -> String {
    let decimals = if matches!(currency, "CLP" | "JPY" | "KRW" | "VND") { 0 } else { 2 };
    let sign = if minor < 0 { "-" } else { "" };
    let n = minor.unsigned_abs();
    let (whole, cents) = if decimals == 0 { (n, None) } else { (n / 100, Some(n % 100)) };
    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let number = match cents {
        Some(c) => format!("{grouped}.{c:02}"),
        None => grouped,
    };
    if currency == "USD" {
        format!("{sign}${number}")
    } else {
        format!("{sign}{number} {currency}")
    }
}

pub fn verify_signature(secret: &str, header: &str, body: &[u8], now: i64) -> bool {
    let mut ts = None;
    let mut sigs = Vec::new();
    for part in header.split(';') {
        match part.trim().split_once('=') {
            Some(("ts", v)) => ts = v.parse::<i64>().ok(),
            Some(("h1", v)) => sigs.push(v.to_owned()),
            _ => {}
        }
    }
    let Some(ts) = ts else { return false };
    // 5분보다 오래된 서명은 재전송 공격일 수 있다. 어떤 ts가 와도 넘치지 않게 차이의 절댓값을 부호 없이 잰다.
    if now.abs_diff(ts) > 300 {
        return false;
    }
    let Ok(mut mac) = Hmac::<Sha256v10>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(format!("{ts}:").as_bytes());
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    let expected: String = expected.iter().map(|b| format!("{b:02x}")).collect();
    sigs.iter()
        .any(|s| constant_eq(s.as_bytes(), expected.as_bytes()))
}

// hmac 0.12는 digest 0.10 계열이라 sha2도 0.10을 쓴다(p256이 쓰는 0.11과 따로).
type Sha256v10 = sha2_v10::Sha256;

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// RFC 3339 시각(예: 2026-11-17T10:00:00.000Z)을 유닉스 초로. 시간대는 Z 또는 ±hh:mm.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    parse_rfc3339_ms(s).map(|ms| ms.div_euclid(1000))
}

/// RFC 3339 시각을 유닉스 밀리초로. 소수 초는 자리 수와 상관없이 밀리초까지 읽는다(".52" = 520ms).
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|v| v.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (time, offset) = if let Some(t) = rest.strip_suffix('Z') {
        (t, 0)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (t, off) = rest.split_at(at);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let mut o = off[1..].split(':').map(|v| v.parse::<i64>());
        (
            t,
            sign * (o.next()?.ok()? * 3600 + o.next().and_then(|v| v.ok()).unwrap_or(0) * 60),
        )
    };
    let mut t = time.split(':');
    let (h, mi) = (
        t.next()?.parse::<i64>().ok()?,
        t.next()?.parse::<i64>().ok()?,
    );
    let (sec, ms) = match t.next() {
        Some(v) => {
            let (whole, frac) = v.split_once('.').unwrap_or((v, ""));
            if !frac.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let ms = format!("{frac:0<3}")[..3].parse::<i64>().ok()?;
            (whole.parse::<i64>().ok()?, ms)
        }
        None => (0, 0),
    };
    // 1970-01-01부터의 날 수(그레고리력, Howard Hinnant의 days_from_civil)
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + h * 3600 + mi * 60 + sec - offset) * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: i64 = 8 * 3600;
    const PRO: i64 = 20 * 3600;
    /// 기간 한가운데(남은 절반)에서 Basic → Pro로 올린 뒤의 포함 시간
    const HALF_UP: i64 = BASIC + (PRO - BASIC) / 2;

    #[test]
    fn signature_must_match_and_be_recent() {
        let body = br#"{"event_id":"evt_1"}"#;
        let ts = 1_800_000_000;
        let mut mac = Hmac::<Sha256v10>::new_from_slice(b"secret").unwrap();
        mac.update(format!("{ts}:").as_bytes());
        mac.update(body);
        let h: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let header = format!("ts={ts};h1={h}");
        assert!(verify_signature("secret", &header, body, ts + 10));
        assert!(!verify_signature("other", &header, body, ts + 10));
        assert!(!verify_signature("secret", &header, b"{}", ts + 10));
        assert!(!verify_signature("secret", &header, body, ts + 1000));
        // 터무니없는 ts도 넘치지 않고 거절한다.
        let extreme = format!("ts={};h1={h}", i64::MIN);
        assert!(!verify_signature("secret", &extreme, body, i64::MAX));
        assert!(!verify_signature("secret", &format!("ts={};h1={h}", i64::MAX), body, i64::MIN));
    }

    #[test]
    fn parses_paddle_times() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-11-17T10:00:00.000Z"),
            Some(1_794_909_600)
        );
        assert_eq!(
            parse_rfc3339("2026-11-17T19:00:00+09:00"),
            Some(1_794_909_600)
        );
        // 이벤트 순서는 밀리초까지 가린다(Paddle은 소수 자리 수가 일정하지 않다).
        assert_eq!(parse_rfc3339_ms("2024-10-12T07:20:50.52Z"), Some(1_728_717_650_520));
        assert_eq!(parse_rfc3339_ms("2024-10-12T07:20:50.621022Z"), Some(1_728_717_650_621));
        assert_eq!(parse_rfc3339_ms("2024-10-12T07:20:50Z"), Some(1_728_717_650_000));
        assert_eq!(parse_rfc3339_ms("2024-10-12T07:20:50.x1Z"), None);
    }

    #[test]
    fn amounts_use_each_currencys_minor_unit() {
        assert_eq!(format_amount(1900, "USD"), "$19.00");
        assert_eq!(format_amount(123_456_789, "USD"), "$1,234,567.89");
        assert_eq!(format_amount(-150, "USD"), "-$1.50");
        assert_eq!(format_amount(-5, "EUR"), "-0.05 EUR");
        assert_eq!(format_amount(2800, "JPY"), "2,800 JPY");
        assert_eq!(format_amount(28000, "KRW"), "28,000 KRW");
        assert_eq!(format_amount(-990, "KRW"), "-990 KRW");
    }

    #[test]
    fn a_pending_checkout_is_reused_until_paid() {
        assert_eq!(pending_checkout("draft"), PendingCheckout::Reuse);
        assert_eq!(pending_checkout("ready"), PendingCheckout::Reuse);
        for paid in ["billed", "paid", "completed"] {
            assert_eq!(pending_checkout(paid), PendingCheckout::Paid);
        }
        assert_eq!(pending_checkout("canceled"), PendingCheckout::Gone);
    }

    fn billing() -> Billing {
        Billing {
            api_key: String::new(),
            client_token: String::new(),
            webhook_secret: String::new(),
            sandbox: true,
            price_basic: "pri_basic".into(),
            price_pro: "pri_pro".into(),
            price_hour: "pri_hour".into(),
            price_day: "pri_day".into(),
            http: reqwest::Client::new(),
        }
    }

    #[tokio::test]
    async fn events_for_a_deleted_account_are_acknowledged_and_skipped() {
        // 계정을 지운 뒤 도착한 웹훅은 실패(재시도 반복)하지 않고 넘기며, 이후 가입자에게 붙지 않는다.
        let db = Db::memory();
        let ins = |email: &'static str| {
            db.blocking(move |c| {
                c.execute("INSERT INTO users (email, created_at) VALUES (?1, 1)", params![email])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap()
        };
        let a = ins("alice@b.co");
        let b = billing();
        let sig = b.sign_user(a);
        let ev = |id: &str| {
            json!({ "event_id": id, "event_type": "subscription.updated", "occurred_at": "2026-10-02T00:00:00Z",
                    "data": { "id": "sub_alice", "customer_id": "ctm_alice", "status": "canceled",
                              "custom_data": { "user_id": a.to_string(), "sig": sig },
                              "items": [{ "price": { "id": "pri_pro" }, "quantity": 1 }] } })
        };
        db.blocking(move |c| c.execute("DELETE FROM users WHERE id = ?1", params![a])).unwrap();
        assert!(b.apply_event(&db, ev("e1")).await.is_ok());
        ins("bob@b.co");
        b.apply_event(&db, ev("e2")).await.unwrap();
        let cus: Option<String> = db
            .blocking(|c| c.query_row("SELECT paddle_customer_id FROM users WHERE email = 'bob@b.co'", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(cus, None);
    }

    #[tokio::test]
    async fn a_second_subscription_reports_the_one_it_replaces() {
        let db = Db::memory();
        let uid = db
            .blocking(|c| {
                c.execute("INSERT INTO users (email, created_at) VALUES ('a@b.co', 1)", [])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        let b = billing();
        let sig = b.sign_user(uid);
        let ev = |id: &str, sub: &str, at: &str| {
            json!({ "event_id": id, "event_type": "subscription.activated", "occurred_at": at,
                    "data": { "id": sub, "status": "active", "custom_data": { "user_id": uid.to_string(), "sig": sig },
                              "items": [{ "price": { "id": "pri_pro" }, "quantity": 1 }] } })
        };
        let first = b.apply_event(&db, ev("e1", "sub_1", "2026-10-01T00:00:00Z")).await.unwrap();
        assert_eq!(first.replaced, None);
        let second = b.apply_event(&db, ev("e2", "sub_2", "2026-10-02T00:00:00Z")).await.unwrap();
        // 예전 구독은 웹훅 처리기가 해지한다(두 번 결제되지 않게).
        assert_eq!(second.replaced.as_deref(), Some("sub_1"));
    }

    #[tokio::test]
    async fn old_subscription_events_do_not_touch_the_new_one() {
        let db = Db::memory();
        let uid = db
            .blocking(|c| {
                c.execute("INSERT INTO users (email, created_at) VALUES ('a@b.co', 1)", [])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        let b = billing();
        let sig = b.sign_user(uid);
        let ev = |id: &str, kind: &str, sub: &str, status: &str, at: &str| {
            json!({ "event_id": id, "event_type": kind, "occurred_at": at,
                    "data": { "id": sub, "status": status, "custom_data": { "user_id": uid.to_string(), "sig": sig },
                              "items": [{ "price": { "id": "pri_pro" }, "quantity": 1 }] } })
        };
        let plan = || -> (String, Option<String>) {
            db.blocking(|c| {
                c.query_row("SELECT plan, paddle_subscription_id FROM users WHERE id = ?1", params![uid], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
            })
            .unwrap()
        };
        b.apply_event(&db, ev("e1", "subscription.activated", "sub_a", "active", "2026-10-01T00:00:00Z")).await.unwrap();
        b.apply_event(&db, ev("e2", "subscription.activated", "sub_b", "active", "2026-10-10T00:00:00Z")).await.unwrap();
        assert_eq!(plan(), ("pro".into(), Some("sub_b".into())));
        // 옛 구독의 해지·변경 이벤트는 새 구독에 반영하지 않는다.
        b.apply_event(&db, ev("e3", "subscription.canceled", "sub_a", "canceled", "2026-10-11T00:00:00Z")).await.unwrap();
        b.apply_event(&db, ev("e4", "subscription.updated", "sub_a", "active", "2026-10-12T00:00:00Z")).await.unwrap();
        assert_eq!(plan(), ("pro".into(), Some("sub_b".into())));
        // 같은 구독이라도 늦게 온 옛 이벤트는 버린다.
        b.apply_event(&db, ev("e5", "subscription.canceled", "sub_b", "canceled", "2026-10-20T00:00:00Z")).await.unwrap();
        b.apply_event(&db, ev("e6", "subscription.updated", "sub_b", "active", "2026-10-15T00:00:00Z")).await.unwrap();
        assert_eq!(plan().0, "none");
    }

    #[tokio::test]
    async fn webhooks_set_plan_once_and_add_hours() {
        let db = Db::memory();
        let uid = db
            .blocking(|c| {
                c.execute(
                    "INSERT INTO users (email, created_at) VALUES ('a@b.co', 1)",
                    [],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        let b = billing();
        let sig = b.sign_user(uid);
        // 서명이 틀린 사용자 id는 무시한다(다른 사람 계정에 구독을 붙이지 못하게).
        b.apply_event(&db, json!({
            "event_id": "evt_0", "event_type": "subscription.activated",
            "data": { "id": "sub_x", "status": "active", "custom_data": { "user_id": uid.to_string(), "sig": "forged" },
                      "items": [{ "price": { "id": "pri_pro" }, "quantity": 1 }] }
        }))
        .await
        .unwrap();
        let plan: String = db
            .blocking(|c| {
                c.query_row("SELECT plan FROM users WHERE id = ?1", params![uid], |r| {
                    r.get(0)
                })
            })
            .unwrap();
        assert_eq!(plan, "none");
        let event = json!({
            "event_id": "evt_1", "event_type": "subscription.activated",
            "data": { "id": "sub_1", "customer_id": "ctm_1", "status": "active",
                      "custom_data": { "user_id": uid.to_string(), "sig": sig }, "next_billed_at": "2026-11-17T10:00:00Z",
                      "items": [{ "price": { "id": "pri_pro" }, "quantity": 1 }] }
        });
        assert!(b.apply_event(&db, event.clone()).await.unwrap().receipt.is_none(), "amounts come with the transaction");
        // 같은 이벤트는 다시 반영하지 않는다.
        assert!(b.apply_event(&db, event).await.unwrap().receipt.is_none());
        // 영수증은 처음 구독한 결제(거래)가 끝났을 때 금액과 함께 보낸다.
        let paid = json!({
            "event_id": "evt_1b", "event_type": "transaction.completed",
            "data": { "id": "txn_sub", "origin": "web", "subscription_id": "sub_1", "customer_id": "ctm_1", "currency_code": "USD",
                      "custom_data": { "user_id": uid.to_string(), "sig": sig },
                      "billing_period": { "starts_at": "2026-10-17T10:00:00Z", "ends_at": "2026-11-17T10:00:00Z" },
                      "details": { "totals": { "grand_total": "3900" } },
                      "items": [{ "price": { "id": "pri_pro" }, "quantity": 1 }] }
        });
        let receipt = b.apply_event(&db, paid).await.unwrap().receipt.expect("receipt");
        assert_eq!((receipt.amount.as_str(), receipt.renews), ("$39.00", Some(1_794_909_600)));
        let plan: String = db
            .blocking(|c| {
                c.query_row("SELECT plan FROM users WHERE id = ?1", params![uid], |r| {
                    r.get(0)
                })
            })
            .unwrap();
        assert_eq!(plan, "pro");
        // 추가 시간 구매는 구독 id 없이도 custom_data로 찾는다.
        b.apply_event(
            &db,
            json!({
                "event_id": "evt_2", "event_type": "transaction.completed",
                "data": { "id": "txn_1", "custom_data": { "user_id": uid.to_string(), "sig": b.sign_user(uid) },
                          "items": [{ "price": { "id": "pri_hour" }, "quantity": 5 }] }
            }),
        )
        .await
        .unwrap();
        let extra: i64 = db
            .blocking(|c| {
                c.query_row(
                    "SELECT extra_seconds FROM users WHERE id = ?1",
                    params![uid],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(extra, 5 * 3600);
        b.apply_event(&db, json!({ "event_id": "evt_3", "event_type": "subscription.canceled", "data": { "id": "sub_1", "status": "canceled" } }))
            .await
            .unwrap();
        let plan: String = db
            .blocking(|c| {
                c.query_row("SELECT plan FROM users WHERE id = ?1", params![uid], |r| {
                    r.get(0)
                })
            })
            .unwrap();
        assert_eq!(plan, "none");
    }

    /// 사용자 하나와 그 사용자의 서명. (db, billing, uid, sig)
    fn fixture() -> (Db, Billing, i64, String) {
        let db = Db::memory();
        let uid = db
            .blocking(|c| {
                c.execute("INSERT INTO users (email, created_at) VALUES ('a@b.co', 1)", [])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        let b = billing();
        let sig = b.sign_user(uid);
        (db, b, uid, sig)
    }

    #[derive(Debug, PartialEq)]
    struct Row {
        plan: String,
        start: Option<i64>,
        end: Option<i64>,
        included: i64,
        extra: i64,
        sub: Option<String>,
    }

    fn row(db: &Db, uid: i64) -> Row {
        db.blocking(|c| {
            c.query_row(
                "SELECT plan, period_start, period_end, period_included, extra_seconds, paddle_subscription_id FROM users WHERE id = ?1",
                params![uid],
                |r| Ok(Row { plan: r.get(0)?, start: r.get(1)?, end: r.get(2)?, included: r.get(3)?, extra: r.get(4)?, sub: r.get(5)? }),
            )
        })
        .unwrap()
    }

    fn sub_event(id: &str, kind: &str, uid: i64, sig: &str, price: &str, at: &str, period: (&str, &str)) -> Value {
        json!({ "event_id": id, "event_type": kind, "occurred_at": at,
                "data": { "id": "sub_1", "customer_id": "ctm_1", "status": "active",
                          "custom_data": { "user_id": uid.to_string(), "sig": sig },
                          "current_billing_period": { "starts_at": period.0, "ends_at": period.1 },
                          "next_billed_at": period.1,
                          "items": [{ "price": { "id": price }, "quantity": 1 }] } })
    }

    /// 구독 결제 거래가 끝남. `lines`: (가격, 수량(음수면 돌려준 예전 요금), 남은 기간 비율)
    fn paid(id: &str, uid: i64, sig: &str, txn: &str, origin: &str, lines: &[(&str, i64, Option<&str>)], period: (&str, &str)) -> Value {
        let line_items: Vec<Value> = lines
            .iter()
            .map(|(price, qty, rate)| json!({ "price_id": price, "quantity": qty, "proration": rate.map(|r| json!({ "rate": r })) }))
            .collect();
        json!({ "event_id": id, "event_type": "transaction.completed", "occurred_at": period.0,
                "data": { "id": txn, "origin": origin, "subscription_id": "sub_1", "customer_id": "ctm_1", "currency_code": "USD",
                          "custom_data": { "user_id": uid.to_string(), "sig": sig },
                          "billing_period": { "starts_at": period.0, "ends_at": period.1 },
                          "details": { "totals": { "grand_total": "1900" }, "line_items": line_items },
                          "items": [{ "price": { "id": lines[0].0 }, "quantity": 1 }] } })
    }

    const OCT: (&str, &str) = ("2026-10-01T00:00:00Z", "2026-10-31T00:00:00Z");
    const NOV: (&str, &str) = ("2026-10-31T00:00:00Z", "2026-11-30T00:00:00Z");

    #[tokio::test]
    async fn hours_come_with_each_paid_period_and_upgrade() {
        let (db, b, uid, sig) = fixture();
        let ev = |id: &str, price: &str, at: &str, period| sub_event(id, "subscription.updated", uid, &sig, price, at, period);
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        // 구독만으로는 시간을 주지 않는다. 첫 결제가 끝나면 그 기간의 포함 시간을 준다.
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.start, r.end, r.included), ("basic", parse_rfc3339(OCT.0), parse_rfc3339(OCT.1), 0));
        b.apply_event(&db, paid("t1", uid, &sig, "txn_1", "web", &[("pri_basic", 1, None)], OCT)).await.unwrap();
        assert_eq!(row(&db, uid).included, BASIC);
        // 기간 한가운데에서 Pro로 올리면 구독 이벤트로는 그대로, 차액 거래(남은 절반)가 끝나면 차이의 절반(10시간)을 더한다.
        b.apply_event(&db, ev("e2", "pri_pro", "2026-10-16T00:00:00Z", OCT)).await.unwrap();
        assert_eq!((row(&db, uid).plan.as_str(), row(&db, uid).included), ("pro", BASIC));
        let upgrade = [("pri_pro", 1, Some("0.5")), ("pri_basic", -1, Some("0.5"))];
        b.apply_event(&db, paid("t2", uid, &sig, "txn_up", "subscription_update", &upgrade, OCT)).await.unwrap();
        b.apply_event(&db, paid("t2b", uid, &sig, "txn_up", "subscription_update", &upgrade, OCT)).await.unwrap();
        assert_eq!(row(&db, uid).included, HALF_UP, "once per transaction");
        b.apply_event(&db, ev("e3", "pri_pro", "2026-10-16T00:00:01Z", OCT)).await.unwrap();
        assert_eq!(row(&db, uid).included, HALF_UP);
        // 내리면 이번 기간은 그대로(청구도 환불도 없다), 다음 기간부터 Basic.
        b.apply_event(&db, ev("e4", "pri_basic", "2026-10-20T00:00:00Z", OCT)).await.unwrap();
        assert_eq!((row(&db, uid).plan.as_str(), row(&db, uid).included), ("basic", HALF_UP));
        // 갱신: Paddle은 결제 전에 기간부터 옮긴다. 결제가 밀려도(past_due) 새 기간의 시간은 결제가 끝날 때까지 없다.
        b.apply_event(&db, ev("e5", "pri_basic", "2026-10-31T00:00:01Z", NOV)).await.unwrap();
        let mut late = sub_event("e6", "subscription.past_due", uid, &sig, "pri_basic", "2026-10-31T00:00:05Z", NOV);
        late["data"]["status"] = json!("past_due");
        b.apply_event(&db, late).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.start, r.included), ("basic", parse_rfc3339(NOV.0), 0));
        assert_eq!(status(&db, uid).as_deref(), Some("past_due"));
        b.apply_event(&db, paid("t3", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_basic", 1, None)], NOV)).await.unwrap();
        assert_eq!(row(&db, uid).included, BASIC);
        // 결제가 구독 이벤트보다 먼저 와도 같다(기간을 옮기고 시간을 준 뒤, 늦게 온 구독 이벤트는 그대로 둔다).
        const DEC: (&str, &str) = ("2026-11-30T00:00:00Z", "2026-12-30T00:00:00Z");
        b.apply_event(&db, paid("t4", uid, &sig, "txn_dec", "subscription_recurring", &[("pri_basic", 1, None)], DEC)).await.unwrap();
        b.apply_event(&db, ev("e7", "pri_basic", "2026-11-30T00:00:01Z", DEC)).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.start, r.included), (parse_rfc3339(DEC.0), BASIC));
        // 지난 기간의 늦은 결제는 지금 기간을 바꾸지 않는다.
        b.apply_event(&db, paid("t5", uid, &sig, "txn_old", "subscription_recurring", &[("pri_basic", 1, None)], NOV)).await.unwrap();
        assert_eq!(row(&db, uid).start, parse_rfc3339(DEC.0));
    }

    #[tokio::test]
    async fn an_unknown_price_keeps_the_plan() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        b.apply_event(&db, paid("t1", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, sub_event("e2", "subscription.updated", uid, &sig, "pri_legacy", "2026-10-02T00:00:00Z", OCT))
            .await
            .unwrap();
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.included), ("pro", PRO));
    }

    #[tokio::test]
    async fn events_in_the_same_second_keep_their_order() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00.100Z", OCT))
            .await
            .unwrap();
        b.apply_event(&db, sub_event("e2", "subscription.updated", uid, &sig, "pri_pro", "2026-10-01T00:00:00.900Z", OCT))
            .await
            .unwrap();
        // 같은 초에 먼저 일어난 이벤트가 늦게 도착하면 버린다.
        b.apply_event(&db, sub_event("e3", "subscription.updated", uid, &sig, "pri_basic", "2026-10-01T00:00:00.200Z", OCT))
            .await
            .unwrap();
        assert_eq!(row(&db, uid).plan, "pro");
    }

    #[tokio::test]
    async fn a_canceled_subscription_is_forgotten_so_resubscribing_cancels_nothing() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        b.apply_event(&db, json!({ "event_id": "e2", "event_type": "subscription.canceled", "occurred_at": "2026-10-05T00:00:00Z",
                                   "data": { "id": "sub_1", "status": "canceled" } }))
            .await
            .unwrap();
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.sub), ("none", None));
        let mut again = sub_event("e3", "subscription.activated", uid, &sig, "pri_basic", "2026-10-06T00:00:00Z", OCT);
        again["data"]["id"] = json!("sub_2");
        let applied = b.apply_event(&db, again).await.unwrap();
        assert_eq!(applied.replaced, None);
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.sub.as_deref(), r.included), ("basic", Some("sub_2"), 0), "until its first payment");
    }

    #[tokio::test]
    async fn signed_hour_purchases_go_to_the_signed_user() {
        // 같은 Paddle 고객이 다른 계정에 묶여 있어도 서명한 계정이 산 것이다.
        let (db, b, other, _) = fixture();
        db.blocking(|c| c.execute("UPDATE users SET paddle_customer_id = 'ctm_shared' WHERE id = ?1", params![other]))
            .unwrap();
        let buyer = db
            .blocking(|c| {
                c.execute("INSERT INTO users (email, created_at) VALUES ('buyer@b.co', 1)", [])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        b.apply_event(&db, json!({ "event_id": "e1", "event_type": "transaction.completed",
            "data": { "id": "txn_1", "customer_id": "ctm_shared", "custom_data": { "user_id": buyer.to_string(), "sig": b.sign_user(buyer) },
                      "items": [{ "price": { "id": "pri_hour" }, "quantity": 2 }] } }))
            .await
            .unwrap();
        assert_eq!((row(&db, buyer).extra, row(&db, other).extra), (2 * 3600, 0));
    }

    /// 조정 웹훅. 대기 중인 환불은 만들 때(created), 승인은 상태가 바뀔 때(updated) 온다.
    /// 장애 보상으로 표시한 조정(사유 "outage ...")
    fn outage(mut v: Value) -> Value {
        v["data"]["reason"] = json!("outage credit");
        v
    }

    fn adjustment(event: &str, id: &str, action: &str, status: &str, txn: &str, full: bool, total: &str) -> Value {
        let kind = if status == "pending_approval" { "adjustment.created" } else { "adjustment.updated" };
        json!({ "event_id": event, "event_type": kind, "occurred_at": "2026-10-10T00:00:00Z",
                "data": { "id": id, "action": action, "type": if full { "full" } else { "partial" }, "status": status,
                          "transaction_id": txn, "customer_id": "ctm_1", "currency_code": "USD", "totals": { "total": total } } })
    }

    /// 유닉스 초 → RFC 3339(UTC). 테스트가 지금 기준의 시각을 만들 때 쓴다.
    fn rfc3339(secs: i64) -> String {
        let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        let (y, m) = crate::meetings::year_month(secs);
        let month_start = parse_rfc3339(&format!("{y:04}-{m:02}-01T00:00:00Z")).unwrap().div_euclid(86_400);
        format!("{y:04}-{m:02}-{:02}T{:02}:{:02}:{:02}Z", days - month_start + 1, rem / 3600, rem % 3600 / 60, rem % 60)
    }

    #[tokio::test]
    async fn day_passes_last_a_day_stack_while_active_and_are_taken_back_on_refund() {
        let (db, b, uid, sig) = fixture();
        let day = || -> (i64, Option<i64>, String) {
            db.blocking(move |c| {
                c.query_row("SELECT day_seconds, day_until, plan FROM users WHERE id = ?1", params![uid], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
            })
            .unwrap()
        };
        let pass = |event: &str, txn: &str, at: i64| {
            json!({ "event_id": event, "event_type": "transaction.completed", "occurred_at": rfc3339(at),
                "data": { "id": txn, "origin": "web", "currency_code": "USD", "custom_data": { "user_id": uid.to_string(), "sig": sig },
                          "details": { "totals": { "grand_total": "1200" } },
                          "items": [{ "price": { "id": "pri_day" }, "quantity": 1 }] } })
        };
        let now = now_secs();
        // 3시간, 산 때부터 24시간. 같은 결제는 한 번만. 구독 요금제는 그대로다.
        let t0 = now - 3600;
        b.apply_event(&db, pass("d1", "txn_d1", t0)).await.unwrap();
        b.apply_event(&db, pass("d1b", "txn_d1", t0)).await.unwrap();
        assert_eq!(day(), (DAY_PASS_SECONDS, Some(t0 + DAY_PASS_LENGTH), "none".into()));
        assert_eq!(row(&db, uid).extra, 0);
        // 지금 일일권을 다 쓴 뒤 새로 사면 그 일일권이 지금 일일권이 된다. 끝나기 전에 산 옛 일일권(d1)을 환불해도
        // 지금 일일권(d2)의 시간은 그대로다.
        let t1 = now - 60;
        db.blocking(move |c| c.execute("UPDATE users SET day_seconds = 0 WHERE id = ?1", params![uid])).unwrap();
        b.apply_event(&db, pass("d2", "txn_d2", t1)).await.unwrap();
        assert_eq!(day(), (DAY_PASS_SECONDS, Some(t1 + DAY_PASS_LENGTH), "none".into()));
        b.apply_event(&db, adjustment("r1", "adj_d1", "refund", "approved", "txn_d1", true, "1200")).await.unwrap();
        assert_eq!(day(), (DAY_PASS_SECONDS, Some(t1 + DAY_PASS_LENGTH), "none".into()));
        // 지금 일일권의 고객 환불은 금액이 일부여도 그 일일권을 끝낸다.
        b.apply_event(&db, adjustment("r2", "adj_d2", "refund", "approved", "txn_d2", false, "500")).await.unwrap();
        let (secs, until, _) = day();
        assert!(secs == 0 && until.unwrap() <= now_secs());
        // 두 결제창에서 거의 동시에 사 남은 시간이 있을 때 온 결제는 낸 것이므로 합친다.
        b.apply_event(&db, pass("d3", "txn_d3", now - 30)).await.unwrap();
        b.apply_event(&db, pass("d4", "txn_d4", now - 20)).await.unwrap();
        assert_eq!(day(), (2 * DAY_PASS_SECONDS, Some(now - 20 + DAY_PASS_LENGTH), "none".into()));
        // 장애 보상은 돌려준 비율만큼만 뺀다(1200 중 600 → 3시간 중 1.5시간).
        b.apply_event(&db, outage(adjustment("r3", "adj_d4", "refund", "approved", "txn_d4", false, "600"))).await.unwrap();
        assert_eq!(day().0, 2 * DAY_PASS_SECONDS - DAY_PASS_SECONDS / 2);
        // 합쳐진 것 중 먼저 산 일일권을 고객 환불하면 그 시간도 지금 잔액에서 뺀다.
        b.apply_event(&db, adjustment("r4", "adj_d3", "refund", "approved", "txn_d3", false, "500")).await.unwrap();
        assert_eq!(day().0, DAY_PASS_SECONDS - DAY_PASS_SECONDS / 2);
        // 24시간이 다 지나서 온 결제는 받은 때부터 센다(낸 일일권을 못 쓰고 끝나지 않게).
        db.blocking(move |c| c.execute("UPDATE users SET day_seconds = 0, day_until = ?2 WHERE id = ?1", params![uid, now - 1]))
            .unwrap();
        b.apply_event(&db, pass("d5", "txn_d5", now - 3 * 86_400)).await.unwrap();
        let (secs, until, _) = day();
        assert!(secs == DAY_PASS_SECONDS && until.unwrap() >= now + DAY_PASS_LENGTH);
    }

    #[tokio::test]
    async fn refunded_hours_are_taken_back_once() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, json!({ "event_id": "e1", "event_type": "transaction.completed",
            "data": { "id": "txn_h", "origin": "web", "currency_code": "USD", "custom_data": { "user_id": uid.to_string(), "sig": sig },
                      "details": { "totals": { "grand_total": "1500" } },
                      "items": [{ "price": { "id": "pri_hour" }, "quantity": 5 }] } }))
            .await
            .unwrap();
        assert_eq!(row(&db, uid).extra, 5 * 3600);
        // 장애 보상: 승인 전에는 반영하지 않고, 승인되면 돌려준 몫(1500 중 600 → 5시간 중 2시간)만큼 뺀다. 같은 조정은 한 번만.
        b.apply_event(&db, outage(adjustment("a1", "adj_1", "refund", "pending_approval", "txn_h", false, "600"))).await.unwrap();
        assert_eq!(row(&db, uid).extra, 5 * 3600);
        b.apply_event(&db, outage(adjustment("a2", "adj_1", "refund", "approved", "txn_h", false, "600"))).await.unwrap();
        b.apply_event(&db, outage(adjustment("a3", "adj_1", "refund", "approved", "txn_h", false, "600"))).await.unwrap();
        assert_eq!(row(&db, uid).extra, 3 * 3600);
        // 나머지를 모두 돌려주면 남은 3시간을 빼되, 이미 쓴 만큼은 0에서 멈춘다.
        db.blocking(|c| c.execute("UPDATE users SET extra_seconds = 3600 WHERE id = ?1", params![uid])).unwrap();
        b.apply_event(&db, adjustment("a4", "adj_2", "refund", "approved", "txn_h", true, "900")).await.unwrap();
        assert_eq!(row(&db, uid).extra, 0);
        // 반영하지 않은 결제의 환불은 넘긴다.
        b.apply_event(&db, adjustment("a5", "adj_3", "refund", "approved", "txn_unknown", true, "900")).await.unwrap();
    }

    #[tokio::test]
    async fn a_refunded_subscription_payment_ends_the_plan() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        b.apply_event(&db, json!({ "event_id": "e2", "event_type": "transaction.completed",
            "data": { "id": "txn_s", "origin": "web", "subscription_id": "sub_1", "currency_code": "USD",
                      "custom_data": { "user_id": uid.to_string(), "sig": sig }, "details": { "totals": { "grand_total": "3900" } },
                      "items": [{ "price": { "id": "pri_pro" }, "quantity": 1 }] } }))
            .await
            .unwrap();
        // 장애 보상 일부 환불은 요금제를 그대로 둔다.
        let partial = b.apply_event(&db, outage(adjustment("a1", "adj_1", "refund", "approved", "txn_s", false, "1000"))).await.unwrap();
        assert_eq!((partial.revoked, row(&db, uid).plan.as_str()), (None, "pro"));
        // 차지백(또는 전액 환불)이면 끝내고, 웹훅 처리기가 Paddle 구독을 바로 해지한다. 같은 조정이 다시 와도 한 번만.
        let charged_back = b.apply_event(&db, adjustment("a2", "adj_2", "chargeback", "approved", "txn_s", true, "2900")).await.unwrap();
        assert_eq!((charged_back.revoked.as_deref(), row(&db, uid).plan.as_str()), (Some("sub_1"), "none"));
        let again = b.apply_event(&db, adjustment("a3", "adj_2", "chargeback", "approved", "txn_s", true, "2900")).await.unwrap();
        assert_eq!(again.revoked, None);
    }

    #[tokio::test]
    async fn customer_refunds_end_what_the_payment_gave_even_when_partial() {
        // 환불 정책: 쓴 시간과 결제 처리 비용을 뺀 나머지만 돌려준다. 그래서 일부만 돌려줘도 그 결제로 받은 이용은 끝난다.
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        b.apply_event(&db, paid("t1", uid, &sig, "txn_1", "web", &[("pri_basic", 1, None)], OCT)).await.unwrap();
        let out = b.apply_event(&db, adjustment("r1", "adj_1", "refund", "approved", "txn_1", false, "1100")).await.unwrap();
        assert_eq!((out.revoked.as_deref(), row(&db, uid).plan.as_str()), (Some("sub_1"), "none"));
        // 추가 시간: 일부 환불이어도 그 구매의 시간을 모두 뺀다(0에서 멈춤).
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, json!({ "event_id": "h1", "event_type": "transaction.completed",
            "data": { "id": "txn_h", "origin": "web", "currency_code": "USD", "custom_data": { "user_id": uid.to_string(), "sig": sig },
                      "details": { "totals": { "grand_total": "1500" } },
                      "items": [{ "price": { "id": "pri_hour" }, "quantity": 5 }] } }))
            .await
            .unwrap();
        db.blocking(move |c| c.execute("UPDATE users SET extra_seconds = 3 * 3600 WHERE id = ?1", params![uid])).unwrap();
        b.apply_event(&db, adjustment("r2", "adj_2", "refund", "approved", "txn_h", false, "775")).await.unwrap();
        assert_eq!(row(&db, uid).extra, 0);
    }

    fn queued(db: &Db, sub: &'static str) -> Option<(bool, bool)> {
        db.blocking(move |c| {
            c.query_row(
                "SELECT revoked, done FROM paddle_cancellations WHERE subscription_id = ?1",
                params![sub],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn refunding_an_upgrade_takes_its_hours_back_and_a_chargeback_ends_the_plan() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        b.apply_event(&db, paid("t1", uid, &sig, "txn_1", "web", &[("pri_basic", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, sub_event("e2", "subscription.updated", uid, &sig, "pri_pro", "2026-10-16T00:00:00Z", OCT))
            .await
            .unwrap();
        let upgrade = [("pri_pro", 1, Some("0.5")), ("pri_basic", -1, Some("0.5"))];
        b.apply_event(&db, paid("t2", uid, &sig, "txn_up", "subscription_update", &upgrade, OCT)).await.unwrap();
        assert_eq!(row(&db, uid).included, HALF_UP);
        // 차액을 모두 돌려주면 그 차액으로 더한 시간만 뺀다(요금제는 그대로).
        let out = b.apply_event(&db, adjustment("a1", "adj_1", "refund", "approved", "txn_up", true, "1900")).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.included, out.revoked), ("pro", BASIC, None));
        // 차액 거래라도 차지백이면 요금제를 끝내고 구독을 해지 대기열에 넣는다.
        b.apply_event(&db, paid("t3", uid, &sig, "txn_up2", "subscription_update", &upgrade, OCT)).await.unwrap();
        let out = b.apply_event(&db, adjustment("a2", "adj_2", "chargeback", "approved", "txn_up2", true, "1900")).await.unwrap();
        assert_eq!((out.revoked.as_deref(), row(&db, uid).plan.as_str()), (Some("sub_1"), "none"));
        assert_eq!(queued(&db, "sub_1"), Some((true, false)));
    }

    #[tokio::test]
    async fn a_revoked_subscription_stays_ended_until_paddle_confirms_the_cancellation() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        b.apply_event(&db, paid("t1", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, adjustment("a1", "adj_1", "chargeback", "approved", "txn_1", true, "1900")).await.unwrap();
        // 해지가 Paddle에 닿기 전에 온 변경(예: 해지 예약)으로 요금제가 살아나지 않는다.
        let mut later = sub_event("e2", "subscription.updated", uid, &sig, "pri_pro", "2026-10-11T00:00:00Z", OCT);
        later["data"]["scheduled_change"] = json!({ "action": "cancel", "effective_at": OCT.1 });
        b.apply_event(&db, later).await.unwrap();
        assert_eq!(row(&db, uid).plan, "none");
        assert_eq!(queued(&db, "sub_1"), Some((true, false)));
        // canceled 웹훅이 해지를 확인한다. 환불된 구독은 표시를 남겨 그 뒤에도 살아나지 않게 한다.
        b.apply_event(&db, json!({ "event_id": "e3", "event_type": "subscription.canceled", "occurred_at": "2026-10-12T00:00:00Z",
                                   "data": { "id": "sub_1", "status": "canceled" } }))
            .await
            .unwrap();
        assert_eq!(queued(&db, "sub_1"), Some((true, true)));
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.sub), ("none", None));
    }

    #[tokio::test]
    async fn replaced_and_deleted_account_subscriptions_wait_in_the_cancellation_queue() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("e1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        let mut second = sub_event("e2", "subscription.activated", uid, &sig, "pri_pro", "2026-10-02T00:00:00Z", OCT);
        second["data"]["id"] = json!("sub_2");
        b.apply_event(&db, second).await.unwrap();
        assert_eq!(queued(&db, "sub_1"), Some((false, false)));
        // 해지가 확인되면(이미 해지됨 포함) 끝난 것으로 남긴다(늦게 온 이벤트가 그 구독을 되살리지 않게).
        b.apply_event(&db, json!({ "event_id": "e3", "event_type": "subscription.canceled", "occurred_at": "2026-10-03T00:00:00Z",
                                   "data": { "id": "sub_1", "status": "canceled" } }))
            .await
            .unwrap();
        assert_eq!(queued(&db, "sub_1"), Some((true, true)), "kept as an ended marker");
        assert_eq!(row(&db, uid).sub.as_deref(), Some("sub_2"));
        // 계정을 지워도 대기열은 남는다(사용자 행과 따로).
        db.blocking(|c| {
            queue_cancellation(c, "sub_2", false, 1)?;
            c.execute("DELETE FROM users WHERE id = ?1", params![uid])
        })
        .unwrap();
        assert_eq!(queued(&db, "sub_2"), Some((false, false)));
    }

    fn paid_start(db: &Db, uid: i64) -> Option<i64> {
        db.blocking(move |c| c.query_row("SELECT period_paid_start FROM users WHERE id = ?1", params![uid], |r| r.get(0)))
            .unwrap()
    }

    #[tokio::test]
    async fn a_subscription_created_after_the_account_was_deleted_is_cancelled() {
        let (db, b, uid, sig) = fixture();
        db.blocking(move |c| c.execute("DELETE FROM users WHERE id = ?1", params![uid])).unwrap();
        b.apply_event(&db, sub_event("d1", "subscription.created", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT))
            .await
            .unwrap();
        b.apply_event(&db, paid("d2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        assert_eq!(queued(&db, "sub_1"), Some((false, false)));
    }

    #[tokio::test]
    async fn a_late_renewal_payment_keeps_upgrade_hours_in_either_order() {
        const UPN: (&str, &str) = ("2026-11-01T00:00:00Z", "2026-11-30T00:00:00Z");
        let up = [("pri_pro", 1, Some("0.9")), ("pri_basic", -1, Some("0.9"))];
        let upgrade = ((PRO - BASIC) as f64 * 0.9) as i64;
        // 갱신 구독 이벤트 → 올리기 → 차액 거래 → (늦게 온) 갱신 거래
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("a1", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("a2", uid, &sig, "txn_1", "web", &[("pri_basic", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, sub_event("a3", "subscription.updated", uid, &sig, "pri_basic", "2026-10-31T00:00:01Z", NOV)).await.unwrap();
        b.apply_event(&db, sub_event("a4", "subscription.updated", uid, &sig, "pri_pro", "2026-11-01T00:00:00Z", NOV)).await.unwrap();
        b.apply_event(&db, paid("a5", uid, &sig, "txn_up", "subscription_update", &up, UPN)).await.unwrap();
        assert_eq!(row(&db, uid).included, upgrade);
        b.apply_event(&db, paid("a6", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_basic", 1, None)], NOV)).await.unwrap();
        b.apply_event(&db, paid("a7", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_basic", 1, None)], NOV)).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.start, r.included), (parse_rfc3339(NOV.0), BASIC + upgrade));
        assert_eq!(paid_start(&db, uid), parse_rfc3339(NOV.0));

        // 차액 거래가 갱신 웹훅보다 먼저: 다음 기간에 붙여 두었다가 그 기간이 시작되면 센다.
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("b1", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("b2", uid, &sig, "txn_1", "web", &[("pri_basic", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, paid("b3", uid, &sig, "txn_up", "subscription_update", &up, UPN)).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.start, r.included), (parse_rfc3339(OCT.0), BASIC), "October is untouched");
        b.apply_event(&db, sub_event("b4", "subscription.updated", uid, &sig, "pri_pro", "2026-11-01T00:00:00Z", NOV)).await.unwrap();
        assert_eq!(row(&db, uid).included, upgrade);
        b.apply_event(&db, paid("b5", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_basic", 1, None)], NOV)).await.unwrap();
        assert_eq!(row(&db, uid).included, BASIC + upgrade);
        // 그 차액을 돌려주면 차액만큼만 뺀다.
        let out = b.apply_event(&db, adjustment("b6", "adj_up", "refund", "approved", "txn_up", true, "1900")).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.included, out.revoked), ("pro", BASIC, None));
    }

    #[tokio::test]
    async fn an_unknown_price_renewal_grants_the_current_plan() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("x1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("x2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, sub_event("x3", "subscription.updated", uid, &sig, "pri_legacy", "2026-10-31T00:00:01Z", NOV)).await.unwrap();
        b.apply_event(&db, paid("x4", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_legacy", 1, None)], NOV)).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.start, r.included), ("pro", parse_rfc3339(NOV.0), PRO));
        let recorded: i64 = db
            .blocking(|c| c.query_row("SELECT count(*) FROM paddle_transactions WHERE id = 'txn_nov'", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(recorded, 1);
    }

    #[tokio::test]
    async fn payments_of_other_subscriptions_do_not_move_the_current_period() {
        // 새 구독에 밀려 해지 대기 중인 예전 구독이 갱신돼도 지금 구독의 기간·시간은 그대로다.
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("r1", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        const P2: (&str, &str) = ("2026-10-15T00:00:00Z", "2026-11-15T00:00:00Z");
        let mut second = sub_event("r2", "subscription.activated", uid, &sig, "pri_pro", "2026-10-15T00:00:00Z", P2);
        second["data"]["id"] = json!("sub_2");
        b.apply_event(&db, second).await.unwrap();
        let mut first_payment = paid("r3", uid, &sig, "txn_2", "web", &[("pri_pro", 1, None)], P2);
        first_payment["data"]["subscription_id"] = json!("sub_2");
        b.apply_event(&db, first_payment).await.unwrap();
        b.apply_event(&db, paid("r4", uid, &sig, "txn_1nov", "subscription_recurring", &[("pri_basic", 1, None)], NOV)).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.sub.as_deref(), r.plan.as_str(), r.start, r.included), (Some("sub_2"), "pro", parse_rfc3339(P2.0), PRO));

        // 차지백 뒤 다시 구독: 새 구독의 첫 결제가 구독 이벤트보다 먼저 와도 새 구독이 그 시간을 받는다.
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("c1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("c2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, adjustment("c3", "adj_1", "chargeback", "approved", "txn_1", true, "1900")).await.unwrap();
        const P3: (&str, &str) = ("2026-10-12T00:00:00Z", "2026-11-12T00:00:00Z");
        let mut payment = paid("c4", uid, &sig, "txn_2", "web", &[("pri_basic", 1, None)], P3);
        payment["data"]["subscription_id"] = json!("sub_2");
        b.apply_event(&db, payment).await.unwrap();
        let mut created = sub_event("c5", "subscription.created", uid, &sig, "pri_basic", "2026-10-12T00:00:00Z", P3);
        created["data"]["id"] = json!("sub_2");
        b.apply_event(&db, created).await.unwrap();
        b.apply_event(&db, json!({ "event_id": "c6", "event_type": "subscription.canceled", "occurred_at": "2026-10-13T00:00:00Z",
                                   "data": { "id": "sub_1", "status": "canceled" } }))
            .await
            .unwrap();
        let r = row(&db, uid);
        assert_eq!((r.sub.as_deref(), r.plan.as_str(), r.included), (Some("sub_2"), "basic", BASIC));
        assert_eq!(paid_start(&db, uid), parse_rfc3339(P3.0));
    }

    #[tokio::test]
    async fn older_periods_never_move_the_period_back() {
        // 갱신 거래 뒤에 늦게 온 갱신 전 이벤트는 기간·시간을 되돌리지 않는다.
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("l1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("l2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, paid("l3", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_pro", 1, None)], NOV)).await.unwrap();
        b.apply_event(&db, sub_event("l4", "subscription.updated", uid, &sig, "pri_pro", "2026-10-20T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, sub_event("l5", "subscription.updated", uid, &sig, "pri_pro", "2026-10-31T00:00:01Z", NOV)).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.start, r.included), (parse_rfc3339(NOV.0), PRO));
        assert_eq!(paid_start(&db, uid), parse_rfc3339(NOV.0));

        // 마이그레이션한 구독자(이번 달을 낸 기간으로 둠): 그보다 먼저 시작한 Paddle 기간은 지난달 사용량을 다시 세지 않고 낸 것으로 남는다.
        let (db, b, uid, sig) = fixture();
        let (oct1, nov1) = (parse_rfc3339(OCT.0).unwrap(), parse_rfc3339("2026-11-01T00:00:00Z").unwrap());
        db.blocking(move |c| {
            c.execute(
                "UPDATE users SET plan = 'pro', paddle_subscription_id = 'sub_1', paddle_event_at = 1, period_start = ?2, period_end = ?3,
                   period_included = 72000, period_paid_start = ?2 WHERE id = ?1",
                params![uid, oct1, nov1],
            )
        })
        .unwrap();
        const PADDLE: (&str, &str) = ("2026-09-25T00:00:00Z", "2026-10-25T00:00:00Z");
        b.apply_event(&db, sub_event("m1", "subscription.updated", uid, &sig, "pri_pro", "2026-10-18T00:00:00Z", PADDLE)).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.start, r.included), (Some(oct1), PRO));
        assert_eq!(paid_start(&db, uid), Some(oct1));
        // 마이그레이션 뒤의 갱신은 결제가 끝나야 시간을 받는다.
        const NEXT: (&str, &str) = ("2026-10-25T00:00:00Z", "2026-11-25T00:00:00Z");
        b.apply_event(&db, sub_event("m2", "subscription.updated", uid, &sig, "pri_pro", "2026-10-25T00:00:01Z", NEXT)).await.unwrap();
        assert_eq!((row(&db, uid).start, row(&db, uid).included), (parse_rfc3339(NEXT.0), 0));
        b.apply_event(&db, paid("m3", uid, &sig, "txn_next", "subscription_recurring", &[("pri_pro", 1, None)], NEXT)).await.unwrap();
        assert_eq!(row(&db, uid).included, PRO);
    }

    #[tokio::test]
    async fn refunding_a_past_period_keeps_the_current_plan() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("p1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("p2", uid, &sig, "txn_oct", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, paid("p3", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_pro", 1, None)], NOV)).await.unwrap();
        // 지난달 결제를 모두 돌려줘도 따로 낸 이번 기간은 그대로다.
        let out = b.apply_event(&db, adjustment("a1", "adj_1", "refund", "approved", "txn_oct", true, "1900")).await.unwrap();
        assert_eq!((out.revoked, row(&db, uid).plan.as_str()), (None, "pro"));
        // 이번 기간의 결제를 돌려주면 끝낸다.
        let out = b.apply_event(&db, adjustment("a2", "adj_2", "refund", "approved", "txn_nov", true, "1900")).await.unwrap();
        assert_eq!((out.revoked.as_deref(), row(&db, uid).plan.as_str()), (Some("sub_1"), "none"));
    }

    fn status(db: &Db, uid: i64) -> Option<String> {
        db.blocking(move |c| c.query_row("SELECT paddle_status FROM users WHERE id = ?1", params![uid], |r| r.get(0)))
            .unwrap()
    }

    #[tokio::test]
    async fn the_dedicated_past_due_event_sets_the_status() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("p1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("p2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, sub_event("p3", "subscription.updated", uid, &sig, "pri_pro", "2026-10-31T00:00:01Z", NOV)).await.unwrap();
        let mut overdue = sub_event("p4", "subscription.past_due", uid, &sig, "pri_pro", "2026-10-31T00:00:09Z", NOV);
        overdue["data"]["status"] = json!("past_due");
        b.apply_event(&db, overdue).await.unwrap();
        assert_eq!((status(&db, uid).as_deref(), row(&db, uid).plan.as_str()), (Some("past_due"), "pro"));
        // 같은 변경을 알리는 조금 앞선 updated는 순서대로 버려도 상태는 그대로다.
        let mut earlier = sub_event("p5", "subscription.updated", uid, &sig, "pri_pro", "2026-10-31T00:00:08Z", NOV);
        earlier["data"]["status"] = json!("past_due");
        b.apply_event(&db, earlier).await.unwrap();
        assert_eq!(status(&db, uid).as_deref(), Some("past_due"));
        // 결제가 되면 시간이 생기고 active로 돌아온다.
        b.apply_event(&db, paid("p6", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_pro", 1, None)], NOV)).await.unwrap();
        b.apply_event(&db, sub_event("p7", "subscription.updated", uid, &sig, "pri_pro", "2026-11-03T00:00:00Z", NOV)).await.unwrap();
        assert_eq!((status(&db, uid).as_deref(), row(&db, uid).included), (Some("active"), PRO));
        // 멈춤·이어 쓰기도 그 이벤트로 상태가 바뀐다.
        let mut paused = sub_event("p8", "subscription.paused", uid, &sig, "pri_pro", "2026-11-04T00:00:00Z", NOV);
        paused["data"]["status"] = json!("paused");
        b.apply_event(&db, paused).await.unwrap();
        assert_eq!((status(&db, uid).as_deref(), row(&db, uid).plan.as_str()), (Some("paused"), "none"));
        b.apply_event(&db, sub_event("p9", "subscription.resumed", uid, &sig, "pri_pro", "2026-11-05T00:00:00Z", NOV)).await.unwrap();
        assert_eq!((status(&db, uid).as_deref(), row(&db, uid).plan.as_str()), (Some("active"), "pro"));
    }

    /// 마이그레이션한 구독자: 이번 달을 낸 기간으로 두고, 마지막으로 받은 이벤트 시각을 둔다.
    fn migrated(db: &Db, uid: i64, plan: &'static str, last_event: &str) {
        let (oct1, nov1) = (parse_rfc3339(OCT.0).unwrap(), parse_rfc3339("2026-11-01T00:00:00Z").unwrap());
        let last = parse_rfc3339_ms(last_event).unwrap();
        db.blocking(move |c| {
            c.execute(
                "UPDATE users SET plan = ?4, paddle_subscription_id = 'sub_1', paddle_event_at = ?5, period_start = ?2, period_end = ?3,
                   period_included = ?6, period_paid_start = ?2 WHERE id = ?1",
                params![uid, oct1, nov1, plan, last, included_seconds(plan)],
            )
        })
        .unwrap();
    }

    #[tokio::test]
    async fn migrated_subscribers_keep_what_their_first_paddle_period_was_paid_at() {
        // Paddle 기간(10/5~11/5)은 마이그레이션 전에 시작해 Pro로 냈다. 그 기간에 Basic으로 내려도(차액 없음) Pro 시간이 남는다.
        const P: (&str, &str) = ("2026-10-05T00:00:00Z", "2026-11-05T00:00:00Z");
        for first in ["pri_pro", "pri_basic"] {
            let (db, b, uid, sig) = fixture();
            migrated(&db, uid, "pro", "2026-10-05T00:00:02Z");
            b.apply_event(&db, sub_event("m1", "subscription.updated", uid, &sig, first, "2026-10-18T00:00:00Z", P)).await.unwrap();
            b.apply_event(&db, sub_event("m2", "subscription.updated", uid, &sig, "pri_basic", "2026-10-20T00:00:00Z", P)).await.unwrap();
            let r = row(&db, uid);
            assert_eq!((r.plan.as_str(), r.start, r.included), ("basic", parse_rfc3339(P.0), PRO), "first event {first}");
            assert_eq!(paid_start(&db, uid), parse_rfc3339(P.0));
        }
        // Basic으로 낸 기간에 Pro로 올리면 차액 거래의 몫만 더한다(한 번만).
        let (db, b, uid, sig) = fixture();
        migrated(&db, uid, "basic", "2026-10-05T00:00:02Z");
        b.apply_event(&db, sub_event("u1", "subscription.updated", uid, &sig, "pri_basic", "2026-10-18T00:00:00Z", P)).await.unwrap();
        b.apply_event(&db, sub_event("u2", "subscription.updated", uid, &sig, "pri_pro", "2026-10-20T00:00:00Z", P)).await.unwrap();
        assert_eq!(row(&db, uid).included, BASIC);
        let up = [("pri_pro", 1, Some("0.5")), ("pri_basic", -1, Some("0.5"))];
        b.apply_event(&db, paid("u3", uid, &sig, "txn_up", "subscription_update", &up, ("2026-10-20T00:00:00Z", P.1))).await.unwrap();
        let mut later = sub_event("u4", "subscription.updated", uid, &sig, "pri_pro", "2026-10-22T00:00:00Z", P);
        later["data"]["scheduled_change"] = json!({ "action": "cancel", "effective_at": P.1 });
        b.apply_event(&db, later).await.unwrap();
        assert_eq!(row(&db, uid).included, HALF_UP);
    }

    #[tokio::test]
    async fn subscription_order_is_compared_only_within_a_subscription() {
        // 예전 구독의 해지 이벤트보다 먼저 일어난 새 구독도 받는다.
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("c1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("c2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, adjustment("c3", "adj_1", "chargeback", "approved", "txn_1", true, "1900")).await.unwrap();
        const P2: (&str, &str) = ("2026-10-12T00:00:00Z", "2026-11-12T00:00:00Z");
        let mut payment = paid("c4", uid, &sig, "txn_2", "web", &[("pri_basic", 1, None)], P2);
        payment["data"]["subscription_id"] = json!("sub_2");
        b.apply_event(&db, payment).await.unwrap();
        b.apply_event(&db, json!({ "event_id": "c5", "event_type": "subscription.canceled", "occurred_at": "2026-10-12T01:00:00Z",
                                   "data": { "id": "sub_1", "status": "canceled" } }))
            .await
            .unwrap();
        let mut created = sub_event("c6", "subscription.created", uid, &sig, "pri_basic", "2026-10-12T00:00:00Z", P2);
        created["data"]["id"] = json!("sub_2");
        b.apply_event(&db, created).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.sub.as_deref(), r.included), ("basic", Some("sub_2"), BASIC));
        // 해지된 구독의 늦은 이벤트는 비어 있는 자리에 다시 들어오지 않는다.
        b.apply_event(&db, json!({ "event_id": "c7", "event_type": "subscription.canceled", "occurred_at": "2026-10-20T00:00:00Z",
                                   "data": { "id": "sub_2", "status": "canceled" } }))
            .await
            .unwrap();
        let mut stale = sub_event("c8", "subscription.updated", uid, &sig, "pri_basic", "2026-10-19T00:00:00Z", P2);
        stale["data"]["id"] = json!("sub_2");
        b.apply_event(&db, stale).await.unwrap();
        assert_eq!((row(&db, uid).plan.as_str(), row(&db, uid).sub), ("none", None));

        // 처음 구독이 둘(늦게 생긴 것이 먼저 도착): 나중에 도착한 먼저 생긴 구독을 해지 대기열에 넣는다.
        let (db, b, uid, sig) = fixture();
        let mut second = sub_event("o1", "subscription.created", uid, &sig, "pri_pro", "2026-10-01T00:05:00Z", OCT);
        second["data"]["id"] = json!("sub_2");
        b.apply_event(&db, second).await.unwrap();
        let out = b.apply_event(&db, sub_event("o2", "subscription.created", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        assert_eq!((row(&db, uid).sub.as_deref(), out.replaced.as_deref()), (Some("sub_2"), Some("sub_1")));
        assert_eq!(queued(&db, "sub_1"), Some((false, false)));
    }

    #[tokio::test]
    async fn a_refund_that_arrives_before_its_payment_is_retried() {
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("h0", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        // 방금 일어난 환불(occurred_at 없음 = 받은 때)인데 그 결제가 아직 기록되지 않았다: Paddle이 다시 보내게 하고 아무것도 남기지 않는다.
        let mut refund = adjustment("h1", "adj_h", "refund", "approved", "txn_h", true, "1500");
        refund.as_object_mut().unwrap().remove("occurred_at");
        assert!(b.apply_event(&db, refund.clone()).await.unwrap().retry);
        let hours = json!({ "event_id": "h2", "event_type": "transaction.completed",
            "data": { "id": "txn_h", "origin": "web", "currency_code": "USD", "custom_data": { "user_id": uid.to_string(), "sig": sig },
                      "details": { "totals": { "grand_total": "1500" } }, "items": [{ "price": { "id": "pri_hour" }, "quantity": 5 }] } });
        b.apply_event(&db, hours).await.unwrap();
        assert_eq!(row(&db, uid).extra, 5 * 3600);
        // 다시 온 같은 이벤트는 이제 반영되고, 그 뒤로는 한 번만.
        assert!(!b.apply_event(&db, refund.clone()).await.unwrap().retry);
        assert_eq!(row(&db, uid).extra, 0);
        b.apply_event(&db, refund).await.unwrap();
        assert_eq!(row(&db, uid).extra, 0);
    }

    /// 모든 순서
    fn orders(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![vec![]];
        }
        let mut out = vec![];
        for p in orders(n - 1) {
            for i in 0..=p.len() {
                let mut q = p.clone();
                q.insert(i, n - 1);
                out.push(q);
            }
        }
        out
    }

    #[tokio::test]
    async fn first_purchase_and_upgrade_in_any_arrival_order() {
        let up = [("pri_pro", 1, Some("0.5")), ("pri_basic", -1, Some("0.5"))];
        let mut bad = vec![];
        for up_period in [OCT, ("2026-10-16T00:00:00Z", OCT.1)] {
            for order in orders(5) {
                let (db, b, uid, sig) = fixture();
                let events = [
                    sub_event("e1", "subscription.created", uid, &sig, "pri_basic", "2026-10-01T00:00:00.100Z", OCT),
                    sub_event("e2", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00.200Z", OCT),
                    paid("e3", uid, &sig, "txn_1", "web", &[("pri_basic", 1, None)], OCT),
                    sub_event("e4", "subscription.updated", uid, &sig, "pri_pro", "2026-10-16T00:00:00Z", OCT),
                    paid("e5", uid, &sig, "txn_up", "subscription_update", &up, up_period),
                ];
                for &i in &order {
                    b.apply_event(&db, events[i].clone()).await.unwrap();
                }
                let r = row(&db, uid);
                if !(r.plan == "pro" && r.start == parse_rfc3339(OCT.0) && r.included == HALF_UP && paid_start(&db, uid) == r.start) {
                    bad.push(format!("{up_period:?} {order:?}: {r:?} paid={:?}", paid_start(&db, uid)));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn renewal_and_upgrade_in_any_arrival_order() {
        const UPN: (&str, &str) = ("2026-11-01T00:00:00Z", "2026-11-30T00:00:00Z");
        let up = [("pri_pro", 1, Some("0.9")), ("pri_basic", -1, Some("0.9"))];
        let want = BASIC + ((PRO - BASIC) as f64 * 0.9) as i64;
        let mut bad = vec![];
        for up_period in [UPN, NOV] {
            for order in orders(4) {
                let (db, b, uid, sig) = fixture();
                b.apply_event(&db, sub_event("a1", "subscription.activated", uid, &sig, "pri_basic", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
                b.apply_event(&db, paid("a2", uid, &sig, "txn_1", "web", &[("pri_basic", 1, None)], OCT)).await.unwrap();
                let events = [
                    sub_event("r1", "subscription.updated", uid, &sig, "pri_basic", "2026-10-31T00:00:01Z", NOV),
                    paid("r2", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_basic", 1, None)], NOV),
                    sub_event("r3", "subscription.updated", uid, &sig, "pri_pro", "2026-11-01T00:00:00Z", NOV),
                    paid("r4", uid, &sig, "txn_up", "subscription_update", &up, up_period),
                ];
                for &i in &order {
                    b.apply_event(&db, events[i].clone()).await.unwrap();
                }
                let r = row(&db, uid);
                if !(r.plan == "pro" && r.start == parse_rfc3339(NOV.0) && r.included == want && paid_start(&db, uid) == r.start) {
                    bad.push(format!("{up_period:?} {order:?}: {r:?} paid={:?}", paid_start(&db, uid)));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn downgrade_and_renewal_in_any_arrival_order() {
        let mut bad = vec![];
        for order in orders(3) {
            let (db, b, uid, sig) = fixture();
            b.apply_event(&db, sub_event("a1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
            b.apply_event(&db, paid("a2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
            let events = [
                sub_event("d1", "subscription.updated", uid, &sig, "pri_basic", "2026-10-20T00:00:00Z", OCT),
                sub_event("d2", "subscription.updated", uid, &sig, "pri_basic", "2026-10-31T00:00:01Z", NOV),
                paid("d3", uid, &sig, "txn_nov", "subscription_recurring", &[("pri_basic", 1, None)], NOV),
            ];
            for &i in &order {
                b.apply_event(&db, events[i].clone()).await.unwrap();
            }
            let r = row(&db, uid);
            if !(r.plan == "basic" && r.start == parse_rfc3339(NOV.0) && r.included == BASIC && paid_start(&db, uid) == r.start) {
                bad.push(format!("{order:?}: {r:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn resubscribing_in_any_arrival_order() {
        const P2: (&str, &str) = ("2026-11-05T00:00:00Z", "2026-12-05T00:00:00Z");
        let mut bad = vec![];
        for order in orders(4) {
            let (db, b, uid, sig) = fixture();
            b.apply_event(&db, sub_event("a1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
            b.apply_event(&db, paid("a2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
            let with_id = |mut v: Value, field: &str| {
                v["data"][field] = json!("sub_2");
                v
            };
            let events = [
                json!({ "event_id": "x0", "event_type": "subscription.canceled", "occurred_at": "2026-10-31T00:00:00Z",
                        "data": { "id": "sub_1", "status": "canceled", "custom_data": { "user_id": uid.to_string(), "sig": sig } } }),
                with_id(sub_event("x1", "subscription.created", uid, &sig, "pri_basic", "2026-11-05T00:00:00.100Z", P2), "id"),
                with_id(sub_event("x2", "subscription.activated", uid, &sig, "pri_basic", "2026-11-05T00:00:00.200Z", P2), "id"),
                with_id(paid("x3", uid, &sig, "txn_2", "web", &[("pri_basic", 1, None)], P2), "subscription_id"),
            ];
            for &i in &order {
                b.apply_event(&db, events[i].clone()).await.unwrap();
            }
            let r = row(&db, uid);
            if !(r.plan == "basic"
                && r.sub.as_deref() == Some("sub_2")
                && r.start == parse_rfc3339(P2.0)
                && r.included == BASIC
                && paid_start(&db, uid) == r.start)
            {
                bad.push(format!("{order:?}: {r:?} queue={:?}", queued(&db, "sub_2")));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn refunds_of_payments_we_skipped_are_not_retried() {
        let recent = |id: &str, adj: &str, txn: &str| {
            let mut refund = adjustment(id, adj, "refund", "approved", txn, true, "1900");
            refund.as_object_mut().unwrap().remove("occurred_at");
            refund
        };
        // 계정을 지우면 결제 기록은 함께 지워지지만, 그 뒤 온 환불은 바로 넘긴다(며칠 동안 다시 받지 않는다).
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("d1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("d2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        db.blocking(move |c| {
            skip_payments_of(c, uid, 1)?;
            c.execute("DELETE FROM users WHERE id = ?1", params![uid])
        })
        .unwrap();
        assert!(!b.apply_event(&db, recent("d3", "adj_1", "txn_1")).await.unwrap().retry);
        // 우리 상품이 아닌 결제, 사용자를 모르는 결제도 같다. 기록되지 않은 결제의 환불만 다시 받는다.
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("k1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        let mut charge = paid("k2", uid, &sig, "txn_c", "subscription_charge", &[("pri_other", 1, None)], OCT);
        charge["data"]["items"] = json!([{ "price": { "id": "pri_other" }, "quantity": 1 }]);
        b.apply_event(&db, charge).await.unwrap();
        assert!(!b.apply_event(&db, recent("k3", "adj_c", "txn_c")).await.unwrap().retry);
        let mut stranger = paid("k4", uid, &sig, "txn_s", "web", &[("pri_pro", 1, None)], OCT);
        stranger["data"].as_object_mut().unwrap().remove("custom_data");
        stranger["data"]["subscription_id"] = json!("sub_unknown");
        stranger["data"]["customer_id"] = json!("ctm_unknown");
        b.apply_event(&db, stranger).await.unwrap();
        assert!(!b.apply_event(&db, recent("k5", "adj_s", "txn_s")).await.unwrap().retry);
        assert!(b.apply_event(&db, recent("k6", "adj_n", "txn_not_yet")).await.unwrap().retry);
    }

    #[tokio::test]
    async fn a_new_subscription_replaces_an_ended_one_with_whichever_event_comes_first() {
        // 지금 구독이 차지백으로 끝났다. 새 구독의 나중 변경(Pro)이 created·activated(Basic)보다 먼저 와도 그 변경이 남는다.
        let (db, b, uid, sig) = fixture();
        b.apply_event(&db, sub_event("r1", "subscription.activated", uid, &sig, "pri_pro", "2026-10-01T00:00:00Z", OCT)).await.unwrap();
        b.apply_event(&db, paid("r2", uid, &sig, "txn_1", "web", &[("pri_pro", 1, None)], OCT)).await.unwrap();
        b.apply_event(&db, adjustment("r3", "adj_1", "chargeback", "approved", "txn_1", true, "1900")).await.unwrap();
        const P2: (&str, &str) = ("2026-10-12T00:00:00Z", "2026-11-12T00:00:00Z");
        let second = |mut v: Value| {
            v["data"]["id"] = json!("sub_2");
            v
        };
        b.apply_event(&db, second(sub_event("r4", "subscription.updated", uid, &sig, "pri_pro", "2026-10-12T00:01:00Z", P2))).await.unwrap();
        b.apply_event(&db, second(sub_event("r5", "subscription.created", uid, &sig, "pri_basic", "2026-10-12T00:00:00Z", P2))).await.unwrap();
        b.apply_event(&db, second(sub_event("r6", "subscription.activated", uid, &sig, "pri_basic", "2026-10-12T00:00:00.1Z", P2))).await.unwrap();
        let r = row(&db, uid);
        assert_eq!((r.plan.as_str(), r.sub.as_deref()), ("pro", Some("sub_2")));
        assert_eq!(queued(&db, "sub_1"), Some((true, false)));
    }
}
