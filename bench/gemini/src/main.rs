//! Gemini 실시간 자막 파이프라인 측정 도구.
//!
//! - `translate`: Flash-Lite 스트리밍 번역을 "말하는 도중의 부분 문장 → 완성 문장" 순서로 호출해
//!   첫 토큰 시간(TTFT)·전체 시간·토큰 수·비용을 잰다.
//! - `transcribe`: 16kHz WAV를 실시간 속도(100ms 조각)로 Live API에 흘려 넣고
//!   중간 결과가 오는 간격, 누적/증분 여부, 말 끝 → 확정 결과 지연을 잰다.
//!
//! API 키는 `GEMINI_API_KEY` 환경변수 또는 이 폴더의 `.env`(git 제외)에서만 읽고, 출력하지 않는다.

use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

const API: &str = "https://generativelanguage.googleapis.com/v1beta";
const LIVE_WS: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str, default: &str| -> String {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
            .unwrap_or_else(|| default.to_owned())
    };
    let key = api_key();
    match args.first().map(String::as_str) {
        Some("translate") => {
            let model = flag("--model", "gemini-3.5-flash-lite");
            let thinking = flag("--thinking", "minimal");
            let runs: usize = flag("--runs", "1")
                .parse()
                .expect("--runs must be a number");
            translate(&key, &model, &thinking, runs).await;
        }
        Some("transcribe") => {
            let file = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .cloned()
                .unwrap_or_else(|| usage());
            let lang = flag("--lang", "");
            transcribe(
                &key,
                &flag("--model", "gemini-3.5-transcribe-live"),
                &file,
                &lang,
            )
            .await;
        }
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  gemini-bench translate [--model gemini-3.5-flash-lite] [--thinking minimal|low|none] [--runs 1]\n  gemini-bench transcribe fixtures/ko.wav [--lang ko] [--model gemini-3.5-transcribe-live]"
    );
    std::process::exit(2)
}

fn api_key() -> String {
    if let Ok(k) = std::env::var("GEMINI_API_KEY")
        && !k.trim().is_empty()
    {
        return k.trim().to_owned();
    }
    let env_path = concat!(env!("CARGO_MANIFEST_DIR"), "/.env");
    std::fs::read_to_string(env_path)
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.trim().strip_prefix("GEMINI_API_KEY=").map(|v| v.trim().to_owned())))
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| {
            eprintln!("GEMINI_API_KEY가 없습니다. 환경변수로 넘기거나 bench/gemini/.env 에 GEMINI_API_KEY=... 를 넣어 주세요.");
            std::process::exit(2)
        })
}

// ───────────────────────── translate ─────────────────────────

struct Sample {
    src: &'static str,
    tgt: &'static str,
    /// 직전에 확정된 문장 쌍(문맥으로 함께 보냄)
    context: (&'static str, &'static str),
    text: &'static str,
}

const SAMPLES: &[Sample] = &[
    Sample {
        src: "Korean",
        tgt: "Japanese",
        context: (
            "오늘 시간 내주셔서 감사합니다.",
            "本日はお時間をいただきありがとうございます。",
        ),
        text: "저희 투자팀은 네 명의 멤버로 구성되어 있고요, 주로 인공지능이랑 최근에는 딥테크 분야에 집중적으로 투자하고 있습니다.",
    },
    Sample {
        src: "Japanese",
        tgt: "Korean",
        context: (
            "こちらこそ、よろしくお願いします。",
            "저야말로 잘 부탁드립니다.",
        ),
        text: "私たちの投資チームは四人のメンバーで構成されていて、主にAI、それから最近だとディープテックといったところへの投資に注力しています。",
    },
];

const SYSTEM: &str = "You translate live meeting captions. The speaker may still be mid-sentence. \
Translate the CURRENT utterance from {src} to {tgt} naturally, using the previous lines only as context. \
If it is incomplete, translate what is there without guessing the rest. Output only the translation.";

/// 말하는 도중 STT가 내놓을 법한 부분 문장들. 한국어는 어절 2개씩, 일본어는 6글자씩 늘어난다.
fn partials(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    if text.contains(' ') {
        let words: Vec<&str> = text.split(' ').collect();
        for n in (2..words.len()).step_by(2) {
            out.push(words[..n].join(" "));
        }
    } else {
        let chars: Vec<char> = text.chars().collect();
        for n in (6..chars.len()).step_by(6) {
            out.push(chars[..n].iter().collect());
        }
    }
    out
}

struct Call {
    is_final: bool,
    ttft: Option<Duration>,
    total: Duration,
    input: u64,
    output: u64,
    thoughts: u64,
    text: String,
    /// HTTP 오류(과부하 503 등). 측정을 멈추지 않고 기록만 한다.
    error: Option<String>,
}

