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

    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Inner { map, overflow, swept_at } = &mut *inner;
        if !map.contains_key(key) && map.len() >= self.cap {
            if swept_at.is_none_or(|t| now.saturating_duration_since(t).as_secs() >= 1) {
                *swept_at = Some(now);
                // 버킷이 다 찬(한동안 안 쓴) 키만 버린다.
                map.retain(|_, b| {
                    let tokens = (b.tokens + now.saturating_duration_since(b.at).as_secs_f64() * b.rate).min(b.burst);
                    tokens < b.burst
                });
                if map.len() >= self.cap {
                    tracing::warn!(cap = self.cap, "rate limiter full: new keys share the overflow bucket");
                }
            }
            if map.len() >= self.cap {
                return overflow.take(now);
            }
        }
        map.entry(key.to_owned())
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
}
