//! 회의 기록(예약·지난 회의), 번역 사용량과 한도, 요금제·결제 API.
//! 회의실 자체(참가자·신호)는 메모리의 `Rooms`가 맡고, 여기서는 오래 남겨야 하는 것만 DB에 둔다.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::account::require_user;
use crate::billing::{self, included_seconds};
use crate::db::{Db, OptionalExtension, now_secs, params};
use crate::mail::{Copy, Lang};
use crate::protocol::RoomSettings;
use crate::room::{self, Metered, Rooms};

const MAX_TITLE: usize = 80;
const MAX_GUESTS: usize = 20;
/// 예약 시각보다 이만큼 지나면 링크로 방을 다시 열지 않는다.
const RESERVED_GRACE: i64 = 24 * 3600;

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "code": code }))).into_response()
}

// ── 날짜(UTC 달력) ─────────────────────────────────────

/// 유닉스 초 → (연, 월). 사용 기록(최근 6달)은 UTC 달력 달로 묶는다. 포함 시간은 결제 기간마다 채운다(`usage_of`).
pub fn year_month(secs: i64) -> (i64, i64) {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m)
}

fn period(secs: i64) -> String {
    let (y, m) = year_month(secs);
    format!("{y:04}-{m:02}")
}

/// 다음 달 1일 0시(UTC). 번역 시간이 다시 채워지는 때.
pub fn next_reset(secs: i64) -> i64 {
    let (y, m) = year_month(secs);
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    billing::parse_rfc3339(&format!("{ny:04}-{nm:02}-01T00:00:00Z")).unwrap_or(secs)
}

// ── 사용량·한도 ───────────────────────────────────────

/// 이번 결제 기간의 사용량. 포함 시간은 Paddle 결제 기간마다 다시 채운다(달력 달이 아니라).
struct Usage {
    plan: String,
    /// 이번 기간에 포함된 시간(요금제를 올리면 남은 비율만큼 늘어난다). 요금제가 없으면 0.
    included: i64,
    /// 이번 기간에 포함 시간에서 쓴 시간(사 둔 추가 시간으로 낸 몫은 빼고)
    used: i64,
    extra: i64,
    /// 남은 일일권 시간(끝났으면 0)과 끝나는 때
    day: i64,
    day_until: Option<i64>,
    start: Option<i64>,
    end: Option<i64>,
    /// 이번 기간의 결제가 끝나 포함 시간을 받았는지(Paddle 구독이 아니면 언제나 true). 갱신 결제가 밀리면 false.
    paid: bool,
    /// 갱신 결제가 실패해 Paddle이 구독을 past_due로 두었다. 첫 결제를 확인하는 몇 초(`paid`만 false)와 구별한다.
    past_due: bool,
}

fn usage_of(c: &rusqlite::Connection, uid: i64, now: i64) -> rusqlite::Result<Option<Usage>> {
    type Row = (String, i64, Option<i64>, Option<i64>, i64, bool, bool, Option<i64>, Option<String>, i64, Option<i64>);
    let row: Option<Row> = c
        .query_row(
            "SELECT plan, extra_seconds, period_start, period_end, period_included,
                    paddle_subscription_id IS NOT NULL, plan_canceled, period_paid_start, paddle_status, day_seconds, day_until
             FROM users WHERE id = ?1",
            params![uid],
            |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?,
                    r.get(10)?,
                ))
            },
        )
        .optional()?;
    let Some((plan, extra, mut start, mut end, mut included, subscribed, canceled, paid_start, status, day_seconds, day_until)) = row
    else {
        return Ok(None);
    };
    let day = if day_until.is_some_and(|u| u > now) { day_seconds.max(0) } else { 0 };
    // Paddle 구독은 결제가 끝난 기간에만 포함 시간을 준다(billing: transaction.completed). 여기서 채우는 것은
    // 결제 없이 켠 요금제(운영자·개발 모드)뿐이고, 해지를 예약했으면 넘기지 않는다.
    if plan != "none" && !subscribed && !canceled {
        let next = match (start, end) {
            // 기간이 끝나면 같은 길이로 넘겨 다시 채운다.
            (Some(s), Some(e)) if e > s && now >= e => {
                let len = e - s;
                let s = s + (now - s) / len * len;
                Some((s, s + len))
            }
            (Some(_), Some(_)) => None,
            // 기간을 모르는 요금제(결제 없이 켠 운영자 계정 등): 이번 달을 기간으로 둔다.
            _ => {
                let month_start = billing::parse_rfc3339(&format!("{}-01T00:00:00Z", period(now))).unwrap_or(now);
                Some((month_start, next_reset(now)))
            }
        };
        if let Some((s, e)) = next {
            included = included_seconds(&plan);
            c.execute(
                "UPDATE users SET period_start = ?2, period_end = ?3, period_included = ?4 WHERE id = ?1",
                params![uid, s, e, included],
            )?;
            (start, end) = (Some(s), Some(e));
        }
    }
    let used: i64 = match start {
        Some(s) => c.query_row(
            "SELECT COALESCE(SUM(seconds - from_extra - from_day), 0) FROM usage WHERE user_id = ?1 AND recorded_at >= ?2",
            params![uid, s],
            |r| r.get(0),
        )?,
        None => 0,
    };
    let included = if plan == "none" { 0 } else { included };
    let paid = !subscribed || (start.is_some() && paid_start == start);
    let past_due = plan != "none" && subscribed && status.as_deref() == Some("past_due");
    Ok(Some(Usage { plan, included, used, extra, day, day_until, start, end, paid, past_due }))
}

impl Usage {
    /// 남은 시간: 이번 기간에 포함된 남은 시간 + 남은 일일권 + 사 둔 추가 시간.
    fn remaining(&self) -> i64 {
        (self.included - self.used).max(0) + self.day + self.extra
    }

    /// 화면에 보이는 요금제(`billing::EFFECTIVE_PLAN_SQL`과 같다): 구독이 없고 일일권이 남아 있으면 "day".
    fn shown_plan(&self, now: i64) -> &str {
        if self.plan == "none" && self.day_until.is_some_and(|u| u > now) { "day" } else { &self.plan }
    }
}

/// 잰 번역 시간을 기록하고, 시간이 다 떨어진 주최자 목록을 돌려준다.
/// 남은 일일권을 가장 먼저 쓴다(24시간 뒤 사라지므로). 그다음 포함된 시간, 넘긴 만큼은 사 둔 추가 시간에서 빼고,
/// 일일권·추가 시간으로 낸 몫을 사용량에 함께 적는다(나중에 포함 시간이 늘어도 그 몫을 포함 시간에서 또 빼지 않게).
pub fn record_usage(
    c: &mut rusqlite::Connection,
    metered: &[Metered],
    check: &HashSet<String>,
    now: i64,
) -> rusqlite::Result<HashSet<String>> {
    let tx = c.transaction()?;
    let mut hosts: HashSet<String> = check.clone();
    for m in metered {
        hosts.insert(m.host.clone());
        tx.execute(
            "UPDATE meetings SET peak_participants = MAX(peak_participants, ?2) WHERE code = ?1",
            params![m.code, m.peak as i64],
        )?;
        if m.seconds == 0 {
            continue;
        }
        let Ok(uid) = m.host.parse::<i64>() else {
            continue;
        };
        let Some(before) = usage_of(&tx, uid, now)? else {
            continue;
        };
        let from_day = m.seconds.clamp(0, before.day);
        let left = (before.included - before.used).max(0);
        let from_extra = (m.seconds - from_day - left).clamp(0, before.extra.max(0));
        tx.execute(
            "INSERT INTO usage (user_id, meeting_code, period, seconds, recorded_at, from_extra, from_day) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![uid, m.code, period(now), m.seconds, now, from_extra, from_day],
        )?;
        if from_day > 0 {
            tx.execute(
                "UPDATE users SET day_seconds = MAX(0, day_seconds - ?2) WHERE id = ?1",
                params![uid, from_day],
            )?;
        }
        if from_extra > 0 {
            tx.execute(
                "UPDATE users SET extra_seconds = MAX(0, extra_seconds - ?2) WHERE id = ?1",
                params![uid, from_extra],
            )?;
        }
        tx.execute(
            "UPDATE meetings SET translated_seconds = translated_seconds + ?2,
               started_at = COALESCE(started_at, ?3), last_active_at = ?4 WHERE code = ?1",
            params![m.code, m.seconds, now - m.seconds, now],
        )?;
    }
    let mut out = HashSet::new();
    for h in hosts {
        let Ok(uid) = h.parse::<i64>() else { continue };
        if usage_of(&tx, uid, now)?.is_none_or(|u| u.remaining() <= 0) {
            out.insert(h);
        }
    }
    tx.commit()?;
    Ok(out)
}