async fn translate(key: &str, model: &str, thinking: &str, runs: usize) {
    let (price_in, price_out) = prices(model);
    let client = reqwest::Client::new();
    // 연결 수립(TLS) 비용은 실제 서비스에서 재사용되므로 측정에서 뺀다.
    let _ = client
        .get(format!("{API}/models/{model}"))
        .header("x-goog-api-key", key)
        .send()
        .await;

    println!("model={model} thinking={thinking} runs={runs}\n");
    let mut calls = Vec::new();
    for run in 0..runs {
        for s in SAMPLES {
            println!("── run {} · {} → {}", run + 1, s.src, s.tgt);
            let mut steps: Vec<(bool, String)> =
                partials(s.text).into_iter().map(|p| (false, p)).collect();
            steps.push((true, s.text.to_owned()));
            for (is_final, current) in steps {
                let call =
                    translate_once(&client, key, model, thinking, s, &current, is_final).await;
                if let Some(err) = &call.error {
                    println!(
                        "  {} ERROR after {}ms: {}",
                        if is_final { "FINAL  " } else { "partial" },
                        call.total.as_millis(),
                        err
                    );
                    calls.push(call);
                    continue;
                }
                println!(
                    "  {} ttft {:>5} total {:>5}ms  in {:>4} out {:>3} think {:>3}  {}",
                    if is_final { "FINAL  " } else { "partial" },
                    call.ttft
                        .map(|d| format!("{}ms", d.as_millis()))
                        .unwrap_or_else(|| "-".into()),
                    call.total.as_millis(),
                    call.input,
                    call.output,
                    call.thoughts,
                    truncate(&call.text, 60),
                );
                calls.push(call);
            }
        }
    }

    let errors = calls.iter().filter(|c| c.error.is_some()).count();
    println!("\n== 요약 ({} calls, 오류 {errors})", calls.len());
    let calls: Vec<Call> = calls.into_iter().filter(|c| c.error.is_none()).collect();
    for (label, finals) in [("partial", false), ("final", true)] {
        let group: Vec<&Call> = calls.iter().filter(|c| c.is_final == finals).collect();
        let ttft: Vec<u128> = group
            .iter()
            .filter_map(|c| c.ttft.map(|d| d.as_millis()))
            .collect();
        let total: Vec<u128> = group.iter().map(|c| c.total.as_millis()).collect();
        println!(
            "  {label:<7} n={:<3} TTFT p50 {}ms p90 {}ms max {}ms | total p50 {}ms p90 {}ms",
            group.len(),
            pct(&ttft, 50),
            pct(&ttft, 90),
            ttft.iter().max().copied().unwrap_or(0),
            pct(&total, 50),
            pct(&total, 90),
        );
    }
    let n = calls.len().max(1) as f64;
    let avg_in = calls.iter().map(|c| c.input).sum::<u64>() as f64 / n;
    let avg_out = calls.iter().map(|c| c.output + c.thoughts).sum::<u64>() as f64 / n;
    let thoughts: u64 = calls.iter().map(|c| c.thoughts).sum();
    let per_call = avg_in * price_in / 1e6 + avg_out * price_out / 1e6;
    println!(
        "  평균 토큰/호출: 입력 {avg_in:.0}, 출력(생각 포함) {avg_out:.0}  · 생각 토큰 합계 {thoughts}"
    );
    println!("  호출당 비용 ≈ ${per_call:.6} (입력 ${price_in}/1M, 출력 ${price_out}/1M 가정)");
    for per_min in [20.0, 40.0, 80.0] {
        println!(
            "  분당 {per_min:>2.0}회 호출 시 번역 비용 ≈ ${:.4}/분",
            per_call * per_min
        );
    }
}

