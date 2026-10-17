//! 회의 기록(예약·지난 회의), 번역 사용량과 한도, 요금제·결제 API.
//! 회의실 자체(참가자·신호)는 메모리의 `Rooms`가 맡고, 여기서는 오래 남겨야 하는 것만 DB에 둔다.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
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

/// 유닉스 초 → (연, 월). 사용량은 UTC 달력 달마다 다시 채운다.
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

struct Usage {
    plan: String,
    used: i64,
    extra: i64,
}

fn usage_of(c: &rusqlite::Connection, uid: i64, now: i64) -> rusqlite::Result<Option<Usage>> {
    let row: Option<(String, i64)> = c
        .query_row(
            "SELECT plan, extra_seconds FROM users WHERE id = ?1",
            params![uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((plan, extra)) = row else {
        return Ok(None);
    };
    let used: i64 = c.query_row(
        "SELECT COALESCE(SUM(seconds), 0) FROM usage WHERE user_id = ?1 AND period = ?2",
        params![uid, period(now)],
        |r| r.get(0),
    )?;
    Ok(Some(Usage { plan, used, extra }))
}

impl Usage {
    fn included(&self) -> i64 {
        included_seconds(&self.plan)
    }
    /// 이번 달 남은 시간: 요금제에 포함된 남은 시간 + 사 둔 추가 시간.
    fn remaining(&self) -> i64 {
        (self.included() - self.used).max(0) + self.extra
    }
}

/// 잰 번역 시간을 기록하고, 시간이 다 떨어진 주최자 목록을 돌려준다.
/// 요금제에 포함된 시간을 넘긴 만큼은 사 둔 추가 시간에서 뺀다.
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
        let Ok(uid) = m.host.parse::<i64>() else {
            continue;
        };
        let Some(before) = usage_of(&tx, uid, now)? else {
            continue;
        };
        tx.execute(
            "INSERT INTO usage (user_id, meeting_code, period, seconds, recorded_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![uid, m.code, period(now), m.seconds, now],
        )?;
        let over_before = (before.used - before.included()).max(0);
        let over_after = (before.used + m.seconds - before.included()).max(0);
        if over_after > over_before {
            tx.execute(
                "UPDATE users SET extra_seconds = MAX(0, extra_seconds - ?2) WHERE id = ?1",
                params![uid, over_after - over_before],
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
        c.execute(
            "UPDATE meetings SET ended_at = COALESCE(last_active_at, ?2) WHERE code = ?1 AND ended_at IS NULL AND started_at IS NOT NULL",
            params![code, now],
        )?;
        c.execute(
            "DELETE FROM meetings WHERE code = ?1 AND starts_at IS NULL AND started_at IS NULL",
            params![code],
        )?;
    }
    Ok(())
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
    let plan: String = db
        .call(move |c| {
            c.query_row("SELECT plan FROM users WHERE id = ?1", params![uid], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap_or_else(|_| "none".into());
    if plan == "none" {
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
    let meta: Option<(String, String)> = state
        .auth
        .db()
        .call(move |conn| {
            conn.query_row(
                "SELECT m.title, u.name FROM meetings m JOIN users u ON u.id = m.owner_id WHERE m.code = ?1",
                params![c],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    let (title, host) = meta.unwrap_or_default();
    Json(json!({ "translation": settings.translation, "title": title, "host": host }))
        .into_response()
}

// ── 예약 회의 ─────────────────────────────────────────

#[derive(Deserialize)]
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

fn clean_meeting(req: &MeetingReq) -> Result<CleanMeeting, &'static str> {
    let title: String = req
        .title
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TITLE)
        .collect();
    if title.is_empty() {
        return Err("title");
    }
    if !(5..=24 * 60).contains(&req.duration_min) {
        return Err("duration");
    }
    // 과거로는 예약하지 않는다(지금부터 10분 전까지는 허용: 바로 시작하는 회의를 예약하는 경우).
    if req.starts_at < now_secs() - 600 || req.starts_at > now_secs() + 400 * 86_400 {
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
    if guests.len() > MAX_GUESTS {
        return Err("guests");
    }
    Ok(CleanMeeting {
        title,
        kind: clean_kind(&req.kind),
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
        "lengthSec": match (started, ended) { (Some(s), Some(e)) => (e - s).max(0), _ => 0 },
        "translatedSec": r.get::<_, i64>("translated_seconds")?,
    }))
}

const MEETING_COLUMNS: &str = "id, code, title, kind, lang_a, lang_b, starts_at, duration_min, guests, created_at, started_at, ended_at, translated_seconds";

/// 다가오는 예약 회의와 지난 회의.
pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let uid = match require_user(&state, &headers) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let now = now_secs();
    let res = state
        .auth
        .db()
        .call(move |c| {
            let mut up = c.prepare(&format!(
                "SELECT {MEETING_COLUMNS} FROM meetings WHERE owner_id = ?1 AND ended_at IS NULL AND starts_at IS NOT NULL
                   AND starts_at + duration_min * 60 > ?2 ORDER BY starts_at LIMIT 50"
            ))?;
            let upcoming = up.query_map(params![uid, now], meeting_json)?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut past = c.prepare(&format!(
                "SELECT {MEETING_COLUMNS} FROM meetings WHERE owner_id = ?1 AND ended_at IS NOT NULL ORDER BY ended_at DESC LIMIT 50"
            ))?;
            let past = past.query_map(params![uid], meeting_json)?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({ "upcoming": upcoming, "past": past }))
        })
        .await;
    match res {
        Ok(v) => Json(v).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
}

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
    if plan_of(&db, uid).await == "none" {
        return error(StatusCode::PAYMENT_REQUIRED, "plan");
    }
    let m = match clean_meeting(&req) {
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
        Ok(Some(meeting)) => {
            if req.invite {
                send_invites(&state, uid, &meeting).await;
            }
            (StatusCode::CREATED, Json(meeting)).into_response()
        }
        Ok(None) => error(StatusCode::TOO_MANY_REQUESTS, "too_many"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    }
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
    let m = match clean_meeting(&req) {
        Ok(m) => m,
        Err(field) => return error(StatusCode::BAD_REQUEST, field),
    };
    let db = state.auth.db().clone();
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
    if req.invite {
        send_invites(&state, uid, &meeting).await;
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
    let n = state
        .auth
        .db()
        .call(move |c| {
            c.execute(
                "DELETE FROM meetings WHERE id = ?1 AND owner_id = ?2",
                params![id, uid],
            )
        })
        .await
        .unwrap_or(0);
    if n == 1 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error(StatusCode::NOT_FOUND, "missing")
    }
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
    let db = state.auth.db().clone();
    let Some(mut meeting) = owned(&db, uid, id).await else {
        return error(StatusCode::NOT_FOUND, "missing");
    };
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
    let only: Vec<String> = req
        .guests
        .iter()
        .map(|g| crate::auth::normalize_username(g))
        .collect();
    let mut subset = meeting.clone();
    subset["guests"] = json!(only);
    send_invites(&state, uid, &subset).await;
    Json(meeting).into_response()
}

/// 손님마다 초대 메일. 손님의 언어를 모르므로 주최자의 화면 언어로 보낸다.
async fn send_invites(state: &AppState, uid: i64, meeting: &Value) {
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
        return;
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
    for g in meeting["guests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
    {
        state
            .accounts
            .mailer
            .send(Copy { lang }.invite(g, &host_name, title, &when, &url));
    }
}

/// 메일 속 회의 시각. 받는 사람의 시간대를 모르므로 UTC로 적는다(캘린더 파일이 현지 시각으로 바꿔 준다).
fn format_when(start: i64, minutes: i64) -> String {
    let (y, m) = year_month(start);
    let day = (start
        - billing::parse_rfc3339(&format!("{y:04}-{m:02}-01T00:00:00Z")).unwrap_or(start))
        / 86_400
        + 1;
    let hm = |t: i64| {
        format!(
            "{:02}:{:02}",
            t.rem_euclid(86_400) / 3600,
            t.rem_euclid(3600) / 60
        )
    };
    format!(
        "{y:04}-{m:02}-{day:02} {} – {} UTC",
        hm(start),
        hm(start + minutes * 60)
    )
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
    let Some(m) = owned(state.auth.db(), uid, id).await else {
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
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
        .replace('\r', "")
}

pub fn ics_text(code: &str, title: &str, start: i64, minutes: i64, url: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Tolkmeet//EN\r\nCALSCALE:GREGORIAN\r\nMETHOD:PUBLISH\r\nBEGIN:VEVENT\r\nUID:{code}@tolkmeet.com\r\nDTSTAMP:{}\r\nDTSTART:{}\r\nDTEND:{}\r\nSUMMARY:{}\r\nDESCRIPTION:{}\r\nURL:{url}\r\nLOCATION:{url}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        ics_time(now_secs()),
        ics_time(start),
        ics_time(start + minutes * 60),
        ics_escape(title),
        ics_escape(&format!("Tolkmeet: {url}")),
    )
}

// ── 요금제·결제 ───────────────────────────────────────

/// 이번 달 번역 시간과 요금제.
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
            Ok(u.map(|u| {
                json!({
                    "plan": u.plan,
                    "includedSec": u.included(),
                    "usedSec": u.used,
                    "extraSec": u.extra,
                    "remainingSec": u.remaining(),
                    "resetsAt": next_reset(now),
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
pub struct CheckoutReq {
    /// "basic" | "pro" | "hours"
    item: String,
    #[serde(default = "one")]
    quantity: i64,
}

fn one() -> i64 {
    1
}

/// 결제창(Paddle.js)에 넘길 값. 금액은 Paddle에 등록된 가격 id로만 정해지므로 브라우저가 바꿀 수 없다.
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
    if !matches!(req.item.as_str(), "basic" | "pro" | "hours") || !(1..=100).contains(&req.quantity)
    {
        return error(StatusCode::BAD_REQUEST, "item");
    }
    let Some(b) = &state.billing else {
        return if state.dev_mode {
            Json(json!({ "dev": true })).into_response()
        } else {
            error(StatusCode::SERVICE_UNAVAILABLE, "billing")
        };
    };
    let price = if req.item == "hours" {
        Some(b.price_hour.as_str())
    } else {
        b.price_for(&req.item)
    };
    let email: String = state
        .auth
        .db()
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
        "priceId": price,
        "quantity": if req.item == "hours" { req.quantity } else { 1 },
        "email": email,
        "customData": { "user_id": uid.to_string() },
    }))
    .into_response()
}

/// 개발 모드 전용: 결제 없이 요금제를 켜거나 시간을 더한다(결제 흐름 화면 확인용).
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
    let renews = next_reset(now_secs());
    let res = state
        .auth
        .db()
        .call(move |c| match req.item.as_str() {
            "basic" | "pro" => c.execute(
                "UPDATE users SET plan = ?2, plan_renews_at = ?3, plan_canceled = 0 WHERE id = ?1",
                params![uid, req.item, renews],
            ),
            _ => c.execute(
                "UPDATE users SET extra_seconds = extra_seconds + ?2 WHERE id = ?1",
                params![uid, req.quantity.clamp(1, 100) * 3600],
            ),
        })
        .await;
    match res {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
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
pub struct PlanReq {
    #[serde(default)]
    plan: String,
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
                "change" if matches!(plan.as_str(), "basic" | "pro") => c.execute(
                    "UPDATE users SET plan = ?2 WHERE id = ?1",
                    params![uid, plan],
                ),
                _ => Ok(0),
            })
            .await;
        return match res {
            Ok(1) => StatusCode::NO_CONTENT.into_response(),
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
            b.change_plan(&sub, &req.plan).await
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
        Ok(receipt) => {
            if let Some(r) = receipt {
                send_receipt(&state, r).await;
            }
            // 추가 시간을 샀으면 막혀 있던 회의가 바로 풀리도록 한도를 다시 본다.
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
    let check = state.rooms.out_of_time_hosts();
    let metered: Vec<Metered> = metered
        .iter()
        .map(|m| Metered {
            host: m.host.clone(),
            code: m.code.clone(),
            seconds: m.seconds,
        })
        .collect();
    let now = now_secs();
    match state
        .auth
        .db()
        .call(move |c| record_usage(c, &metered, &check, now))
        .await
    {
        Ok(out) => state.rooms.set_out_of_time(out),
        Err(e) => tracing::warn!(error = %e, "usage not recorded"),
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
            }]
        };
        // 포함된 10시간 중 9시간 59분 쓰기 → 아직 남음
        let out = db
            .blocking(|c| record_usage(c, &m(10 * 3600 - 60), &HashSet::new(), now))
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

    #[test]
    fn rooms_that_never_ran_are_forgotten_and_others_end() {
        let db = Db::memory();
        db.blocking(|c| {
            c.execute("INSERT INTO users (id, email, created_at) VALUES (1, 'a@b.co', 1)", [])?;
            c.execute("INSERT INTO meetings (code, owner_id, created_at) VALUES ('aaaa-aaaa-aaaa', 1, 1)", [])?;
            c.execute("INSERT INTO meetings (code, owner_id, created_at, started_at, last_active_at) VALUES ('bbbb-bbbb-bbbb', 1, 1, 10, 70)", [])?;
            c.execute("INSERT INTO meetings (code, owner_id, created_at, starts_at) VALUES ('cccc-cccc-cccc', 1, 1, 999999)", [])?;
            end_rooms(c, &["aaaa-aaaa-aaaa".into(), "bbbb-bbbb-bbbb".into(), "cccc-cccc-cccc".into()], 500)
        })
        .unwrap();
        let rows: Vec<(String, Option<i64>)> = db
            .blocking(|c| {
                let mut s = c.prepare("SELECT code, ended_at FROM meetings ORDER BY code")?;
                s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
            })
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("bbbb-bbbb-bbbb".into(), Some(70)),
                ("cccc-cccc-cccc".into(), None)
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
    }
}
