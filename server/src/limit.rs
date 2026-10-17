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

/// 키(IP·이메일 등)마다 따로 세는 토큰 버킷. 맵은 상한을 넘으면 오래된 것부터 버린다.
pub struct KeyedLimiter {
    rate: f64,
    burst: f64,
    cap: usize,
    map: std::sync::Mutex<std::collections::HashMap<String, TokenBucket>>,
}

impl KeyedLimiter {
    /// `per`초 동안 `count`번(처음에 몰아서 `count`번까지).
    pub fn new(count: u32, per: std::time::Duration) -> Self {
        KeyedLimiter {
            rate: f64::from(count) / per.as_secs_f64(),
            burst: f64::from(count),
            cap: 10_000,
            map: Default::default(),
        }
    }

    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if !map.contains_key(key) && map.len() >= self.cap {
            // 가득 차면 버킷이 다 찬(오래 안 쓴) 것부터 버린다.
            map.retain(|_, b| {
                let mut b = b.clone();
                b.tokens = (b.tokens + now.saturating_duration_since(b.at).as_secs_f64() * b.rate).min(b.burst);
                b.tokens < b.burst
            });
            // 그래도 가득하면 가장 오래 안 쓴 키 하나를 버린다(가득 찼다고 모든 새 키를 막으면 누구도 못 쓴다).
            if map.len() >= self.cap
                && let Some(oldest) = map.iter().min_by_key(|(_, b)| b.at).map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
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
}
