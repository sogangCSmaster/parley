//! Gemini `streamGenerateContent`(SSE) 번역. 설정은 docs/benchmarks/2026-10-16-gemini.md에서 측정으로 정했다:
//! `gemini-3.5-flash-lite` + `thinkingLevel: minimal`, 유료 Standard에서 첫 토큰 p50 1.0s.

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};
use serde_json::{Value, json};

use super::{TranslateError, TranslateRequest, Translator, language_name};

const API: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const DEFAULT_MODEL: &str = "gemini-3.5-flash-lite";

const SYSTEM: &str = "You translate live meeting captions. The speaker may still be mid-sentence. \
Translate the CURRENT utterance from {src} to {tgt} naturally, using the previous lines only as context. \
If it is incomplete, translate what is there without guessing the rest. Output only the translation.";

pub struct Gemini {
    client: reqwest::Client,
    key: String,
    model: String,
}

impl Gemini {
    pub fn new(key: String, model: String) -> Self {
        let client = reqwest::Client::builder()
            // TLS 연결 수립(약 0.1s)을 요청마다 반복하지 않도록 연결을 오래 살려 둔다.
            .pool_idle_timeout(None)
            .http2_keep_alive_interval(Duration::from_secs(30))
            .http2_keep_alive_while_idle(true)
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client");
        Gemini { client, key, model }
    }

    /// 첫 번역 요청이 TLS 수립 시간을 물지 않도록 시작할 때 연결을 미리 열어 둔다.
    pub async fn warm_up(&self) {
        let res = self
            .client
            .get(format!("{API}/models/{}", self.model))
            .header("x-goog-api-key", &self.key)
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {
                tracing::info!(model = %self.model, "translator ready")
            }
            Ok(r) => {
                tracing::warn!(status = %r.status(), model = %self.model, "translator warm-up failed")
            }
            Err(err) => tracing::warn!(%err, "translator warm-up failed"),
        }
    }

    fn body(req: &TranslateRequest) -> Value {
        let system = SYSTEM
            .replace("{src}", language_name(&req.src_lang))
            .replace("{tgt}", language_name(&req.tgt_lang));
        let mut user = String::new();
        if !req.context.is_empty() {
            user.push_str("Previous lines:\n");
            for (src, tgt) in &req.context {
                user.push_str(&format!("{src} => {tgt}\n"));
            }
            user.push('\n');
        }
        user.push_str(&format!(
            "Current utterance ({}):\n{}",
            if req.complete {
                "complete"
            } else {
                "in progress"
            },
            req.text
        ));
        json!({
            "systemInstruction": { "parts": [{ "text": system }] },
            "contents": [{ "role": "user", "parts": [{ "text": user }] }],
            "generationConfig": {
                "temperature": 0.2,
                "maxOutputTokens": 256,
                "thinkingConfig": { "thinkingLevel": "minimal" },
            },
        })
    }
}

enum State {
    Start(reqwest::RequestBuilder),
    Body {
        bytes: BoxStream<'static, reqwest::Result<bytes::Bytes>>,
        buf: Vec<u8>,
        ready: VecDeque<String>,
    },
    Done,
}

