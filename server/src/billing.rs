//! 결제(Paddle, 판매 대행). 결제창은 브라우저의 Paddle.js가 열고, 서버는 웹훅으로 구독 상태를 받아
//! 계정의 요금제를 바꾼다. 해지·요금제 변경·결제 수단 관리는 Paddle API로 한다.
//! 키(PADDLE_*)가 없으면 결제 기능은 꺼진다. 개발 모드에서는 결제 없이 요금제를 켜 보는 경로를 둔다.

use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::{Value, json};

use crate::db::{Db, OptionalExtension, now_secs, params};

/// 요금제: 매달 포함되는 번역 시간(초).
pub fn included_seconds(plan: &str) -> i64 {
    match plan {
        "basic" => 10 * 3600,
        "pro" => 30 * 3600,
        _ => 0,
    }
}

pub struct Billing {
    api_key: String,
    pub client_token: String,
    webhook_secret: String,
    pub sandbox: bool,
    pub price_basic: String,
    pub price_pro: String,
    pub price_hour: String,
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
        let billing = Self {
            api_key: var("PADDLE_API_KEY")?,
            client_token: var("PADDLE_CLIENT_TOKEN")?,
            webhook_secret: var("PADDLE_WEBHOOK_SECRET")?,
            sandbox: var("PADDLE_ENV").as_deref() != Some("production"),
            price_basic: var("PADDLE_PRICE_BASIC")?,
            price_pro: var("PADDLE_PRICE_PRO")?,
            price_hour: var("PADDLE_PRICE_HOUR")?,
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
    ) -> Result<Value, String> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.api()))
            .bearer_auth(&self.api_key)
            .timeout(Duration::from_secs(20));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.map_err(|e| e.to_string())?;
        let status = res.status();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(body)
        } else {
            Err(format!("paddle {status}"))
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

    /// 계정 삭제 때: 바로 해지(남은 기간 없이).
    pub async fn cancel_now(&self, subscription: &str) -> Result<(), String> {
        self.call(
            reqwest::Method::POST,
            &format!("/subscriptions/{subscription}/cancel"),
            Some(json!({ "effective_from": "immediately" })),
        )
        .await
        .map(|_| ())
    }

    /// 이번 결제 기간이 끝나면 해지.
    pub async fn cancel_at_period_end(&self, subscription: &str) -> Result<(), String> {
        self.call(
            reqwest::Method::POST,
            &format!("/subscriptions/{subscription}/cancel"),
            Some(json!({ "effective_from": "next_billing_period" })),
        )
        .await
        .map(|_| ())
    }

    /// 예약된 해지를 취소.
    pub async fn resume(&self, subscription: &str) -> Result<(), String> {
        self.call(
            reqwest::Method::PATCH,
            &format!("/subscriptions/{subscription}"),
            Some(json!({ "scheduled_change": null })),
        )
        .await
        .map(|_| ())
    }

    /// 요금제 바꾸기(남은 기간만큼 바로 정산).
    pub async fn change_plan(&self, subscription: &str, plan: &str) -> Result<(), String> {
        let price = self.price_for(plan).ok_or("plan")?;
        self.call(
            reqwest::Method::PATCH,
            &format!("/subscriptions/{subscription}"),
            Some(json!({ "items": [{ "price_id": price, "quantity": 1 }], "proration_billing_mode": "prorated_immediately" })),
        )
        .await
        .map(|_| ())
    }

    /// 결제 수단·영수증을 보는 Paddle 고객 포털 주소.
    pub async fn portal_url(&self, customer: &str) -> Result<String, String> {
        let body = self
            .call(
                reqwest::Method::POST,
                &format!("/customers/{customer}/portal-sessions"),
                Some(json!({})),
            )
            .await?;
        body["data"]["urls"]["general"]["overview"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "portal".into())
    }

    /// `Paddle-Signature: ts=...;h1=...` 확인. 서명 대상은 "ts:원문".
    pub fn verify_signature(&self, header: &str, body: &[u8], now: i64) -> bool {
        verify_signature(&self.webhook_secret, header, body, now)
    }

    /// 웹훅 하나를 반영한다. 돌려주는 값: 영수증 메일을 보낼 (사용자, 요금제, 금액, 다음 결제일).
    pub async fn apply_event(&self, db: &Db, event: Value) -> Result<Option<Receipt>, String> {
        let id = event["event_id"].as_str().ok_or("event id")?.to_owned();
        let kind = event["event_type"].as_str().unwrap_or("").to_owned();
        let data = event["data"].clone();
        // 결제창이 넘긴 사용자 id는 브라우저가 바꿀 수 있으므로 서버 서명이 맞을 때만 믿는다.
        let custom_user = data["custom_data"]["user_id"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|uid| self.check_user(*uid, data["custom_data"]["sig"].as_str().unwrap_or("")));
        let subscription_ref = data["subscription_id"].as_str().map(str::to_owned);
        let prices: Vec<String> = data["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i["price"]["id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let plan = prices.iter().find_map(|p| self.plan_for_price(p));
        let hours: i64 = data["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter(|i| i["price"]["id"].as_str() == Some(self.price_hour.as_str()))
                    .map(|i| i["quantity"].as_i64().unwrap_or(1))
                    .sum()
            })
            .unwrap_or(0);
        let subscription = data["id"].as_str().map(str::to_owned);
        let customer = data["customer_id"].as_str().map(str::to_owned);
        let status = data["status"].as_str().unwrap_or("").to_owned();
        let renews = data["next_billed_at"].as_str().and_then(parse_rfc3339);
        let canceling = data["scheduled_change"]["action"].as_str() == Some("cancel");
        let total = data["details"]["totals"]["grand_total"]
            .as_str()
            .map(str::to_owned);
        let currency = data["currency_code"].as_str().unwrap_or("USD").to_owned();

        db.call(move |c| {
            let tx = c.transaction()?;
            // 같은 이벤트가 다시 와도(Paddle 재시도) 한 번만 반영한다.
            if tx.execute("INSERT OR IGNORE INTO paddle_events (id, received_at) VALUES (?1, ?2)", params![id, now_secs()])? == 0 {
                return Ok(None);
            }
            // 사용자: 서버가 서명한 custom_data가 기준이다. 서명이 없으면 구독 이벤트는 구독 id로만,
            // 결제(추가 시간) 이벤트는 고객 id로 이미 묶인 사용자를 찾는다.
            let is_subscription = kind.starts_with("subscription.");
            let by_sub: Option<i64> = if is_subscription {
                tx.query_row(
                    "SELECT id FROM users WHERE paddle_subscription_id = ?1",
                    params![subscription],
                    |r| r.get(0),
                )
                .optional()?
            } else {
                tx.query_row(
                    "SELECT id FROM users WHERE paddle_customer_id = ?1 OR paddle_subscription_id = ?2 LIMIT 1",
                    params![customer, subscription_ref],
                    |r| r.get(0),
                )
                .optional()?
            };
            let user = match (custom_user, by_sub) {
                // 이 구독이 이미 다른 사용자에게 묶여 있으면 반영하지 않는다(가로채기 방지).
                (Some(u), Some(l)) if u != l => {
                    tracing::warn!(event = %id, "paddle event user mismatch: ignored");
                    None
                }
                (Some(u), _) => Some(u),
                (None, l) => l,
            };
            let Some(user) = user else {
                tx.commit()?;
                return Ok(None);
            };
            let mut receipt = None;
            match kind.as_str() {
                "subscription.created" | "subscription.activated" | "subscription.updated" | "subscription.resumed" => {
                    let active = matches!(status.as_str(), "active" | "trialing" | "past_due");
                    let new_plan = if active { plan.unwrap_or("none") } else { "none" };
                    tx.execute(
                        "UPDATE users SET plan = ?2, plan_renews_at = ?3, plan_canceled = ?4,
                           paddle_subscription_id = ?5, paddle_customer_id = COALESCE(?6, paddle_customer_id) WHERE id = ?1",
                        params![user, new_plan, renews, canceling as i64, subscription, customer],
                    )?;
                    if kind == "subscription.activated" && active {
                        receipt = Some(Receipt { user, plan: new_plan.to_owned(), amount: String::new(), renews });
                    }
                }
                "subscription.canceled" | "subscription.paused" => {
                    tx.execute(
                        "UPDATE users SET plan = 'none', plan_canceled = 0, plan_renews_at = NULL WHERE id = ?1",
                        params![user],
                    )?;
                }
                "transaction.completed" if hours > 0 => {
                    tx.execute("UPDATE users SET extra_seconds = extra_seconds + ?2 WHERE id = ?1", params![user, hours * 3600])?;
                }
                _ => {}
            }
            if let (Some(r), Some(t)) = (&mut receipt, &total) {
                r.amount = format_amount(t, &currency);
            }
            tx.commit()?;
            Ok(receipt)
        })
        .await
        .map_err(|e| e.to_string())
    }
}

pub struct Receipt {
    pub user: i64,
    pub plan: String,
    pub amount: String,
    pub renews: Option<i64>,
}

/// Paddle 금액은 가장 작은 단위의 문자열(예: "1900" = $19.00).
fn format_amount(minor: &str, currency: &str) -> String {
    let n: i64 = minor.parse().unwrap_or(0);
    let symbol = if currency == "USD" { "$" } else { "" };
    format!(
        "{symbol}{}.{:02}{}",
        n / 100,
        n % 100,
        if symbol.is_empty() {
            format!(" {currency}")
        } else {
            String::new()
        }
    )
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
    // 5분보다 오래된 서명은 재전송 공격일 수 있다.
    if (now - ts).abs() > 300 {
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
    let sec = t
        .next()
        .and_then(|v| v.split('.').next()?.parse::<i64>().ok())
        .unwrap_or(0);
    // 1970-01-01부터의 날 수(그레고리력, Howard Hinnant의 days_from_civil)
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            http: reqwest::Client::new(),
        }
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
        assert!(b.apply_event(&db, event.clone()).await.unwrap().is_some());
        // 같은 이벤트는 다시 반영하지 않는다.
        assert!(b.apply_event(&db, event).await.unwrap().is_none());
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
}