/// 비워져 지워진 방: 한 번이라도 번역이 돌았으면 끝난 회의로 남기고, 아니면(잠깐 열었다 닫은 즉석 회의) 지운다.
/// 예약 회의는 시작 전이면 그대로 둔다(링크를 미리 열어 본 경우).
pub fn end_rooms(c: &rusqlite::Connection, codes: &[String], now: i64) -> rusqlite::Result<()> {
    for code in codes {
        // 예약 회의는 정한 시간이 지나기 전에는 끝내지 않는다. 미리 열어 보거나 30분 넘게 쉬어도 같은 링크가 정한 시각에 열린다.
        c.execute(
            "UPDATE meetings SET ended_at = COALESCE(last_active_at, ?2) WHERE code = ?1 AND ended_at IS NULL AND started_at IS NOT NULL
               AND (starts_at IS NULL OR starts_at + duration_min * 60 <= ?2)",
            params![code, now],
        )?;
        // 시작 시각 전에 미리 열어 보기만 한 것(모든 활동이 시작 시각 전)은 회의 길이에 넣지 않는다.
        // 조금 일찍 시작해 정한 시간까지 이어진 실제 회의는 그대로 둔다.
        c.execute(
            "UPDATE meetings SET started_at = NULL, last_active_at = NULL
             WHERE code = ?1 AND ended_at IS NULL AND starts_at IS NOT NULL AND last_active_at < starts_at",
            params![code],
        )?;
        c.execute(
            "DELETE FROM meetings WHERE code = ?1 AND starts_at IS NULL AND started_at IS NULL",
            params![code],
        )?;
    }
    Ok(())
}

/// 주최자가 나가기 버튼으로 끝낸 방: 예약 회의도 정한 시간과 상관없이 끝난 회의로 남겨
/// 링크로 다시 열리지 않게 한다. 시작도 안 한 즉석 회의는 지운다.
pub fn end_rooms_now(c: &rusqlite::Connection, codes: &[String], now: i64) -> rusqlite::Result<()> {
    for code in codes {
        c.execute(
            "UPDATE meetings SET ended_at = COALESCE(last_active_at, started_at, ?2)
             WHERE code = ?1 AND ended_at IS NULL AND (started_at IS NOT NULL OR starts_at IS NOT NULL)",
            params![code, now],
        )?;
        c.execute(
            "DELETE FROM meetings WHERE code = ?1 AND starts_at IS NULL AND started_at IS NULL",
            params![code],
        )?;
    }
    Ok(())
}

pub fn spawn_end_now(state: &AppState, ended: Vec<String>) {
    if ended.is_empty() {
        return;
    }
    let db = state.auth.db().clone();
    tokio::spawn(async move {
        let now = now_secs();
        if let Err(err) = db.call(move |c| end_rooms_now(c, &ended, now)).await {
            tracing::error!(%err, "could not record meetings ended by the host");
        }
    });
}

/// 서버가 다시 시작되면 메모리의 즉석 회의 방은 모두 사라진다. 그 회의 기록을 닫아 둔다
/// (시작했으면 마지막 활동 시각으로 끝내고, 시작도 안 했으면 지운다). 예약 회의는 링크로 다시 열 수 있으므로 그대로 둔다.
pub fn close_orphans(db: &Db) -> rusqlite::Result<()> {
    let now = now_secs();
    db.blocking(move |c| {
        c.execute(
            "UPDATE meetings SET ended_at = COALESCE(last_active_at, started_at, ?1)
             WHERE starts_at IS NULL AND started_at IS NOT NULL AND ended_at IS NULL",
            params![now],
        )?;
        c.execute(
            "DELETE FROM meetings WHERE starts_at IS NULL AND started_at IS NULL AND ended_at IS NULL",
            [],
        )?;
        Ok(())
    })
}

/// 메모리에 없는 회의 코드: 예약된(아직 안 끝난) 회의면 방을 연다.
pub async fn ensure_room(db: &Db, rooms: &Rooms, code: &str) {
    if rooms.exists(code) {
        return;
    }
    let c = code.to_owned();
    let now = now_secs();
    let found: Option<(i64, bool)> = db
        .call(move |conn| {
            conn.query_row(
                "SELECT owner_id, translation FROM meetings
                 WHERE code = ?1 AND ended_at IS NULL AND starts_at IS NOT NULL AND starts_at + duration_min * 60 + ?2 > ?3",
                params![c, RESERVED_GRACE, now],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    if let Some((owner, translation)) = found {
        rooms.open_reserved(&owner.to_string(), code, RoomSettings { translation });
    }
}

// ── 즉석 회의 만들기·로비 정보 ─────────────────────────

#[derive(Deserialize)]
pub struct CreateRoomReq {
    translation: bool,
    #[serde(default)]
    kind: String,
}

fn clean_kind(k: &str) -> &'static str {
    match k {
        "inPerson" => "inPerson",
        "live" => "live",
        _ => "meeting",
    }
}

/// 로그인한 주최자만, 요금제가 있을 때만 회의를 만들 수 있다.
pub async fn create_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateRoomReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let db = state.auth.db().clone();
    if active_plan(&db, uid).await == "none" {
        return error(StatusCode::PAYMENT_REQUIRED, "plan");
    }
    let code = match state.rooms.create(
        &uid.to_string(),
        RoomSettings {
            translation: req.translation,
        },
    ) {
        Ok(code) => code,
        Err(_) => return error(StatusCode::TOO_MANY_REQUESTS, "too_many_rooms"),
    };
    let kind = clean_kind(&req.kind);
    let c = code.clone();
    let translation = req.translation;
    let _ = db
        .call(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO meetings (code, owner_id, kind, translation, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![c, uid, kind, translation, now_secs()],
            )
        })
        .await;
    Json(json!({ "code": code })).into_response()
}

/// 로비에서 입장 전에 보는 공개 정보: 번역 사용 여부, 회의 이름, 주최자 이름.
/// 코드를 아는 사람(링크를 받은 사람)에게만 의미가 있으므로 그 이상은 내보내지 않는다.
pub async fn room_info(State(state): State<AppState>, Path(code): Path<String>) -> Response {
    if !room::is_valid_room_code(&code) {
        return StatusCode::NOT_FOUND.into_response();
    }
    ensure_room(state.auth.db(), &state.rooms, &code).await;
    let Some(settings) = state.rooms.settings(&code) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let c = code.clone();
    type Meta = (String, String, String, String, String);
    let meta: Option<Meta> = state
        .auth
        .db()
        .call(move |conn| {
            conn.query_row(
                "SELECT m.title, COALESCE(NULLIF(u.display_name, ''), u.name), m.kind, m.lang_a, m.lang_b
                 FROM meetings m JOIN users u ON u.id = m.owner_id WHERE m.code = ?1",
                params![c],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    let (title, host, kind, lang_a, lang_b) = meta.unwrap_or_default();
    // 예약할 때 정한 언어 쌍과 종류(대면·자막 화면은 주최자 기기에서만 돌아 손님이 들어올 수 없다).
    let kind = if kind.is_empty() { "meeting".to_owned() } else { kind };
    Json(json!({
        "translation": settings.translation, "title": title, "host": host,
        "kind": kind, "langA": lang_a, "langB": lang_b,
    }))
    .into_response()
}

// ── 예약 회의 ─────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingReq {
    title: String,
    #[serde(default)]
    kind: String,
    starts_at: i64,
    #[serde(default = "default_duration")]
    duration_min: i64,
    #[serde(default)]
    lang_a: String,
    #[serde(default)]
    lang_b: String,
    #[serde(default)]
    guests: Vec<String>,
    /// 저장하면서 손님들에게 초대 메일을 보낸다.
    #[serde(default)]
    invite: bool,
}

fn default_duration() -> i64 {
    60
}

struct CleanMeeting {
    title: String,
    kind: &'static str,
    starts_at: i64,
    duration: i64,
    lang_a: String,
    lang_b: String,
    guests: Vec<String>,
}

fn clean_meeting(req: &MeetingReq, keep_start: Option<i64>) -> Result<CleanMeeting, &'static str> {
    let visible: String = req
        .title
        .chars()
        .filter(|&c| !c.is_control() && !crate::account::invisible(c))
        .collect();
    let title: String = visible.trim().chars().take(MAX_TITLE).collect();
    if title.is_empty() {
        return Err("title");
    }
    if !(5..=24 * 60).contains(&req.duration_min) {
        return Err("duration");
    }
    // 과거로는 예약하지 않는다(지금부터 10분 전까지는 허용: 바로 시작하는 회의를 예약하는 경우).
    // 고칠 때 시각을 그대로 두면(이미 시작한 회의의 이름만 고치는 등) 받는다.
    let unchanged = keep_start == Some(req.starts_at);
    if (!unchanged && req.starts_at < now_secs() - 600) || req.starts_at > now_secs() + 400 * 86_400 {
        return Err("starts_at");
    }
    let lang = |l: &str| {
        if l.is_empty() || room::is_valid_lang(l) {
            Ok(l.to_owned())
        } else {
            Err("lang")
        }
    };
    let mut guests = Vec::new();
    for g in &req.guests {
        let g = crate::auth::normalize_username(g);
        if g.is_empty() {
            continue;
        }
        if !g.contains('@') || !crate::auth::is_valid_username(&g) {
            return Err("guests");
        }
        if !guests.contains(&g) {
            guests.push(g);
        }
    }
    let kind = clean_kind(&req.kind);
    // 대면 대화·자막 화면은 주최자 기기에서만 돌아 손님을 받을 수 없다.
    if guests.len() > MAX_GUESTS || (kind != "meeting" && !guests.is_empty()) {
        return Err("guests");
    }
    Ok(CleanMeeting {
        title,
        kind,
        starts_at: req.starts_at,
        duration: req.duration_min,
        lang_a: lang(&req.lang_a)?,
        lang_b: lang(&req.lang_b)?,
        guests,
    })
}

fn meeting_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let started: Option<i64> = r.get("started_at")?;
    let ended: Option<i64> = r.get("ended_at")?;
    // 끝난 시각이 기록되지 않은 회의(재시작 등)는 마지막 활동 시각으로 길이를 잰다.
    let last_active: Option<i64> = r.get("last_active_at")?;
    Ok(json!({
        "id": r.get::<_, i64>("id")?,
        "code": r.get::<_, String>("code")?,
        "title": r.get::<_, String>("title")?,
        "kind": r.get::<_, String>("kind")?,
        "langA": r.get::<_, String>("lang_a")?,
        "langB": r.get::<_, String>("lang_b")?,
        "startsAt": r.get::<_, Option<i64>>("starts_at")?,
        "durationMin": r.get::<_, i64>("duration_min")?,
        "guests": serde_json::from_str::<Value>(&r.get::<_, String>("guests")?).unwrap_or(json!([])),
        "createdAt": r.get::<_, i64>("created_at")?,
        "startedAt": started,
        "endedAt": ended,
        "lengthSec": match (started, ended.or(last_active)) { (Some(s), Some(e)) => (e - s).max(0), _ => 0 },
        "translatedSec": r.get::<_, i64>("translated_seconds")?,
        "peak": r.get::<_, i64>("peak_participants")?,
    }))
}