async fn translate_once(
    client: &reqwest::Client,
    key: &str,
    model: &str,
    thinking: &str,
    s: &Sample,
    current: &str,
    is_final: bool,
) -> Call {
    let system = SYSTEM.replace("{src}", s.src).replace("{tgt}", s.tgt);
    let user = format!(
        "Previous lines:\n{} => {}\n\nCurrent utterance ({}):\n{}",
        s.context.0,
        s.context.1,
        if is_final { "complete" } else { "in progress" },
        current
    );
    let mut generation = json!({ "temperature": 0.2, "maxOutputTokens": 256 });
    if thinking != "none" {
        generation["thinkingConfig"] = json!({ "thinkingLevel": thinking });
    }
    let body = json!({
        "systemInstruction": { "parts": [{ "text": system }] },
        "contents": [{ "role": "user", "parts": [{ "text": user }] }],
        "generationConfig": generation,
    });

    let started = Instant::now();
    let res = client
        .post(format!(
            "{API}/models/{model}:streamGenerateContent?alt=sse"
        ))
        .header("x-goog-api-key", key)
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|e| fail(&format!("request failed: {e}")));
    let mut call = Call {
        is_final,
        ttft: None,
        total: Duration::ZERO,
        input: 0,
        output: 0,
        thoughts: 0,
        text: String::new(),
        error: None,
    };
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or(text);
        call.error = Some(format!("HTTP {status}: {}", truncate(&message, 120)));
        call.total = started.elapsed();
        return call;
    }
    let mut buf = String::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap_or_else(|e| fail(&format!("stream error: {e}")));
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buf.find("\n\n").or_else(|| buf.find("\r\n\r\n")) {
            let event: String = buf.drain(..end).collect();
            buf = buf.trim_start_matches(['\r', '\n']).to_owned();
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
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        if !t.is_empty() && call.ttft.is_none() {
                            call.ttft = Some(started.elapsed());
                        }
                        call.text.push_str(t);
                    }
                }
                if let Some(u) = v.get("usageMetadata") {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    call.input = n("promptTokenCount");
                    call.output = n("candidatesTokenCount");
                    call.thoughts = n("thoughtsTokenCount");
                }
            }
        }
    }
    call.total = started.elapsed();
    call
}

fn prices(model: &str) -> (f64, f64) {
    // 2026-10 Gemini API 표준 등급 가격(USD / 1M tokens). 모르는 모델이면 Flash-Lite 기준.
    if model.contains("flash-lite") {
        (0.30, 2.50)
    } else if model.contains("flash") {
        (0.75, 3.75)
    } else {
        (0.30, 2.50)
    }
}

// ───────────────────────── transcribe ─────────────────────────

async fn transcribe(key: &str, model: &str, file: &str, lang: &str) {
    let pcm = read_wav_pcm16k(file);
    let speech = Duration::from_millis((pcm.len() / 32) as u64); // 16kHz * 2bytes = 32 bytes/ms
    // 말이 끝났음을 VAD가 알아차리도록 뒤에 무음 2초를 붙인다.
    let mut audio = pcm;
    audio.extend(std::iter::repeat_n(0u8, 32 * 2000));

    let url = format!("{LIVE_WS}?key={key}");
    let connect_started = Instant::now();
    let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .unwrap_or_else(|e| fail(&format!("websocket connect failed: {e}")));
    let (mut tx, mut rx) = ws.split();
    let langs: Vec<&str> = if lang.is_empty() { vec![] } else { vec![lang] };
    let setup = json!({ "setup": {
        "model": format!("models/{model}"),
        "generationConfig": { "responseModalities": ["TEXT"] },
        "inputAudioTranscription": { "languageCodes": langs },
    }});
    tx.send(Message::Text(setup.to_string().into()))
        .await
        .unwrap_or_else(|e| fail(&format!("send setup: {e}")));

    // setupComplete(또는 첫 메시지)까지 기다린다.
    match tokio::time::timeout(Duration::from_secs(10), rx.next()).await {
        Ok(Some(Ok(msg))) => {
            let v = parse_ws(&msg);
            if let Some(reason) = v.get("close") {
                fail(&format!("setup rejected: {reason}"));
            }
            if v.get("setupComplete").is_none() {
                println!(
                    "첫 메시지가 setupComplete가 아님: {}",
                    truncate(&v.to_string(), 400)
                );
            }
        }
        Ok(Some(Err(e))) => fail(&format!("websocket error: {e}")),
        Ok(None) => fail("connection closed during setup (모델 이름·권한을 확인하세요)"),
        Err(_) => fail("setup timeout"),
    }
    println!(
        "model={model} file={file} 발화 {:.1}s · 연결+설정 {}ms\n",
        speech.as_secs_f64(),
        connect_started.elapsed().as_millis()
    );

    let started = Instant::now();
    let sender = tokio::spawn(async move {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        for chunk in audio.chunks(3200) {
            tick.tick().await;
            let msg = json!({ "realtimeInput": { "audio": { "data": b64.encode(chunk), "mimeType": "audio/pcm;rate=16000" } } });
            if tx
                .send(Message::Text(msg.to_string().into()))
                .await
                .is_err()
            {
                return tx;
            }
        }
        let _ = tx
            .send(Message::Text(
                json!({ "realtimeInput": { "audioStreamEnd": true } })
                    .to_string()
                    .into(),
            ))
            .await;
        tx
    });

    let mut interims: Vec<(Duration, String)> = Vec::new();
    let mut finals: Vec<(Duration, String)> = Vec::new();
    let deadline = speech + Duration::from_secs(12);
    loop {
        let left = deadline.saturating_sub(started.elapsed());
        let msg = match tokio::time::timeout(left, rx.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => {
                println!("websocket error: {e}");
                break;
            }
            Ok(None) | Err(_) => break,
        };
        let at = started.elapsed();
        let v = parse_ws(&msg);
        if let Some(t) = v
            .pointer("/serverContent/interimInputTranscription/text")
            .and_then(Value::as_str)
        {
            println!("  {:>6}ms interim  {}", at.as_millis(), truncate(t, 70));
            interims.push((at, t.to_owned()));
        }
        if let Some(t) = v
            .pointer("/serverContent/inputTranscription/text")
            .and_then(Value::as_str)
        {
            println!("  {:>6}ms FINAL    {}", at.as_millis(), truncate(t, 70));
            finals.push((at, t.to_owned()));
        }
        if let Some(u) = v.get("usageMetadata") {
            println!("  {:>6}ms usage    {}", at.as_millis(), u);
        }
        if let Some(g) = v.get("goAway") {
            println!("  {:>6}ms goAway   {}", at.as_millis(), g);
        }
        let known = ["serverContent", "usageMetadata", "goAway", "setupComplete"];
        if v.as_object()
            .is_some_and(|o| !o.keys().any(|k| known.contains(&k.as_str())))
        {
            println!(
                "  {:>6}ms other    {}",
                at.as_millis(),
                truncate(&v.to_string(), 200)
            );
        }
    }
    let _ = sender.await;

    println!("\n== 요약");
    println!(
        "  발화 길이 {}ms (오디오 송신 시작 기준)",
        speech.as_millis()
    );
    if let Some((t, _)) = interims.first() {
        println!("  첫 중간 결과: {}ms", t.as_millis());
    }
    let gaps: Vec<u128> = interims
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).as_millis())
        .collect();
    println!(
        "  중간 결과 {}개 · 간격 p50 {}ms p90 {}ms",
        interims.len(),
        pct(&gaps, 50),
        pct(&gaps, 90)
    );
    let cumulative = interims
        .windows(2)
        .filter(|w| w[1].1.starts_with(w[0].1.trim_end()))
        .count();
    println!(
        "  누적형(앞 결과로 시작) {}/{} → {}",
        cumulative,
        interims.len().saturating_sub(1),
        if cumulative * 2 >= interims.len().saturating_sub(1) {
            "주로 전체 문장을 다시 보냄"
        } else {
            "주로 조각(증분)으로 보냄"
        }
    );
    match finals.last() {
        Some((t, _)) => {
            let after = t.as_millis() as i128 - speech.as_millis() as i128;
            println!(
                "  확정 결과 {}개 · 마지막 확정: 말 끝난 뒤 {after}ms",
                finals.len()
            );
            println!(
                "  확정 문장: {}",
                truncate(
                    &finals
                        .iter()
                        .map(|f| f.1.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    200
                )
            );
        }
        None => println!("  확정 결과 없음(시간 안에 오지 않음)"),
    }
}

