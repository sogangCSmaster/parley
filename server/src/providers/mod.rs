//! 외부 번역 제공자. 파이프라인은 `Translator` trait만 알고, 구현체를 바꿔 끼워 여러 제공자를 지원한다.

pub mod gemini;

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;

#[derive(Debug, Clone)]
pub struct TranslateRequest {
    /// BCP-47
    pub src_lang: String,
    pub tgt_lang: String,
    pub text: String,
    /// 말하는 중인 문장이면 false. 모델에게 뒷부분을 추측하지 말라고 알린다.
    pub complete: bool,
    /// 직전에 확정된 (원문, 번역) 쌍. 오래된 것부터.
    pub context: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct TranslateError(pub String);

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait Translator: Send + Sync {
    /// 번역 결과를 조각(토큰 묶음) 단위로 흘려보낸다. 요청은 스트림을 처음 poll할 때 시작한다.
    fn translate(
        &self,
        req: TranslateRequest,
    ) -> BoxStream<'static, Result<String, TranslateError>>;
}

pub struct Translation {
    pub text: String,
    pub first_token: Duration,
    pub total: Duration,
    pub hedged: bool,
}

/// 헤징: `hedge_after` 안에 첫 조각이 없으면 같은 요청을 하나 더 보내고, 먼저 첫 조각을 내는 쪽을 쓴다.
/// 드물게 생기는 20초급 지연을 없앤다(docs/benchmarks/2026-10-16-gemini.md). 진 쪽 요청은 버려서 취소한다.
pub async fn translate_hedged(
    translator: Arc<dyn Translator>,
    req: TranslateRequest,
    hedge_after: Duration,
) -> Result<Translation, TranslateError> {
    let started = Instant::now();
    let mut first = translator.translate(req.clone());
    let (head, rest, hedged) = match tokio::time::timeout(hedge_after, first.next()).await {
        Ok(head) => (head, first, false),
        Err(_) => {
            let mut second = translator.translate(req);
            tokio::select! {
                head = first.next() => (head, first, true),
                head = second.next() => (head, second, true),
            }
        }
    };
    let first_token = started.elapsed();
    let mut text = match head {
        Some(chunk) => chunk?,
        None => String::new(),
    };
    let mut rest = rest;
    while let Some(chunk) = rest.next().await {
        text.push_str(&chunk?);
    }
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(TranslateError("empty translation".into()));
    }
    Ok(Translation {
        text,
        first_token,
        total: started.elapsed(),
        hedged,
    })
}

/// 프롬프트에 넣을 영어 언어 이름. 모르는 태그는 그대로 쓴다(모델이 BCP-47도 이해한다).
pub fn language_name(tag: &str) -> &str {
    let primary = tag.split('-').next().unwrap_or(tag);
    match (primary, tag) {
        (_, "zh-TW" | "zh-Hant") => "Traditional Chinese",
        ("zh", _) => "Simplified Chinese",
        ("ko", _) => "Korean",
        ("ja", _) => "Japanese",
        ("en", _) => "English",
        ("es", _) => "Spanish",
        ("fr", _) => "French",
        ("de", _) => "German",
        ("vi", _) => "Vietnamese",
        _ => tag,
    }
}

#[cfg(test)]
pub mod testing {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::stream;

    use super::*;

    /// 호출마다 정해 둔 지연 뒤에 `[tgt] text`를 돌려주는 가짜 번역기.
    pub struct Fake {
        pub delays: Mutex<Vec<Duration>>,
        pub calls: AtomicUsize,
        pub requests: Mutex<Vec<TranslateRequest>>,
        pub fail: bool,
    }

    impl Fake {
        pub fn new(delays: Vec<Duration>) -> Self {
            Fake {
                delays: Mutex::new(delays),
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                fail: false,
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Translator for Fake {
        fn translate(
            &self,
            req: TranslateRequest,
        ) -> BoxStream<'static, Result<String, TranslateError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let delay = {
                let mut d = self.delays.lock().unwrap();
                if d.len() > 1 {
                    d.remove(0)
                } else {
                    d.first().copied().unwrap_or_default()
                }
            };
            let out = if self.fail {
                Err(TranslateError("fake failure".into()))
            } else {
                Ok(format!("[{}] {}", req.tgt_lang, req.text))
            };
            self.requests.lock().unwrap().push(req);
            stream::once(async move {
                tokio::time::sleep(delay).await;
                out
            })
            .boxed()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Fake;
    use super::*;

    fn req() -> TranslateRequest {
        TranslateRequest {
            src_lang: "ko".into(),
            tgt_lang: "ja".into(),
            text: "안녕".into(),
            complete: true,
            context: Vec::new(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fast_first_request_is_not_hedged() {
        let fake = Arc::new(Fake::new(vec![Duration::from_millis(900)]));
        let t = translate_hedged(fake.clone(), req(), Duration::from_millis(1500))
            .await
            .unwrap();
        assert_eq!(t.text, "[ja] 안녕");
        assert!(!t.hedged);
        assert_eq!(fake.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_first_request_is_hedged_and_second_wins() {
        let fake = Arc::new(Fake::new(vec![
            Duration::from_secs(20),
            Duration::from_millis(800),
        ]));
        let t = translate_hedged(fake.clone(), req(), Duration::from_millis(1500))
            .await
            .unwrap();
        assert!(t.hedged);
        assert_eq!(fake.calls(), 2);
        assert_eq!(t.first_token, Duration::from_millis(2300));
    }

    #[tokio::test(start_paused = true)]
    async fn failure_is_reported() {
        let mut fake = Fake::new(vec![Duration::ZERO]);
        fake.fail = true;
        assert!(
            translate_hedged(Arc::new(fake), req(), Duration::from_millis(1500))
                .await
                .is_err()
        );
    }

    #[test]
    fn language_names() {
        assert_eq!(language_name("ko"), "Korean");
        assert_eq!(language_name("zh-CN"), "Simplified Chinese");
        assert_eq!(language_name("zh-TW"), "Traditional Chinese");
        assert_eq!(language_name("th"), "th");
    }
}