const MEETING_COLUMNS: &str = "id, code, title, kind, lang_a, lang_b, starts_at, duration_min, guests, created_at, started_at, ended_at, translated_seconds, peak_participants, last_active_at";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    /// 지난 회의는 50개씩 나눠 받는다(오래된 기록도 지울 수 있게).
    #[serde(default)]
    past_offset: u32,
}

const PAGE: u32 = 50;

/// 다가오는 예약 회의와 지난 회의.
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let now = now_secs();
    let offset = q.past_offset.min(100_000);
    let res = state
        .auth
        .db()
        .call(move |c| {
            // 예약 회의 링크는 끝날 시각 + RESERVED_GRACE까지 쓸 수 있으므로(ensure_room) 그때까지는 다가오는 회의로 둔다.
            let open_until = now - RESERVED_GRACE;
            let mut up = c.prepare(&format!(
                "SELECT {MEETING_COLUMNS} FROM meetings WHERE owner_id = ?1 AND ended_at IS NULL AND starts_at IS NOT NULL
                   AND starts_at + duration_min * 60 > ?2
                 ORDER BY starts_at + duration_min * 60 <= ?3, starts_at LIMIT 50"
            ))?;
            // 아직 끝날 시각이 안 된 회의를 먼저(시간이 지났지만 링크는 아직 쓸 수 있는 회의는 뒤로).
            let upcoming = up.query_map(params![uid, open_until, now], meeting_json)?.collect::<rusqlite::Result<Vec<_>>>()?;
            // 지난 회의: 끝난 회의와, 번역 없이(또는 열리지 않고) 링크를 쓸 수 있는 시간이 지난 회의.
            // 뒤의 것도 보여야 그 기록(이름·초대한 사람 주소)을 지울 수 있다.
            let mut past = c.prepare(&format!(
                "SELECT {MEETING_COLUMNS} FROM meetings WHERE owner_id = ?1 AND (ended_at IS NOT NULL
                   OR (starts_at IS NOT NULL AND starts_at + duration_min * 60 <= ?2))
                 ORDER BY COALESCE(ended_at, starts_at + duration_min * 60) DESC, id DESC LIMIT ?3 OFFSET ?4"
            ))?;
            let mut past = past
                .query_map(params![uid, open_until, PAGE + 1, offset], meeting_json)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let more = past.len() > PAGE as usize;
            past.truncate(PAGE as usize);
            Ok(json!({ "upcoming": upcoming, "past": past, "pastMore": more }))
        })
        .await;
    match res {
        Ok(v) => Json(v).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

/// 회의를 열 수 있는 요금제: 구독 요금제, 없으면 남은 일일권("day"), 둘 다 없으면 "none".
async fn active_plan(db: &Db, uid: i64) -> String {
    db.call(move |c| {
        c.query_row(&format!("SELECT {} FROM users WHERE id = ?1", billing::EFFECTIVE_PLAN_SQL), params![uid], |r| r.get(0))
    })
    .await
    .unwrap_or_else(|_| "none".into())
}

/// 지금 일일권에 남은 시간(끝났으면 0).
async fn day_left(db: &Db, uid: i64) -> i64 {
    db.call(move |c| {
        c.query_row(
            "SELECT CASE WHEN day_until > unixepoch() THEN day_seconds ELSE 0 END FROM users WHERE id = ?1",
            params![uid],
            |r| r.get(0),
        )
    })
    .await
    .unwrap_or(0)
}

/// 구독 요금제(`users.plan`). 결제·요금제 바꾸기는 일일권과 상관없이 이것으로 판단한다.
async fn plan_of(db: &Db, uid: i64) -> String {
    db.call(move |c| {
        c.query_row("SELECT plan FROM users WHERE id = ?1", params![uid], |r| {
            r.get(0)
        })
    })
    .await
    .unwrap_or_else(|_| "none".into())
}

pub async fn schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MeetingReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let db = state.auth.db().clone();
    if active_plan(&db, uid).await == "none" {
        return error(StatusCode::PAYMENT_REQUIRED, "plan");
    }
    let m = match clean_meeting(&req, None) {
        Ok(m) => m,
        Err(field) => return error(StatusCode::BAD_REQUEST, field),
    };
    // 하루에 예약할 수 있는 수를 묶어 둔다(메일 남용 방지).
    let rooms = state.rooms.clone();
    let guests = serde_json::to_string(&m.guests).unwrap_or_else(|_| "[]".into());
    let now = now_secs();
    let res = db
        .call(move |c| {
            let today: i64 = c.query_row(
                "SELECT count(*) FROM meetings WHERE owner_id = ?1 AND starts_at IS NOT NULL AND created_at > ?2",
                params![uid, now - 86_400],
                |r| r.get(0),
            )?;
            if today >= 50 {
                return Ok(None);
            }
            // 메모리의 방·DB의 회의와 모두 겹치지 않는 코드
            let code = loop {
                let code = rooms.fresh_code();
                let taken: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM meetings WHERE code = ?1)", params![code], |r| r.get(0))?;
                if !taken {
                    break code;
                }
            };
            c.execute(
                "INSERT INTO meetings (code, owner_id, title, kind, lang_a, lang_b, starts_at, duration_min, guests, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![code, uid, m.title, m.kind, m.lang_a, m.lang_b, m.starts_at, m.duration, guests, now],
            )?;
            let id = c.last_insert_rowid();
            c.query_row(&format!("SELECT {MEETING_COLUMNS} FROM meetings WHERE id = ?1"), params![id], meeting_json)
                .map(Some)
        })
        .await;
    match res {
        Ok(Some(mut meeting)) => {
            if req.invite {
                meeting["sent"] = json!(send_invites(&state, uid, &meeting).await);
            }
            (StatusCode::CREATED, Json(meeting)).into_response()
        }
        Ok(None) => error(StatusCode::TOO_MANY_REQUESTS, "too_many"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

/// 아직 끝나지 않은 예약 회의(초대 메일·캘린더 파일을 만들 수 있는 회의).
fn upcoming(m: &Value) -> bool {
    m["startsAt"].is_i64() && m["endedAt"].is_null()
}

async fn owned(db: &Db, uid: i64, id: i64) -> Option<Value> {
    db.call(move |c| {
        c.query_row(
            &format!("SELECT {MEETING_COLUMNS} FROM meetings WHERE id = ?1 AND owner_id = ?2"),
            params![id, uid],
            meeting_json,
        )
        .optional()
    })
    .await
    .ok()
    .flatten()
}

pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<MeetingReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    // 요금제가 끝난 계정은 예약 회의를 고치거나 초대 메일을 보내지 못한다(예약과 같게).
    if active_plan(state.auth.db(), uid).await == "none" {
        return error(StatusCode::PAYMENT_REQUIRED, "plan");
    }
    let db = state.auth.db().clone();
    let current = owned(&db, uid, id).await.and_then(|m| m["startsAt"].as_i64());
    let m = match clean_meeting(&req, current) {
        Ok(m) => m,
        Err(field) => return error(StatusCode::BAD_REQUEST, field),
    };
    let guests = serde_json::to_string(&m.guests).unwrap_or_else(|_| "[]".into());
    let n = db
        .call(move |c| {
            c.execute(
                "UPDATE meetings SET title = ?3, kind = ?4, lang_a = ?5, lang_b = ?6, starts_at = ?7, duration_min = ?8, guests = ?9
                 WHERE id = ?1 AND owner_id = ?2 AND ended_at IS NULL",
                params![id, uid, m.title, m.kind, m.lang_a, m.lang_b, m.starts_at, m.duration, guests],
            )
        })
        .await
        .unwrap_or(0);
    if n == 0 {
        return error(StatusCode::NOT_FOUND, "missing");
    }
    let Some(meeting) = owned(&db, uid, id).await else {
        return error(StatusCode::NOT_FOUND, "missing");
    };
    let mut meeting = meeting;
    if req.invite {
        meeting["sent"] = json!(send_invites(&state, uid, &meeting).await);
    }
    Json(meeting).into_response()
}

pub async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let code: Option<String> = state
        .auth
        .db()
        .call(move |c| {
            c.query_row(
                "DELETE FROM meetings WHERE id = ?1 AND owner_id = ?2 RETURNING code",
                params![id, uid],
                |r| r.get(0),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    let Some(code) = code else {
        return error(StatusCode::NOT_FOUND, "missing");
    };
    // 지운 회의의 방이 열려 있으면 닫는다(링크를 받은 사람이 더는 들어오지 못하고, 기록 없는 번역 시간도 쌓이지 않게).
    state.rooms.close(&code);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub struct InviteReq {
    guests: Vec<String>,
}

/// 초대 메일을 (다시) 보낸다. 목록에 없던 주소는 회의 손님 목록에 더한다.
pub async fn invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<InviteReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    // 요금제가 끝난 계정은 예약 회의를 고치거나 초대 메일을 보내지 못한다(예약과 같게).
    if active_plan(state.auth.db(), uid).await == "none" {
        return error(StatusCode::PAYMENT_REQUIRED, "plan");
    }
    let db = state.auth.db().clone();
    let Some(mut meeting) = owned(&db, uid, id).await.filter(upcoming) else {
        return error(StatusCode::NOT_FOUND, "missing");
    };
    // 대면 대화·자막 화면은 주최자 기기에서만 돌아 손님이 들어올 수 없다.
    if meeting["kind"] != "meeting" {
        return error(StatusCode::BAD_REQUEST, "guests");
    }
    let mut guests: Vec<String> = meeting["guests"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for g in &req.guests {
        let g = crate::auth::normalize_username(g);
        if !g.contains('@') || !crate::auth::is_valid_username(&g) {
            return error(StatusCode::BAD_REQUEST, "guests");
        }
        if !guests.contains(&g) {
            guests.push(g);
        }
    }
    if guests.len() > MAX_GUESTS {
        return error(StatusCode::BAD_REQUEST, "guests");
    }
    let json_guests = serde_json::to_string(&guests).unwrap_or_else(|_| "[]".into());
    let _ = db
        .call(move |c| {
            c.execute(
                "UPDATE meetings SET guests = ?2 WHERE id = ?1",
                params![id, json_guests],
            )
        })
        .await;
    meeting["guests"] = json!(guests);
    let mut only: Vec<String> = Vec::new();
    for g in &req.guests {
        let g = crate::auth::normalize_username(g);
        if !only.contains(&g) {
            only.push(g);
        }
    }
    let mut subset = meeting.clone();
    subset["guests"] = json!(only);
    meeting["sent"] = json!(send_invites(&state, uid, &subset).await);
    Json(meeting).into_response()
}

/// 손님마다 초대 메일. 손님의 언어를 모르므로 주최자의 화면 언어로 보낸다.
async fn send_invites(state: &AppState, uid: i64, meeting: &Value) -> Vec<String> {
    let db = state.auth.db().clone();
    let host: Option<(String, String, String)> = db
        .call(move |c| {
            c.query_row(
                "SELECT name, email, locale FROM users WHERE id = ?1",
                params![uid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    let Some((name, email, locale)) = host else {
        return Vec::new();
    };
    let host_name = if name.is_empty() { email } else { name };
    let lang = Lang::parse(&locale);
    let url = format!(
        "{}/{}",
        state.accounts.origin,
        meeting["code"].as_str().unwrap_or("")
    );
    let title = meeting["title"].as_str().unwrap_or("");
    let when = meeting["startsAt"]
        .as_i64()
        .map(|t| format_when(t, meeting["durationMin"].as_i64().unwrap_or(60)))
        .unwrap_or_default();
    let mut sent = Vec::new();
    for g in meeting["guests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
    {
        // 초대 메일로 남에게 스팸을 보내지 못하게 보내는 사람·받는 사람마다 하루 한도를 둔다.
        if !state.accounts.may_invite(uid, g) {
            continue;
        }
        state
            .accounts
            .mailer
            .send(Copy { lang }.invite(g, &host_name, title, &when, &url));
        sent.push(g.to_owned());
    }
    sent
}

/// 메일 속 회의 시각. 받는 사람의 시간대를 모르므로 UTC로 적는다(캘린더 파일이 현지 시각으로 바꿔 준다).
fn format_when(start: i64, minutes: i64) -> String {
    let date = |t: i64| {
        let (y, m) = year_month(t);
        let day = (t - billing::parse_rfc3339(&format!("{y:04}-{m:02}-01T00:00:00Z")).unwrap_or(t)) / 86_400 + 1;
        format!("{y:04}-{m:02}-{day:02}")
    };
    let hm = |t: i64| format!("{:02}:{:02}", t.rem_euclid(86_400) / 3600, t.rem_euclid(3600) / 60);
    let end = start + minutes * 60;
    // 자정을 넘기면 끝나는 날짜도 적는다(24시간 회의가 "23:30 – 23:30"으로 보이지 않게).
    let end_text = if start.div_euclid(86_400) == end.div_euclid(86_400) {
        hm(end)
    } else {
        format!("{} {}", date(end), hm(end))
    };
    format!("{} {} – {end_text} UTC", date(start), hm(start))
}

/// 캘린더 파일(.ics). 주최자가 받아서 자기 캘린더에 넣거나 손님에게 보낸다.
pub async fn ics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let Some(m) = owned(state.auth.db(), uid, id).await.filter(upcoming) else {
        return error(StatusCode::NOT_FOUND, "missing");
    };
    let url = format!(
        "{}/{}",
        state.accounts.origin,
        m["code"].as_str().unwrap_or("")
    );
    let body = ics_text(
        m["code"].as_str().unwrap_or(""),
        m["title"].as_str().unwrap_or(""),
        m["startsAt"].as_i64().unwrap_or(0),
        m["durationMin"].as_i64().unwrap_or(60),
        &url,
    );
    (
        [
            (
                header::CONTENT_TYPE,
                "text/calendar; charset=utf-8".to_owned(),
            ),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"tolkmeet.ics\"".to_owned(),
            ),
        ],
        body,
    )
        .into_response()
}

fn ics_time(t: i64) -> String {
    let (y, m) = year_month(t);
    let month_start = billing::parse_rfc3339(&format!("{y:04}-{m:02}-01T00:00:00Z")).unwrap_or(t);
    let day = (t - month_start) / 86_400 + 1;
    let s = t.rem_euclid(86_400);
    format!(
        "{y:04}{m:02}{day:02}T{:02}{:02}{:02}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

fn ics_escape(s: &str) -> String {
    let s: String = s.chars().filter(|&c| !crate::account::invisible(c)).collect();
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
        .replace('\r', "")
}

pub fn ics_text(code: &str, title: &str, start: i64, minutes: i64, url: &str) -> String {
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Tolkmeet//EN\r\nCALSCALE:GREGORIAN\r\nMETHOD:PUBLISH\r\nBEGIN:VEVENT\r\nUID:{code}@tolkmeet.com\r\nDTSTAMP:{}\r\nDTSTART:{}\r\nDTEND:{}\r\nSUMMARY:{}\r\nDESCRIPTION:{}\r\nURL:{url}\r\nLOCATION:{url}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        ics_time(now_secs()),
        ics_time(start),
        ics_time(start + minutes * 60),
        ics_escape(title),
        ics_escape(&format!("Tolkmeet: {url}")),
    );
    text.split_inclusive("\r\n").map(ics_fold).collect()
}

/// RFC 5545: 한 줄은 75바이트를 넘기지 않는다. 넘으면 글자 경계에서 CRLF + 공백으로 접는다.
fn ics_fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut width = 0;
    for c in line.chars() {
        if c != '\r' && c != '\n' && width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        width += c.len_utf8();
        out.push(c);
    }
    out
}

// ── 요금제·결제 ───────────────────────────────────────

/// 이번 결제 기간의 번역 시간과 요금제, 최근 6달 사용 기록.
pub async fn billing_summary(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let now = now_secs();
    let res = state
        .auth
        .db()
        .call(move |c| {
            let u = usage_of(c, uid, now)?;
            let (renews, canceled, customer): (Option<i64>, bool, Option<String>) = c.query_row(
                "SELECT plan_renews_at, plan_canceled, paddle_customer_id FROM users WHERE id = ?1",
                params![uid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            // 최근 6달 사용량(이번 달 포함, 오래된 달부터)
            let mut history = Vec::new();
            let (mut y, mut m) = year_month(now);
            let mut months = Vec::new();
            for _ in 0..6 {
                months.push(format!("{y:04}-{m:02}"));
                if m == 1 {
                    y -= 1;
                    m = 12;
                } else {
                    m -= 1;
                }
            }
            for p in months.into_iter().rev() {
                let secs: i64 = c.query_row(
                    "SELECT COALESCE(SUM(seconds), 0) FROM usage WHERE user_id = ?1 AND period = ?2",
                    params![uid, p],
                    |r| r.get(0),
                )?;
                history.push(json!({ "period": p, "seconds": secs }));
            }
            Ok(u.map(|u| {
                json!({
                    "history": history,
                    "plan": u.shown_plan(now),
                    // 이번 결제 기간: 포함된 시간(올린 요금제의 차이 포함), 그중 쓴 시간(추가 시간으로 낸 몫 제외), 기간이 끝나 다시 채워지는 때
                    "includedSec": u.included,
                    "usedSec": u.used,
                    "extraSec": u.extra,
                    // 남은 일일권 시간과 끝나는 때(일일권이 없거나 끝났으면 0·null)
                    "daySec": u.day,
                    "dayUntil": u.day_until.filter(|t| *t > now),
                    "remainingSec": u.remaining(),
                    "periodStart": u.start,
                    // 이번 기간의 결제가 아직 끝나지 않았다(첫 결제 직후 잠깐, 또는 갱신 결제가 밀림): 포함 시간은 결제가 끝나면 생긴다.
                    "periodPaid": u.paid,
                    // 갱신 결제가 밀렸다(결제 수단을 고치라고 안내할 때). 첫 결제 직후에는 false.
                    "pastDue": u.past_due,
                    "resetsAt": u.end.unwrap_or_else(|| next_reset(now)),
                    "renewsAt": renews,
                    "canceled": canceled,
                    "hasCustomer": customer.is_some(),
                })
            }))
        })
        .await;
    match res {
        Ok(Some(v)) => Json(v).into_response(),
        _ => error(StatusCode::NOT_FOUND, "missing"),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutReq {
    /// "basic" | "pro" | "hours" | "day"
    item: String,
    #[serde(default = "one")]
    quantity: i64,
    /// 결제 화면에서 Paddle 제공·국외 이전에 각각 동의했는지
    #[serde(default)]
    consent_provide: bool,
    #[serde(default)]
    consent_transfer: bool,
}

fn one() -> i64 {
    1
}

/// Paddle 제공·국외 이전 동의 시각을 남긴다(개인정보 보호법 제17조·제28조의8 동의의 기록).
async fn record_consent(db: &Db, uid: i64) -> bool {
    let now = now_secs();
    // 새로 동의하면 예전 철회 기록은 지운다.
    db.call(move |c| {
        c.execute(
            "UPDATE users SET paddle_consent_at = ?2, paddle_consent_withdrawn_at = NULL WHERE id = ?1",
            params![uid, now],
        )
    })
        .await
        .is_ok()
}

/// 결제창(Paddle.js)에 넘길 값. 금액은 Paddle에 등록된 가격 id로만 정해지므로 브라우저가 바꿀 수 없다.
/// 결제창은 서버가 만든 거래(`transactionId`)로 연다. 구독 결제는 웹훅이 오기 전에 다시 눌러도 같은 거래를 다시 쓰므로
/// (한 거래는 한 번만 결제된다) 구독이 두 개 생기지 않는다.
/// 결제 키가 없는 개발 모드에서는 `dev: true`를 돌려주고, 화면은 결제 없이 요금제를 켜는 경로를 쓴다.
pub async fn checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CheckoutReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if !matches!(req.item.as_str(), "basic" | "pro" | "hours" | "day") || !(1..=100).contains(&req.quantity)
    {
        return error(StatusCode::BAD_REQUEST, "item");
    }
    // 결제 정보는 Paddle(영국)에 제공·국외 이전되므로 두 가지 동의가 모두 있어야 한다.
    if !(req.consent_provide && req.consent_transfer) {
        return error(StatusCode::BAD_REQUEST, "consent");
    }
    let db = state.auth.db().clone();
    let plan = plan_of(&db, uid).await;
    // 추가 시간은 요금제가 있을 때만 쓸 수 있으므로(회의를 열 수 없다) 요금제 없이는 팔지 않는다.
    if req.item == "hours" && plan == "none" {
        return error(StatusCode::PAYMENT_REQUIRED, "plan");
    }
    let Some(b) = &state.billing else {
        return if state.dev_mode {
            if !record_consent(&db, uid).await {
                return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
            }
            Json(json!({ "dev": true })).into_response()
        } else {
            error(StatusCode::SERVICE_UNAVAILABLE, "billing")
        };
    };
    // 이미 구독 중이면 새 구독을 만들지 않는다(두 번 결제되지 않게). 요금제 바꾸기·다시 이어 쓰기로 처리한다.
    // 일일권도 구독 중에는 팔지 않는다(포함 시간이 있으면 필요 없다. 더 필요하면 추가 시간을 산다).
    if req.item != "hours" && plan != "none" {
        return error(StatusCode::CONFLICT, "subscribed");
    }
    // 일일권은 지금 일일권의 시간을 다 쓴 뒤에만 판다(남은 시간을 합치면 환불이 어느 일일권의 시간인지 가를 수 없다).
    if req.item == "day" && day_left(&db, uid).await > 0 {
        return error(StatusCode::CONFLICT, "day_active");
    }
    // 실제로 결제창을 열 때만 동의 시각을 남긴다.
    if !record_consent(&db, uid).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    }
    let (price, quantity) = match req.item.as_str() {
        "hours" => (b.price_hour.as_str(), req.quantity),
        "day" => (b.price_day.as_str(), 1),
        _ => (b.price_for(&req.item).unwrap_or_default(), 1),
    };
    // 한 번 결제(추가 시간·일일권)는 결제할 때마다 새 거래다. 구독만 기다리는 거래를 다시 쓴다(구독이 둘 생기지 않게).
    let txn = if matches!(req.item.as_str(), "hours" | "day") {
        match b.create_transaction(price, quantity, uid).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "could not create the checkout transaction");
                return error(StatusCode::BAD_GATEWAY, "billing");
            }
        }
    } else {
        match subscription_transaction(b, &db, uid, price).await {
            Ok(t) => t,
            Err(r) => return r,
        }
    };
    let email: String = db
        .call(move |c| {
            c.query_row("SELECT email FROM users WHERE id = ?1", params![uid], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap_or_default();
    Json(json!({
        "clientToken": b.client_token,
        "sandbox": b.sandbox,
        "transactionId": txn,
        "priceId": price,
        "quantity": quantity,
        "email": email,
        "customData": { "user_id": uid.to_string(), "sig": b.sign_user(uid) },
    }))
    .into_response()
}

/// 아직 결제하지 않은 구독 결제 거래를 이만큼 다시 쓴다. 그보다 오래됐으면 취소하고 새로 만든다.
/// 거래는 나이와 상관없이 기억하고 매번 상태를 본다(이미 결제됐으면 웹훅이 올 때까지 새 결제를 받지 않는다).
const PENDING_CHECKOUT: i64 = 30 * 60;
/// 다른 요청이 거래를 만드는 중인 표시(빈 값)를 믿는 시간. Paddle 요청 시간 제한(20초)보다 길게.
const CREATING_CHECKOUT: i64 = 60;

enum Claim {
    /// 기다리는 거래가 있다(거래, 만든 지 지난 초).
    Pending(String, i64),
    /// 이 요청이 새 거래를 만든다(다른 요청은 기다린다).
    Create,
    /// 다른 요청이 지금 거래를 만드는 중이다.
    Busy,
}

/// 구독 결제 거래: 기다리는 거래가 아직 결제 전이면 그것을(요금제가 바뀌었으면 바꿔서, 오래됐으면 취소하고 새로),
/// 이미 결제됐으면 409 "pending", 없거나 쓸 수 없으면 새로 만든다. 확인과 표시는 DB 호출 하나 안에서 해서
/// 동시에 눌러도 거래를 둘 만들지 않는다.
async fn subscription_transaction(b: &billing::Billing, db: &Db, uid: i64, price: &str) -> Result<String, Response> {
    let now = now_secs();
    let failed = |_| error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
    let claim = db
        .call(move |c| {
            let (txn, at): (Option<String>, Option<i64>) = c.query_row(
                "SELECT paddle_pending_txn, paddle_pending_at FROM users WHERE id = ?1",
                params![uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            match (txn, at) {
                (Some(t), Some(a)) if t.is_empty() && now - a < CREATING_CHECKOUT => return Ok(Claim::Busy),
                (Some(t), Some(a)) if !t.is_empty() => return Ok(Claim::Pending(t, now - a)),
                _ => {}
            }
            c.execute(
                "UPDATE users SET paddle_pending_txn = '', paddle_pending_at = ?2 WHERE id = ?1",
                params![uid, now],
            )?;
            Ok(Claim::Create)
        })
        .await
        .map_err(failed)?;
    match claim {
        Claim::Busy => return Err(error(StatusCode::CONFLICT, "pending")),
        Claim::Pending(t, age) => {
            match b.transaction_status(&t).await {
                Ok((status, _)) if age >= PENDING_CHECKOUT && billing::pending_checkout(&status) == billing::PendingCheckout::Reuse => {
                    // 오래된 결제창도 아직 결제할 수 있다. 새 거래를 만들기 전에 취소해 두 곳에서 결제되지 않게 한다.
                    match b.cancel_transaction(&t).await {
                        Ok(()) => {}
                        // 그사이 결제가 시작됐거나 끝났다.
                        Err(e) if e.code.starts_with("transaction_immutable") => {
                            return Err(error(StatusCode::CONFLICT, "pending"));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "could not cancel the old checkout");
                            return Err(error(StatusCode::BAD_GATEWAY, "billing"));
                        }
                    }
                }
                Ok((status, current)) => match billing::pending_checkout(&status) {
                    billing::PendingCheckout::Reuse => {
                        if current.as_deref() != Some(price)
                            && let Err(e) = b.set_transaction_price(&t, price).await
                        {
                            tracing::warn!(error = %e, "could not change the pending checkout");
                            return Err(error(StatusCode::CONFLICT, "pending"));
                        }
                        let keep = t.clone();
                        let _ = db
                            .call(move |c| {
                                c.execute(
                                    "UPDATE users SET paddle_pending_at = ?3 WHERE id = ?1 AND paddle_pending_txn = ?2",
                                    params![uid, keep, now],
                                )
                            })
                            .await;
                        return Ok(t);
                    }
                    billing::PendingCheckout::Paid => return Err(error(StatusCode::CONFLICT, "pending")),
                    billing::PendingCheckout::Gone => {}
                },
                Err(e) if e.code == "not_found" => {}
                Err(e) => {
                    tracing::warn!(error = %e, "could not read the pending checkout");
                    return Err(error(StatusCode::BAD_GATEWAY, "billing"));
                }
            }
            // 쓸 수 없는 거래: 바꿔 넣는 것도 한 요청만 한다.
            let old = t.clone();
            let took = db
                .call(move |c| {
                    c.execute(
                        "UPDATE users SET paddle_pending_txn = '', paddle_pending_at = ?3 WHERE id = ?1 AND paddle_pending_txn = ?2",
                        params![uid, old, now],
                    )
                })
                .await
                .map_err(failed)?;
            if took == 0 {
                return Err(error(StatusCode::CONFLICT, "pending"));
            }
        }
        Claim::Create => {}
    }
    let created = b.create_transaction(price, 1, uid).await;
    let keep = created.as_ref().ok().cloned();
    // 만들지 못했으면 표시를 지워 바로 다시 시도할 수 있게 한다.
    let _ = db
        .call(move |c| {
            c.execute(
                "UPDATE users SET paddle_pending_txn = ?2, paddle_pending_at = CASE WHEN ?2 IS NULL THEN NULL ELSE ?3 END
                 WHERE id = ?1 AND paddle_pending_txn = ''",
                params![uid, keep, now],
            )
        })
        .await;
    created.map_err(|e| {
        tracing::warn!(error = %e, "could not create the checkout transaction");
        error(StatusCode::BAD_GATEWAY, "billing")
    })
}

/// 개발 모드 전용: 결제 없이 요금제를 켜거나 시간을 더한다(결제 흐름 화면 확인용). 결제 기간은 지금부터 30일.
pub async fn dev_activate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CheckoutReq>,
) -> Response {
    if !state.dev_mode {
        return StatusCode::NOT_FOUND.into_response();
    }
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let plan = plan_of(state.auth.db(), uid).await;
    if req.item == "hours" && plan == "none" {
        return error(StatusCode::PAYMENT_REQUIRED, "plan");
    }
    if req.item == "day" && plan != "none" {
        return error(StatusCode::CONFLICT, "subscribed");
    }
    if req.item == "day" && day_left(state.auth.db(), uid).await > 0 {
        return error(StatusCode::CONFLICT, "day_active");
    }
    let now = now_secs();
    let renews = now + billing::DEFAULT_PERIOD;
    let item = req.item.clone();
    let res = state
        .auth
        .db()
        .call(move |c| match req.item.as_str() {
            "basic" | "pro" => c.execute(
                "UPDATE users SET plan = ?2, plan_renews_at = ?3, plan_canceled = 0,
                   period_start = ?4, period_end = ?3, period_included = ?5 WHERE id = ?1",
                params![uid, req.item, renews, now, included_seconds(&req.item)],
            ),
            // 웹훅의 일일권 반영(billing `Grant::Day`)과 같다.
            "day" => c.execute(
                "UPDATE users SET day_seconds = CASE WHEN day_until > ?3 AND day_seconds > 0 THEN day_seconds ELSE 0 END + ?2,
                   day_since = CASE WHEN day_until > ?3 AND day_seconds > 0 THEN MIN(COALESCE(day_since, ?3), ?3) ELSE ?3 END,
                   day_until = MAX(COALESCE(day_until, 0), ?3 + ?4) WHERE id = ?1",
                params![uid, billing::DAY_PASS_SECONDS, now, billing::DAY_PASS_LENGTH],
            ),
            _ => c.execute(
                "UPDATE users SET extra_seconds = extra_seconds + ?2 WHERE id = ?1",
                params![uid, req.quantity.clamp(1, 100) * 3600],
            ),
        })
        .await;
    match res {
        Ok(_) => {
            // 시간이 떨어져 막혀 있던 회의가 바로 풀리게 한다(실제 결제의 웹훅처럼).
            refresh_quota(&state).await;
            // 실제 결제처럼 영수증 메일도 같은 경로로 보낸다(개발 모드에서는 data/mail에 파일로 남는다).
            if matches!(item.as_str(), "basic" | "pro") {
                // 가격은 web의 PLAN_PRICE(AccountPages)·PRICE(BillingPages)와 같다. 바꿀 때 함께 바꾼다.
                let amount = if item == "pro" { "$49.00" } else { "$19.00" };
                send_receipt(
                    &state,
                    billing::Receipt { user: uid, plan: item.clone(), amount: amount.into(), renews: Some(renews) },
                )
                .await;
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

async fn subscription_of(db: &Db, uid: i64) -> (Option<String>, Option<String>) {
    db.call(move |c| {
        c.query_row(
            "SELECT paddle_subscription_id, paddle_customer_id FROM users WHERE id = ?1",
            params![uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    })
    .await
    .unwrap_or((None, None))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReq {
    #[serde(default)]
    plan: String,
    /// 동의를 철회한 사람이 요금제를 바꾸며 다시 동의할 때(결제 화면의 두 체크박스)
    #[serde(default)]
    consent_provide: bool,
    #[serde(default)]
    consent_transfer: bool,
}

/// cancel: 이번 기간이 끝나면 해지 · resume: 예약된 해지 취소 · change: 요금제 바꾸기
pub async fn manage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(action): Path<String>,
    Json(req): Json<PlanReq>,
) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let db = state.auth.db().clone();
    // Paddle 동의를 철회했으면 다시 동의하기 전까지 요금제를 바꾸지 않는다(해지·이어 쓰기는 된다).
    // 결제 화면이 두 동의를 함께 보내면 그 자리에서 다시 동의한 것으로 기록하고 진행한다.
    if action == "change" {
        let withdrawn = db
            .call(move |c| {
                c.query_row(
                    "SELECT paddle_consent_withdrawn_at IS NOT NULL FROM users WHERE id = ?1",
                    params![uid],
                    |r| r.get::<_, bool>(0),
                )
            })
            .await;
        match withdrawn {
            Ok(false) => {}
            Ok(true) if req.consent_provide && req.consent_transfer => {
                if !record_consent(&db, uid).await {
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "failed");
                }
            }
            Ok(true) => return error(StatusCode::FORBIDDEN, "consent"),
            Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
        }
    }
    let Some(b) = &state.billing else {
        // 개발 모드: Paddle 없이 같은 결과를 흉내 낸다.
        if !state.dev_mode {
            return error(StatusCode::SERVICE_UNAVAILABLE, "billing");
        }
        let plan = req.plan.clone();
        let res = db
            .call(move |c| match action.as_str() {
                "cancel" => c.execute(
                    "UPDATE users SET plan_canceled = 1 WHERE id = ?1",
                    params![uid],
                ),
                "resume" => c.execute(
                    "UPDATE users SET plan_canceled = 0 WHERE id = ?1",
                    params![uid],
                ),
                // 실제 결제와 같게: 올리면 이번 기간에 남은 비율만큼 포함 시간을 더하고, 내리면 그대로 둔다.
                "change" if matches!(plan.as_str(), "basic" | "pro") => {
                    let (old, start, end, included): (String, Option<i64>, Option<i64>, i64) = c.query_row(
                        "SELECT plan, period_start, period_end, period_included FROM users WHERE id = ?1",
                        params![uid],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )?;
                    let included = match (start, end) {
                        (Some(s), Some(e)) if old != "none" => {
                            billing::included_after_change(included, &old, &plan, (s, e), now_secs())
                        }
                        _ => included_seconds(&plan),
                    };
                    c.execute(
                        "UPDATE users SET plan = ?2, period_included = ?3 WHERE id = ?1",
                        params![uid, plan, included],
                    )
                }
                _ => Ok(0),
            })
            .await;
        return match res {
            Ok(1) => {
                refresh_quota(&state).await;
                StatusCode::NO_CONTENT.into_response()
            }
            _ => error(StatusCode::BAD_REQUEST, "action"),
        };
    };
    let (Some(sub), _) = subscription_of(&db, uid).await else {
        return error(StatusCode::CONFLICT, "no_subscription");
    };
    let result = match action.as_str() {
        "cancel" => b.cancel_at_period_end(&sub).await,
        "resume" => b.resume(&sub).await,
        "change" if matches!(req.plan.as_str(), "basic" | "pro") => {
            let current = plan_of(&db, uid).await;
            if current == req.plan {
                return StatusCode::NO_CONTENT.into_response();
            }
            let upgrade = included_seconds(&req.plan) > included_seconds(&current);
            b.change_plan(&sub, &req.plan, upgrade).await
        }
        _ => return error(StatusCode::BAD_REQUEST, "action"),
    };
    // 실제 반영은 Paddle 웹훅으로 온다. 화면이 바로 바뀌도록 해지 예약 표시만 먼저 맞춘다.
    match result {
        Ok(()) => {
            let flag = match action.as_str() {
                "cancel" => Some(1),
                "resume" => Some(0),
                _ => None,
            };
            if let Some(f) = flag {
                let _ = db
                    .call(move |c| {
                        c.execute(
                            "UPDATE users SET plan_canceled = ?2 WHERE id = ?1",
                            params![uid, f],
                        )
                    })
                    .await;
            }
            StatusCode::NO_CONTENT.into_response()
        }
        // 결제가 밀린 구독은 Paddle이 바꾸지 못하게 한다. 화면이 결제 수단을 고치라고 안내하게 따로 알린다.
        Err(e) if e.code == "subscription_update_when_past_due" => error(StatusCode::CONFLICT, "past_due"),
        Err(e) => {
            tracing::warn!(error = %e, "paddle request failed");
            error(StatusCode::BAD_GATEWAY, "billing")
        }
    }
}

/// 결제 수단·영수증을 보는 Paddle 고객 포털.
pub async fn portal(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let Some(b) = &state.billing else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "billing");
    };
    let (_, Some(customer)) = subscription_of(state.auth.db(), uid).await else {
        return error(StatusCode::CONFLICT, "no_customer");
    };
    match b.portal_url(&customer).await {
        Ok(url) => Json(json!({ "url": url })).into_response(),
        Err(_) => error(StatusCode::BAD_GATEWAY, "billing"),
    }
}

/// Paddle 웹훅. 서명이 맞는 것만 반영하고, 같은 이벤트는 한 번만 반영한다.
pub async fn paddle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(b) = state.billing.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let sig = headers
        .get("paddle-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !b.verify_signature(sig, &body, now_secs()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(event) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match b.apply_event(state.auth.db(), event).await {
        // 아직 반영할 수 없는 이벤트(환불된 결제가 아직 기록되지 않음): Paddle이 나중에 다시 보낸다.
        Ok(applied) if applied.retry => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Ok(applied) => {
            if let Some(r) = applied.receipt {
                send_receipt(&state, r).await;
            }
            // 새 구독에 밀린 예전 구독·환불된 구독의 해지는 DB에 남겨 두었다. Paddle의 응답 시간 제한(5초)을 넘기지 않게
            // 웹훅에 먼저 답하고 따로 해지하며, 실패하면 주기 작업이 확인될 때까지 다시 한다.
            for sub in applied.replaced.iter().chain(&applied.revoked) {
                tracing::info!(subscription = %sub, "subscription queued for cancellation");
            }
            // 추가 시간을 샀으면 막혀 있던 회의가 바로 풀리도록 한도를 다시 본다(해지 대기열도 이때 돈다).
            refresh_quota(&state).await;
            StatusCode::OK.into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "paddle webhook failed");
            // 5xx면 Paddle이 다시 보낸다.
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn send_receipt(state: &AppState, r: billing::Receipt) {
    let uid = r.user;
    let info: Option<(String, String)> = state
        .auth
        .db()
        .call(move |c| {
            c.query_row(
                "SELECT email, locale FROM users WHERE id = ?1",
                params![uid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    let Some((email, locale)) = info else { return };
    let plan = if r.plan == "pro" { "Pro" } else { "Basic" };
    let renews = r
        .renews
        .map(|t| format_when(t, 0).split(' ').next().unwrap_or("").to_owned())
        .unwrap_or_default();
    state.accounts.mailer.send(
        Copy {
            lang: Lang::parse(&locale),
        }
        .receipt(&email, plan, &r.amount, &renews),
    );
}

/// 지금 막혀 있는 주최자들의 남은 시간을 다시 계산한다(결제 직후).
pub async fn refresh_quota(state: &AppState) {
    tick(state, &[]).await;
}

/// 주기 작업: 잰 번역 시간을 기록하고, 시간이 떨어진 주최자의 회의를 막는다.
pub async fn tick(state: &AppState, metered: &[Metered]) {
    // 해지를 기다리는 구독은 따로 돌린다(Paddle 왕복을 기다리지 않게, 이미 돌고 있으면 건너뛴다).
    if let Some(b) = state.billing.clone() {
        let db = state.auth.db().clone();
        tokio::spawn(async move { b.process_cancellations(&db).await });
    }
    // 주기 작업과 웹훅의 확인이 겹치면 늦게 끝난 쪽이 옛 목록으로 덮어쓸 수 있어 한 번에 하나씩 돈다.
    // 회의 요청 경로가 아니라(주기 작업·결제 웹훅) 기다려도 회의는 막히지 않는다.
    static TICK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one_at_a_time = TICK.lock().await;
    // 이미 막힌 주최자와 열려 있는 방의 주최자를 함께 본다(재시작 직후나 방금 연 방도 다음 주기 안에 막히게).
    let mut check = state.rooms.out_of_time_hosts();
    check.extend(state.rooms.open_hosts());
    let pending: Vec<Metered> = metered.to_vec();
    let now = now_secs();
    match state
        .auth
        .db()
        .call(move |c| record_usage(c, &pending, &check, now))
        .await
    {
        Ok(out) => state.rooms.set_out_of_time(out),
        Err(e) => {
            // 잰 시간을 잃지 않게 되돌려 두고 다음 주기에 다시 기록한다.
            tracing::warn!(error = %e, "usage not recorded");
            state.rooms.restore_metered(metered);
        }
    }
}

pub fn spawn_end(state: &AppState, ended: Vec<String>) {
    if ended.is_empty() {
        return;
    }
    let db = state.auth.db().clone();
    tokio::spawn(async move {
        let now = now_secs();
        let _ = db.call(move |c| end_rooms(c, &ended, now)).await;
    });
}

#[allow(dead_code)]
fn _assert_send(_: Arc<AppState>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_request_reads_web_field_names() {
        // 웹은 camelCase로 보낸다. 이름이 어긋나면 예약 저장이 422로 실패한다.
        let req: MeetingReq = serde_json::from_value(json!({
            "title": "t", "kind": "meeting", "startsAt": 1_800_000_000i64, "durationMin": 30,
            "langA": "ko", "langB": "ja", "guests": ["a@example.com"], "invite": true
        }))
        .unwrap();
        assert_eq!(
            (req.starts_at, req.duration_min, req.lang_b.as_str()),
            (1_800_000_000, 30, "ja")
        );
    }

    #[test]
    fn calendar_month_math() {
        assert_eq!(year_month(0), (1970, 1));
        assert_eq!(year_month(1_794_909_600), (2026, 11));
        assert_eq!(
            next_reset(1_794_909_600),
            billing::parse_rfc3339("2026-12-01T00:00:00Z").unwrap()
        );
        assert_eq!(
            next_reset(billing::parse_rfc3339("2026-12-31T23:59:59Z").unwrap()),
            billing::parse_rfc3339("2027-01-01T00:00:00Z").unwrap()
        );
        assert_eq!(
            ics_time(billing::parse_rfc3339("2026-11-17T09:05:00Z").unwrap()),
            "20261117T090500Z"
        );
    }

    #[test]
    fn usage_spills_into_bought_hours_and_blocks_when_empty() {
        let db = Db::memory();
        let uid = db
            .blocking(|c| {
                c.execute("INSERT INTO users (email, plan, extra_seconds, created_at) VALUES ('a@b.co', 'basic', 100, 1)", [])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        let host = uid.to_string();
        let now = 1_794_909_600;
        let m = |s| {
            vec![Metered {
                host: host.clone(),
                code: "abcd-efgh-ijkm".into(),
                seconds: s,
                peak: 2,
                langs: 2,
            }]
        };
        // 포함된 시간에서 1분 덜 쓰기 → 아직 남음
        let out = db
            .blocking(|c| record_usage(c, &m(billing::included_seconds("basic") - 60), &HashSet::new(), now))
            .unwrap();
        assert!(out.is_empty());
        // 2분 더 → 포함 시간 60초 넘김 → 추가 시간 100초에서 60초 차감
        db.blocking(|c| record_usage(c, &m(120), &HashSet::new(), now))
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
        assert_eq!(extra, 40);
        // 다 쓰면 막힌다
        let out = db
            .blocking(|c| record_usage(c, &m(60), &HashSet::new(), now))
            .unwrap();
        assert!(out.contains(&host));
        // 다음 달에는 다시 채워진다
        let out = db
            .blocking(|c| record_usage(c, &[], &HashSet::from([host.clone()]), now + 40 * 86_400))
            .unwrap();
        assert!(out.is_empty());
    }

    /// 결제 기간 [start, start + 30일)인 사용자
    fn subscriber(db: &Db, plan: &str, extra: i64, start: i64) -> i64 {
        let plan = plan.to_owned();
        db.blocking(move |c| {
            c.execute(
                "INSERT INTO users (email, plan, extra_seconds, period_start, period_end, period_included, created_at)
                 VALUES ('a@b.co', ?1, ?2, ?3, ?4, ?5, 1)",
                params![plan, extra, start, start + billing::DEFAULT_PERIOD, included_seconds(&plan)],
            )?;
            Ok(c.last_insert_rowid())
        })
        .unwrap()
    }

    fn metered(uid: i64, seconds: i64) -> Vec<Metered> {
        vec![Metered { host: uid.to_string(), code: "abcd-efgh-ijkm".into(), seconds, peak: 1, langs: 1 }]
    }

    #[test]
    fn bought_hours_are_not_counted_twice_when_included_hours_grow() {
        let db = Db::memory();
        let start = 1_794_909_600;
        let uid = subscriber(&db, "basic", 2 * 3600, start);
        let (basic, pro) = (billing::included_seconds("basic"), billing::included_seconds("pro"));
        // 포함 10시간 + 산 시간에서 1시간
        db.blocking(|c| record_usage(c, &metered(uid, basic + 3600), &HashSet::new(), start + 3600)).unwrap();
        let u = db.blocking(|c| usage_of(c, uid, start + 3600)).unwrap().unwrap();
        assert_eq!((u.used, u.extra, u.remaining()), (basic, 3600, 3600));
        // 기간 한가운데에서 Pro로 올리면 포함 시간이 20시간이 된다. 산 시간으로 낸 1시간을 포함 시간에서 또 빼지 않는다.
        let half = start + billing::DEFAULT_PERIOD / 2;
        let included = billing::included_after_change(basic, "basic", "pro", (start, start + billing::DEFAULT_PERIOD), half);
        db.blocking(|c| c.execute("UPDATE users SET plan = 'pro', period_included = ?2 WHERE id = ?1", params![uid, included]))
            .unwrap();
        let u = db.blocking(|c| usage_of(c, uid, half)).unwrap().unwrap();
        let half_up = basic + (pro - basic) / 2;
        assert_eq!((u.included, u.remaining()), (half_up, half_up - basic + 3600));
    }

    #[test]
    fn paddle_subscriptions_and_canceled_plans_do_not_refill_by_themselves() {
        // Paddle 구독은 다음 기간의 결제가 끝나야 시간을 받는다(결제가 밀려도 기간 끝에 저절로 채우지 않는다).
        let db = Db::memory();
        let start = 1_794_909_600;
        let end = start + billing::DEFAULT_PERIOD;
        let uid = subscriber(&db, "pro", 0, start);
        db.blocking(|c| record_usage(c, &metered(uid, billing::included_seconds("pro")), &HashSet::new(), start + 60)).unwrap();
        for (column, value) in [("paddle_subscription_id", "'sub_1'"), ("plan_canceled", "1")] {
            db.blocking(|c| {
                c.execute_batch(&format!(
                    "UPDATE users SET paddle_subscription_id = NULL, plan_canceled = 0; UPDATE users SET {column} = {value};"
                ))
            })
            .unwrap();
            let u = db.blocking(|c| usage_of(c, uid, end + 1)).unwrap().unwrap();
            assert_eq!((u.start, u.remaining()), (Some(start), 0), "{column}");
        }
    }

    #[test]
    fn past_due_only_when_a_renewal_stays_unpaid() {
        let db = Db::memory();
        let start = 1_794_909_600;
        let uid = subscriber(&db, "pro", 0, start);
        let state = |paid: Option<i64>, status: &'static str| {
            db.blocking(move |c| {
                c.execute(
                    "UPDATE users SET paddle_subscription_id = 'sub_1', period_paid_start = ?2, paddle_status = ?3 WHERE id = ?1",
                    params![uid, paid, status],
                )?;
                usage_of(c, uid, start + 3600).map(|u| u.map(|u| (u.paid, u.past_due)))
            })
            .unwrap()
            .unwrap()
        };
        assert_eq!(state(Some(start), "active"), (true, false), "paid");
        assert_eq!(state(None, "active"), (false, false), "first purchase, payment webhook still on its way");
        assert_eq!(state(Some(start - billing::DEFAULT_PERIOD), "active"), (false, false), "renewal webhook still on its way");
        assert_eq!(state(Some(start - billing::DEFAULT_PERIOD), "past_due"), (false, true), "renewal payment failed");
    }

    #[test]
    fn day_pass_hours_are_spent_before_bought_hours_and_expire() {
        let db = Db::memory();
        let now = now_secs();
        let uid = db
            .blocking(move |c| {
                c.execute(
                    "INSERT INTO users (email, extra_seconds, day_seconds, day_until, created_at) VALUES ('a@b.co', 3600, ?1, ?2, 1)",
                    params![billing::DAY_PASS_SECONDS, now + 600],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        fn db_plan(db: &Db, uid: i64) -> String {
            db.blocking(move |c| {
                c.query_row(&format!("SELECT {} FROM users WHERE id = ?1", billing::EFFECTIVE_PLAN_SQL), params![uid], |r| r.get(0))
            })
            .unwrap()
        }
        assert_eq!(db_plan(&db, uid), "day");
        // 일일권 3시간 + 30분: 일일권을 먼저 다 쓰고 나머지를 산 시간에서 뺀다.
        let out = db.blocking(move |c| record_usage(c, &metered(uid, billing::DAY_PASS_SECONDS + 1800), &HashSet::new(), now)).unwrap();
        assert!(out.is_empty());
        let u = db.blocking(move |c| usage_of(c, uid, now)).unwrap().unwrap();
        assert_eq!((u.day, u.extra, u.used, u.remaining(), u.shown_plan(now)), (0, 1800, 0, 1800, "day"));
        // 24시간이 지나면 남은 일일권은 세지 않고, 회의를 열 수 있는 요금제도 없어진다.
        db.blocking(move |c| c.execute("UPDATE users SET day_seconds = 3600, day_until = ?2 WHERE id = ?1", params![uid, now - 1]))
            .unwrap();
        let u = db.blocking(move |c| usage_of(c, uid, now)).unwrap().unwrap();
        assert_eq!((u.day, u.remaining(), u.shown_plan(now)), (0, 1800, "none"));
        assert_eq!(db_plan(&db, uid), "none");
    }

    #[test]
    fn plans_without_payment_refill_when_the_period_ends() {
        let db = Db::memory();
        let start = 1_794_909_600;
        let uid = subscriber(&db, "pro", 0, start);
        let host = uid.to_string();
        let out = db.blocking(|c| record_usage(c, &metered(uid, billing::included_seconds("pro")), &HashSet::new(), start + 86_400)).unwrap();
        assert!(out.contains(&host), "used up");
        // 결제 없이 켠 요금제(운영자·개발 모드)는 기간이 끝나면 같은 길이로 넘겨 다시 채운다.
        let end = start + billing::DEFAULT_PERIOD;
        let out = db.blocking(|c| record_usage(c, &[], &HashSet::from([host.clone()]), end + 1)).unwrap();
        assert!(out.is_empty());
        let u = db.blocking(|c| usage_of(c, uid, end + 1)).unwrap().unwrap();
        assert_eq!((u.start, u.end, u.included, u.used), (Some(end), Some(end + billing::DEFAULT_PERIOD), billing::included_seconds("pro"), 0));
        // 요금제가 없으면 포함 시간은 없고 산 시간만 남는다.
        db.blocking(|c| c.execute("UPDATE users SET plan = 'none', extra_seconds = 60 WHERE id = ?1", params![uid])).unwrap();
        let u = db.blocking(|c| usage_of(c, uid, end + 2)).unwrap().unwrap();
        assert_eq!((u.included, u.remaining()), (0, 60));
    }

    #[test]
    fn rooms_that_never_ran_are_forgotten_and_others_end() {
        let db = Db::memory();
        db.blocking(|c| {
            c.execute("INSERT INTO users (id, email, created_at) VALUES (1, 'a@b.co', 1)", [])?;
            c.execute("INSERT INTO meetings (code, owner_id, created_at) VALUES ('aaaa-aaaa-aaaa', 1, 1)", [])?;
            c.execute("INSERT INTO meetings (code, owner_id, created_at, started_at, last_active_at) VALUES ('bbbb-bbbb-bbbb', 1, 1, 10, 70)", [])?;
            c.execute("INSERT INTO meetings (code, owner_id, created_at, starts_at) VALUES ('cccc-cccc-cccc', 1, 1, 999999)", [])?;
            // 이틀 뒤 회의를 미리 열어 봄 → 끝내지 않고, 미리 연 기록은 지운다.
            c.execute("INSERT INTO meetings (code, owner_id, created_at, starts_at, duration_min, started_at, last_active_at)
                       VALUES ('dddd-dddd-dddd', 1, 1, 999999, 60, 10, 70)", [])?;
            // 정한 시간이 지난 예약 회의는 끝낸다.
            c.execute("INSERT INTO meetings (code, owner_id, created_at, starts_at, duration_min, started_at, last_active_at)
                       VALUES ('eeee-eeee-eeee', 1, 1, 100, 5, 120, 400)", [])?;
            // 5초 일찍 시작해 정한 시간 안에서 쉬는 중 → 기록을 남기고 링크도 그대로 쓴다.
            c.execute("INSERT INTO meetings (code, owner_id, created_at, starts_at, duration_min, started_at, last_active_at)
                       VALUES ('ffff-ffff-ffff', 1, 1, 100, 60, 95, 300)", [])?;
            let codes = ["aaaa-aaaa-aaaa", "bbbb-bbbb-bbbb", "cccc-cccc-cccc", "dddd-dddd-dddd", "eeee-eeee-eeee", "ffff-ffff-ffff"];
            end_rooms(c, &codes.map(String::from), 500)
        })
        .unwrap();
        let rows: Vec<(String, Option<i64>, Option<i64>)> = db
            .blocking(|c| {
                let mut s = c.prepare("SELECT code, ended_at, started_at FROM meetings ORDER BY code")?;
                s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
            })
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("bbbb-bbbb-bbbb".into(), Some(70), Some(10)),
                ("cccc-cccc-cccc".into(), None, None),
                ("dddd-dddd-dddd".into(), None, None),
                ("eeee-eeee-eeee".into(), Some(400), Some(120)),
                ("ffff-ffff-ffff".into(), None, Some(95)),
            ]
        );
    }

    #[test]
    fn ics_escapes_text() {
        let t = ics_text(
            "abcd-efgh-ijkm",
            "Q1; plan, review",
            0,
            30,
            "https://x/abcd-efgh-ijkm",
        );
        assert!(t.contains("SUMMARY:Q1\\; plan\\, review\r\n"));
        assert!(t.contains("DTEND:19700101T003000Z"));
        // 긴 제목은 75바이트마다 접고, 글자를 가르지 않는다.
        let long = ics_text("abcd-efgh-ijkm", &"회의".repeat(40), 0, 30, "https://x/abcd-efgh-ijkm");
        assert!(long.split("\r\n").all(|l| l.len() <= 75));
        assert!(long.replace("\r\n ", "").contains(&format!("SUMMARY:{}\r\n", "회의".repeat(40))));
    }

    #[test]
    fn invite_time_names_the_end_date_after_midnight() {
        let start = billing::parse_rfc3339("2026-10-18T23:30:00Z").unwrap();
        assert_eq!(format_when(start, 90), "2026-10-18 23:30 – 2026-10-19 01:00 UTC");
        assert_eq!(format_when(start, 24 * 60), "2026-10-18 23:30 – 2026-10-19 23:30 UTC");
        assert_eq!(format_when(start - 3600, 30), "2026-10-18 22:30 – 23:00 UTC");
    }
}
