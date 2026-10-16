//! "말 → 오디오를 아예 보내지 않는 침묵 → 말" 시나리오.
//!
//! 브라우저 VAD가 침묵 구간을 보내지 않을 때(과금 절감) Live 세션이 버티는지,
//! 두 번째 발화도 정상 인식되는지, 확정 결과가 언제 오는지를 잰다.
//! - 자동 모드: 서버 VAD에 맡긴다. 클라이언트 VAD처럼 말 뒤에 600ms만 무음을 보내고 멈춘다.
//! - 수동 모드(`--manual`): 서버 VAD를 끄고 activityStart / activityEnd 신호로 발화 구간을 직접 알린다.

use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::{LIVE_WS, fail, parse_ws, read_wav_pcm16k, truncate};

/// 클라이언트 VAD가 말이 끝난 뒤에도 잠깐 더 보내는 시간(문장 중간 쉼에서 끊기지 않게)
const HANGOVER: Duration = Duration::from_millis(600);

pub struct Options<'a> {
    pub model: &'a str,
    pub first: &'a str,
    pub second: &'a str,
    pub lang: &'a str,
    pub gap: Duration,
    pub tail: Duration,
    pub manual: bool,
    /// 침묵 중 이 간격마다 100ms 무음을 보내 세션을 살려 둔다(없으면 0). 서버는 입력이 60초 없으면 끊는다.
    pub keepalive: Duration,
}

pub async fn run(key: &str, o: Options<'_>) {
    let speech1 = read_wav_pcm16k(o.first);
    let speech2 = read_wav_pcm16k(o.second);
    let dur = |pcm: &[u8]| Duration::from_millis((pcm.len() / 32) as u64);

    let (ws, _) = tokio_tungstenite::connect_async(format!("{LIVE_WS}?key={key}").as_str())
        .await
        .unwrap_or_else(|e| fail(&format!("websocket connect failed: {e}")));
    let (mut tx, mut rx) = ws.split();
    // 쉼표로 여러 언어를 줄 수 있다(예: ko,ja). 비우면 언어를 지정하지 않는다.
    let langs: Vec<&str> = o.lang.split(',').filter(|l| !l.is_empty()).collect();
    let mut setup = json!({ "setup": {
        "model": format!("models/{}", o.model),
        "generationConfig": { "responseModalities": ["TEXT"] },
        "inputAudioTranscription": { "languageCodes": langs },
    }});
    if o.manual {
        setup["setup"]["realtimeInputConfig"] =
            json!({ "automaticActivityDetection": { "disabled": true } });
    }
    send(&mut tx, setup).await;
    match tokio::time::timeout(Duration::from_secs(10), rx.next()).await {
        Ok(Some(Ok(msg))) => {
            let v = parse_ws(&msg);
            if let Some(reason) = v.get("close") {
                fail(&format!("setup rejected: {reason}"));
            }
        }
        _ => fail("setup failed or timed out"),
    }

    let started = Instant::now();
    // 수신은 별도 작업에서 시각과 함께 모은다.
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<(Duration, Value)>();
    let reader = tokio::spawn(async move {
        while let Some(msg) = rx.next().await {
            let at = started.elapsed();
            let v = match msg {
                Ok(m) => parse_ws(&m),
                Err(e) => json!({ "close": format!("error: {e}") }),
            };
            print_event(at, &v);
            let closed = v.get("close").is_some();
            let _ = events_tx.send((at, v));
            if closed {
                break;
            }
        }
    });

    println!(
        "model={} mode={} 발화1 {:.1}s → 보내지 않는 침묵 {}s → 발화2 {:.1}s\n",
        o.model,
        if o.manual {
            "manual(activityStart/End)"
        } else {
            "auto(server VAD)"
        },
        dur(&speech1).as_secs_f64(),
        o.gap.as_secs(),
        dur(&speech2).as_secs_f64()
    );

    let mut sent_audio = Duration::ZERO;
    let speak = |n: u8| println!("  {:>6}ms ── 발화{n} 시작", started.elapsed().as_millis());
    speak(1);
    let start1 = started.elapsed();
    sent_audio += utterance(&mut tx, &speech1, o.manual).await;
    let end1 = start1 + dur(&speech1);
    println!(
        "  {:>6}ms ── 침묵 시작(아무것도 보내지 않음)",
        started.elapsed().as_millis()
    );
    let mut keepalive_sent = 0u32;
    if o.keepalive.is_zero() {
        tokio::time::sleep(o.gap).await;
    } else {
        let gap_end = Instant::now() + o.gap;
        let silence = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 3200]);
        loop {
            let next = Instant::now() + o.keepalive;
            if next >= gap_end {
                tokio::time::sleep_until(gap_end.into()).await;
                break;
            }
            tokio::time::sleep_until(next.into()).await;
            let msg = json!({ "realtimeInput": { "audio": { "data": silence, "mimeType": "audio/pcm;rate=16000" } } });
            if !send(&mut tx, msg).await {
                break;
            }
            keepalive_sent += 1;
            sent_audio += Duration::from_millis(100);
        }
        println!(
            "  {:>6}ms ── 침묵 중 keepalive {keepalive_sent}회 보냄",
            started.elapsed().as_millis()
        );
    }
    speak(2);
    let start2 = started.elapsed();
    sent_audio += utterance(&mut tx, &speech2, o.manual).await;
    let end2 = start2 + dur(&speech2);
    tokio::time::sleep(o.tail).await;
    let _ = tx.close().await;
    reader.abort();

    let mut events = Vec::new();
    while let Ok(e) = events_rx.try_recv() {
        events.push(e);
    }
    let finals = |from: Duration, to: Duration| -> Vec<(Duration, String)> {
        events
            .iter()
            .filter(|(at, _)| *at >= from && *at < to)
            .filter_map(|(at, v)| {
                v.pointer("/serverContent/inputTranscription/text")
                    .and_then(Value::as_str)
                    .map(|t| (*at, t.to_owned()))
            })
            .collect()
    };
    let closed_at = events
        .iter()
        .find(|(_, v)| v.get("close").is_some())
        .map(|(at, v)| (*at, v["close"].clone()));
    let go_away = events.iter().any(|(_, v)| v.get("goAway").is_some());

    println!("\n== 요약");
    match &closed_at {
        Some((at, why)) => println!("  세션 종료: {}ms ({why})", at.as_millis()),
        None => println!("  세션 유지: 끝까지 끊기지 않음"),
    }
    println!("  goAway 수신: {}", if go_away { "예" } else { "아니오" });
    for (label, from, to, end) in [
        ("발화1", start1, start2, end1),
        ("발화2", start2, Duration::MAX, end2),
    ] {
        let f = finals(from, to);
        match f.last() {
            Some((at, _)) => println!(
                "  {label}: 확정 {}개 · 마지막 확정은 말 끝난 뒤 {}ms · {}",
                f.len(),
                at.as_millis() as i128 - end.as_millis() as i128,
                truncate(
                    &f.iter().map(|x| x.1.as_str()).collect::<Vec<_>>().join(" "),
                    80
                )
            ),
            None => println!("  {label}: 확정 결과 없음"),
        }
    }
    println!(
        "  실제로 보낸 오디오 {:.1}s / 경과 시간 {:.1}s → 침묵을 안 보내서 {:.0}% 절감",
        sent_audio.as_secs_f64(),
        started.elapsed().as_secs_f64(),
        100.0 * (1.0 - sent_audio.as_secs_f64() / started.elapsed().as_secs_f64())
    );
}