impl Translator for Gemini {
    fn translate(
        &self,
        req: TranslateRequest,
    ) -> BoxStream<'static, Result<String, TranslateError>> {
        let request = self
            .client
            .post(format!(
                "{API}/models/{}:streamGenerateContent?alt=sse",
                self.model
            ))
            .header("x-goog-api-key", &self.key)
            .timeout(Duration::from_secs(30))
            .json(&Self::body(&req));
        stream::unfold(State::Start(request), |mut state| async move {
            loop {
                match state {
                    State::Done => return None,
                    State::Start(request) => {
                        let res = match request.send().await {
                            Ok(res) => res,
                            Err(err) => return Some((Err(error(err)), State::Done)),
                        };
                        if !res.status().is_success() {
                            // 응답 본문은 로그에 남기지 않는다(요청 내용이 되돌아올 수 있다).
                            let err = TranslateError(format!("HTTP {}", res.status()));
                            return Some((Err(err), State::Done));
                        }
                        state = State::Body {
                            bytes: res.bytes_stream().boxed(),
                            buf: Vec::new(),
                            ready: VecDeque::new(),
                        };
                    }
                    State::Body {
                        mut bytes,
                        mut buf,
                        mut ready,
                    } => {
                        if let Some(text) = ready.pop_front() {
                            return Some((Ok(text), State::Body { bytes, buf, ready }));
                        }
                        match bytes.next().await {
                            Some(Ok(chunk)) => {
                                buf.extend_from_slice(&chunk);
                                drain_events(&mut buf, &mut ready);
                                state = State::Body { bytes, buf, ready };
                            }
                            Some(Err(err)) => return Some((Err(error(err)), State::Done)),
                            None => state = State::Done,
                        }
                    }
                }
            }
        })
        .boxed()
    }
}

fn error(err: reqwest::Error) -> TranslateError {
    // URL에는 키가 없지만(헤더로 보낸다) 혹시 모를 노출을 막기 위해 URL을 지운다.
    TranslateError(err.without_url().to_string())
}

/// 완성된 SSE 이벤트(`\n\n`으로 끝남)에서 번역 텍스트를 꺼낸다. 생각(thought) 조각은 버린다.
fn drain_events(buf: &mut Vec<u8>, ready: &mut VecDeque<String>) {
    while let Some((end, sep)) = find_event_end(buf) {
        let event: Vec<u8> = buf.drain(..end + sep).collect();
        let event = String::from_utf8_lossy(&event[..end]);
        for line in event.lines() {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(data.trim()) else {
                continue;
            };
            let parts = v
                .pointer("/candidates/0/content/parts")
                .and_then(Value::as_array);
            for p in parts.into_iter().flatten() {
                if p.get("thought").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                if let Some(t) = p
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    ready.push_back(t.to_owned());
                }
            }
        }
    }
}

fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_events_across_chunks() {
        let mut buf = Vec::new();
        let mut ready = VecDeque::new();
        let event = |t: &str| {
            format!(r#"data: {{"candidates":[{{"content":{{"parts":[{{"text":"{t}"}}]}}}}]}}"#)
        };
        let raw = format!(
            "{}\n\ndata: {{\"candidates\":[{{\"content\":{{\"parts\":[{{\"text\":\"skip\",\"thought\":true}}]}}}}]}}\r\n\r\n{}\n\n",
            event("저희 "),
            event("투자팀은")
        );
        let bytes = raw.as_bytes();
        // 멀티바이트 문자 중간에서 잘린 조각도 이어 붙여 해석해야 한다.
        let cut = bytes.iter().position(|&b| b >= 0x80).unwrap() + 1;
        buf.extend_from_slice(&bytes[..cut]);
        drain_events(&mut buf, &mut ready);
        assert!(ready.is_empty());
        buf.extend_from_slice(&bytes[cut..]);
        drain_events(&mut buf, &mut ready);
        assert_eq!(ready, ["저희 ", "투자팀은"]);
        assert!(buf.is_empty());
    }

    #[test]
    fn prompt_includes_context_and_progress() {
        let body = Gemini::body(&TranslateRequest {
            src_lang: "ja".into(),
            tgt_lang: "ko".into(),
            text: "主にAI".into(),
            complete: false,
            context: vec![("はい。".into(), "네.".into())],
        });
        let system = body
            .pointer("/systemInstruction/parts/0/text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(system.contains("from Japanese to Korean"));
        let user = body
            .pointer("/contents/0/parts/0/text")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(
            user,
            "Previous lines:\nはい。 => 네.\n\nCurrent utterance (in progress):\n主にAI"
        );
        assert_eq!(
            body.pointer("/generationConfig/thinkingConfig/thinkingLevel")
                .unwrap(),
            "minimal"
        );
    }
}