fn parse_ws(msg: &Message) -> Value {
    // Live API는 JSON을 텍스트 또는 바이너리 프레임으로 보낸다.
    match msg {
        Message::Text(t) => serde_json::from_str(t).unwrap_or(Value::Null),
        Message::Binary(b) => serde_json::from_slice(b).unwrap_or(Value::Null),
        Message::Close(frame) => json!({ "close": frame.as_ref().map(|f| f.reason.to_string()) }),
        _ => Value::Null,
    }
}

/// 16kHz mono 16-bit WAV에서 PCM 데이터 부분만 꺼낸다.
fn read_wav_pcm16k(path: &str) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        fail(&format!(
            "{path}: {e} (make-fixtures.sh 를 먼저 실행하세요)"
        ))
    });
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let size = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        if id == b"fmt " {
            let channels = u16::from_le_bytes(bytes[i + 10..i + 12].try_into().unwrap());
            let rate = u32::from_le_bytes(bytes[i + 12..i + 16].try_into().unwrap());
            let bits = u16::from_le_bytes(bytes[i + 22..i + 24].try_into().unwrap());
            if (channels, rate, bits) != (1, 16000, 16) {
                fail(&format!(
                    "{path}: 16kHz mono 16-bit WAV가 필요합니다 (got {channels}ch {rate}Hz {bits}bit)"
                ));
            }
        }
        if id == b"data" {
            return bytes[i + 8..(i + 8 + size).min(bytes.len())].to_vec();
        }
        i += 8 + size + (size & 1);
    }
    fail(&format!("{path}: data chunk not found"))
}

// ───────────────────────── helpers ─────────────────────────

fn pct(values: &[u128], p: usize) -> u128 {
    if values.is_empty() {
        return 0;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    v[((v.len() - 1) * p).div_ceil(100)]
}

fn truncate(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out.replace('\n', " ")
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}