/// 한 발화를 실시간 속도로 보낸다. 보낸 오디오 길이를 돌려준다.
async fn utterance<S>(tx: &mut S, pcm: &[u8], manual: bool) -> Duration
where
    S: SinkExt<Message> + Unpin,
{
    if manual && !send(tx, json!({ "realtimeInput": { "activityStart": {} } })).await {
        return Duration::ZERO;
    }
    let mut audio = pcm.to_vec();
    if !manual {
        audio.extend(std::iter::repeat_n(0u8, 32 * HANGOVER.as_millis() as usize));
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    for chunk in audio.chunks(3200) {
        tick.tick().await;
        let msg = json!({ "realtimeInput": { "audio": { "data": b64.encode(chunk), "mimeType": "audio/pcm;rate=16000" } } });
        if !send(tx, msg).await {
            return Duration::ZERO;
        }
    }
    if manual {
        send(tx, json!({ "realtimeInput": { "activityEnd": {} } })).await;
    }
    Duration::from_millis((audio.len() / 32) as u64)
}

/// 보냈으면 true. 연결이 끊겼으면 한 번만 알리고 false.
async fn send<S>(tx: &mut S, v: Value) -> bool
where
    S: SinkExt<Message> + Unpin,
{
    let ok = tx.send(Message::Text(v.to_string().into())).await.is_ok();
    if !ok {
        println!("  (send failed: connection closed)");
    }
    ok
}

fn print_event(at: Duration, v: &Value) {
    let ms = at.as_millis();
    if let Some(t) = v
        .pointer("/serverContent/interimInputTranscription/text")
        .and_then(Value::as_str)
    {
        println!("  {ms:>6}ms interim  {}", truncate(t, 60));
    }
    if let Some(t) = v
        .pointer("/serverContent/inputTranscription/text")
        .and_then(Value::as_str)
    {
        println!("  {ms:>6}ms FINAL    {}", truncate(t, 60));
    }
    if v.pointer("/serverContent/turnComplete")
        .and_then(Value::as_bool)
        == Some(true)
    {
        println!("  {ms:>6}ms turnComplete");
    }
    if let Some(u) = v.get("usageMetadata") {
        println!("  {ms:>6}ms usage    {u}");
    }
    if let Some(g) = v.get("goAway") {
        println!("  {ms:>6}ms goAway   {g}");
    }
    if let Some(c) = v.get("close") {
        println!("  {ms:>6}ms CLOSED   {c}");
    }
}
