//! 빈도 제한(토큰 버킷).

use std::time::Instant;

/// 초당 `rate`개, 최대 `burst`개까지 허용한다.
#[derive(Clone)]
pub struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    at: Instant,
}

impl TokenBucket {
    pub fn new(rate: f64, burst: f64) -> Self {
        TokenBucket {
            rate,
            burst,
            tokens: burst,
            at: Instant::now(),
        }
    }

    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// 키(IP·이메일 등)마다 따로 세는 토큰 버킷.
/// 맵이 가득 차면(공격으로 키를 잔뜩 만든 경우) 아직 제한 중인 키는 버리지 않고(버리면 남의 제한이 풀린다),
/// 새 키들은 하나의 넘침 버킷을 함께 쓴다. 그동안 새 사용자는 느려지지만 모두 막히지는 않는다.
pub struct KeyedLimiter {
    rate: f64,
    burst: f64,
    cap: usize,
    inner: std::sync::Mutex<Inner>,
}

struct Inner {
    map: std::collections::HashMap<String, TokenBucket>,
    overflow: TokenBucket,
    /// 다 찬 버킷을 마지막으로 치운 때(가득 찬 동안 매 요청마다 맵 전체를 훑지 않게)
    swept_at: Option<Instant>,
}

impl KeyedLimiter {
    /// `per`초 동안 `count`번(처음에 몰아서 `count`번까지).
    pub fn new(count: u32, per: std::time::Duration) -> Self {
        Self::with_cap(count, per, 100_000)
    }

    fn with_cap(count: u32, per: std::time::Duration, cap: usize) -> Self {
        let rate = f64::from(count) / per.as_secs_f64();
        let burst = f64::from(count);
        KeyedLimiter {
            rate,
            burst,
            cap,
            inner: std::sync::Mutex::new(Inner {
                map: Default::default(),
                // 넘침 버킷: 키 하나 몫. 새 키들을 다 합쳐도 키 하나의 한도를 넘지 못한다(한도가 느슨해지지 않게).
                // 맵을 채우기는 어렵다: 아무 값이나 넣을 수 있는 키(주소)는 요청한 쪽(IP·/48·사용자) 한도를 통과한 뒤에만 쌓인다.
                overflow: TokenBucket::new(rate, burst),
                swept_at: None,
            }),
        }
    }

    /// 지금 이 키로 한 번 허용될지(토큰을 쓰지도, 키를 만들지도 않는다). 여러 단위를 함께 볼 때
    /// 모두 허용될 때만 `check`로 쓰려고(한 단위에서 막힌 요청이 다른 단위의 몫을 쓰거나 키를 쌓지 않게).
    pub fn allows(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let ready = |b: &TokenBucket| (b.tokens + now.saturating_duration_since(b.at).as_secs_f64() * b.rate).min(b.burst) >= 1.0;
        if let Some(b) = inner.map.get(key) {
            return ready(b);
        }
        // 없는 키: `check`와 같게, 가득 찼으면 먼저 치워 보고 그래도 가득하면 넘침 버킷으로 판단한다.
        if self.sweep_if_full(&mut inner, now) {
            ready(&inner.overflow)
        } else {
            true
        }
    }

    /// 맵이 가득 찼으면 다 찬(한동안 안 쓴) 버킷을 치운다(1초에 한 번까지). 그래도 가득하면 true.
    fn sweep_if_full(&self, inner: &mut Inner, now: Instant) -> bool {
        if inner.map.len() < self.cap {
            return false;
        }
        if inner.swept_at.is_none_or(|t| now.saturating_duration_since(t).as_secs() >= 1) {
            inner.swept_at = Some(now);
            inner.map.retain(|_, b| {
                let tokens = (b.tokens + now.saturating_duration_since(b.at).as_secs_f64() * b.rate).min(b.burst);
                tokens < b.burst
            });
            if inner.map.len() >= self.cap {
                tracing::warn!(cap = self.cap, "rate limiter full: new keys share the overflow bucket");
            }
        }
        inner.map.len() >= self.cap
    }

    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.map.contains_key(key) && self.sweep_if_full(&mut inner, now) {
            return inner.overflow.take(now);
        }
        inner
            .map
            .entry(key.to_owned())
            .or_insert_with(|| TokenBucket::new(self.rate, self.burst))
            .take(now)
    }
}

#[cfg(test)]
mod keyed_tests {
    use super::*;

    #[test]
    fn each_key_has_its_own_budget() {
        let l = KeyedLimiter::new(2, std::time::Duration::from_secs(3600));
        assert!(l.check("a") && l.check("a"));
        assert!(!l.check("a"));
        assert!(l.check("b"));
    }

    #[test]
    fn full_map_keeps_limits_and_shares_overflow() {
        let l = KeyedLimiter::with_cap(1, std::time::Duration::from_secs(3600), 2);
        assert!(l.check("a") && l.check("b"));
        // 가득 차도 제한 중인 키는 풀리지 않는다.
        assert!(!l.check("a"));
        // 새 키들은 넘침 버킷(키 하나 몫)을 함께 쓴다.
        assert!(l.check("new0"));
        assert!(!l.check("new1"));
    }

    #[test]
    fn allows_does_not_spend_or_insert() {
        let l = KeyedLimiter::new(1, std::time::Duration::from_secs(3600));
        assert!(l.allows("a") && l.allows("a"));
        assert!(l.check("a"));
        assert!(!l.allows("a"));
    }

    #[test]
    fn allows_uses_overflow_when_full() {
        let l = KeyedLimiter::with_cap(1, std::time::Duration::from_secs(3600), 1);
        assert!(l.check("a"));
        assert!(l.allows("new") && l.check("new"));
        // 넘침 버킷도 다 썼으면 없는 키도 허용하지 않는다.
        assert!(!l.allows("other"));
    }
}
