use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, mpsc};

use super::*;
use crate::protocol::{MediaState, RoomSettings};
use crate::providers::testing::Fake;
use crate::room::{JoinOutcome, JoinRequest, Outbox};

const HOST: &str = "alice";

struct Room {
    rooms: Arc<Rooms>,
    code: String,
    host: PeerId,
}

type Inbox = mpsc::Receiver<ServerMsg>;

fn join_req(name: &str, lang: &str, partner: &str) -> JoinRequest {
    JoinRequest {
        name: name.into(),
        lang: lang.into(),
        partner_lang: partner.into(),
        media: MediaState::default(),
    }
}

/// 주최자(화자, ko→ja)와 게스트들이 입장한 방
fn room(guest_langs: &[&str]) -> (Room, Inbox, Vec<Inbox>) {
    let rooms = Arc::new(Rooms::default());
    let code = rooms
        .create(HOST, RoomSettings { translation: true })
        .unwrap();
    let (tx, mut host_rx) = mpsc::channel(256);
    let tx = Outbox::new(tx, Arc::new(Notify::new()));
    let JoinOutcome::Joined(host) = rooms
        .join(&code, join_req("A", "ko", "ja"), Some(HOST), tx)
        .unwrap()
    else {
        panic!("host joins directly")
    };
    let mut guests = Vec::new();
    for (i, lang) in guest_langs.iter().enumerate() {
        let (tx, mut rx) = mpsc::channel(256);
        let tx = Outbox::new(tx, Arc::new(Notify::new()));
        let JoinOutcome::Waiting(id) = rooms
            .join(&code, join_req(&format!("G{i}"), lang, "ko"), None, tx)
            .unwrap()
        else {
            panic!("guest waits")
        };
        rooms.admit(&code, &host, &id).unwrap();
        while rx.try_recv().is_ok() {}
        guests.push(rx);
    }
    while host_rx.try_recv().is_ok() {}
    (Room { rooms, code, host }, host_rx, guests)
}

fn pipeline(room: &Room, fake: Fake) -> (Arc<Pipeline>, Arc<Fake>) {
    let fake = Arc::new(fake);
    let p = Arc::new(Pipeline::new(room.rooms.clone(), Some(fake.clone())));
    (p, fake)
}

#[derive(Debug, PartialEq)]
struct Cap {
    seg: String,
    src: String,
    tgt_lang: String,
    text: Option<String>,
    is_final: bool,
}

fn captions(rx: &mut Inbox) -> Vec<Cap> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let ServerMsg::Caption {
            seg,
            src,
            tgt_lang,
            text,
            is_final,
            ..
        } = msg
        {
            out.push(Cap {
                seg,
                src,
                tgt_lang,
                text,
                is_final,
            });
        }
    }
    out
}

async fn wait(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[tokio::test(start_paused = true)]
async fn interim_is_shown_then_translated_then_finalized() {
    let (room, mut host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(900)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());
    let seg = format!("{}-0", room.host);

    speaker.push(0, "저희 투자팀은".into(), false);
    wait(10).await;
    let first = captions(&mut guests[0]);
    assert_eq!(
        first,
        [Cap {
            seg: seg.clone(),
            src: "저희 투자팀은".into(),
            tgt_lang: "ja".into(),
            text: None,
            is_final: false,
        }]
    );
    // 화자 자신도 같은 줄을 상대 언어 번역으로 본다.
    assert_eq!(captions(&mut host_rx).len(), 1);

    wait(900).await;
    let translated = captions(&mut guests[0]);
    assert_eq!(
        translated.last().unwrap().text.as_deref(),
        Some("[ja] 저희 투자팀은")
    );
    assert!(!translated.last().unwrap().is_final);

    speaker.push(0, "저희 투자팀은 AI에 투자합니다.".into(), true);
    wait(10).await;
    let locked = captions(&mut guests[0]);
    // 확정 번역이 오기 전까지는 이전 번역을 보여주며 확정되지 않은 상태다.
    assert_eq!(locked.last().unwrap().src, "저희 투자팀은 AI에 투자합니다.");
    assert!(!locked.last().unwrap().is_final);

    wait(900).await;
    let done = captions(&mut guests[0]);
    assert_eq!(
        done.last().unwrap(),
        &Cap {
            seg,
            src: "저희 투자팀은 AI에 투자합니다.".into(),
            tgt_lang: "ja".into(),
            text: Some("[ja] 저희 투자팀은 AI에 투자합니다.".into()),
            is_final: true,
        }
    );
    assert_eq!(fake.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn interims_during_a_translation_are_coalesced_to_the_latest() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(1000)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "하나".into(), false);
    wait(300).await;
    speaker.push(0, "하나 둘".into(), false);
    wait(300).await;
    speaker.push(0, "하나 둘 셋".into(), false);
    wait(300).await;
    assert_eq!(fake.calls(), 1, "no new request while one is in flight");

    wait(200).await; // 첫 요청 완료(1000ms) → 최신 텍스트로 바로 다음 요청
    assert_eq!(fake.calls(), 2);
    assert_eq!(fake.requests.lock().unwrap()[1].text, "하나 둘 셋");
    assert!(!fake.requests.lock().unwrap()[1].complete);

    let caps = captions(&mut guests[0]);
    let last = caps.last().unwrap();
    // 번역은 이전 원문의 것이지만 원문은 최신이다.
    assert_eq!(last.src, "하나 둘 셋");
    assert_eq!(last.text.as_deref(), Some("[ja] 하나"));
}

#[tokio::test(start_paused = true)]
async fn locking_cancels_the_in_progress_request() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(1000)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "네".into(), false);
    wait(100).await;
    speaker.push(0, "네.".into(), true);
    wait(10).await;
    assert_eq!(fake.calls(), 2, "final request starts immediately");
    assert!(fake.requests.lock().unwrap()[1].complete);

    wait(1000).await;
    let caps = captions(&mut guests[0]);
    assert!(caps.last().unwrap().is_final);
    assert_eq!(caps.iter().filter(|c| c.is_final).count(), 1);
}

#[tokio::test(start_paused = true)]
async fn finals_split_into_sentences_and_carry_context() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "안녕하세요. 저는".into(), true);
    wait(200).await;
    speaker.push(1, "유승재입니다.".into(), true);
    wait(200).await;

    let finals: Vec<Cap> = captions(&mut guests[0])
        .into_iter()
        .filter(|c| c.is_final)
        .collect();
    assert_eq!(finals.len(), 2);
    assert_eq!(finals[0].src, "안녕하세요.");
    assert_eq!(finals[1].src, "저는 유승재입니다.");
    assert_ne!(finals[0].seg, finals[1].seg);

    let reqs = fake.requests.lock().unwrap();
    let last = reqs.last().unwrap();
    assert_eq!(last.text, "저는 유승재입니다.");
    assert_eq!(
        last.context,
        [("안녕하세요.".to_owned(), "[ja] 안녕하세요.".to_owned())]
    );
}

#[tokio::test(start_paused = true)]
async fn unfinished_final_is_locked_after_silence() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "주로 AI에".into(), true);
    wait(1500).await;
    assert!(!captions(&mut guests[0]).iter().any(|c| c.is_final));
    wait(700).await;
    let caps = captions(&mut guests[0]);
    let last = caps.last().unwrap();
    assert!(last.is_final);
    assert_eq!(last.src, "주로 AI에");
}

#[tokio::test(start_paused = true)]
async fn missing_final_is_recovered_from_next_segment() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let (p, _fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "주로".into(), false);
    wait(10).await;
    // 구간 0의 확정 결과 없이 구간 1이 시작됨
    speaker.push(1, "AI에 투자합니다.".into(), true);
    wait(200).await;
    let caps = captions(&mut guests[0]);
    let last = caps.last().unwrap();
    assert!(last.is_final);
    assert_eq!(last.src, "주로 AI에 투자합니다.");
    // 늦게 온 구간 0 메시지는 무시한다.
    speaker.push(0, "주로 무시".into(), false);
    wait(10).await;
    assert!(captions(&mut guests[0]).is_empty());
}

#[tokio::test(start_paused = true)]
async fn each_target_language_is_translated_once() {
    // 화자 ko→ja, 독자: ja, ja, en
    let (room, mut host_rx, mut guests) = room(&["ja", "ja", "en"]);
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "좋아요.".into(), true);
    wait(200).await;
    assert_eq!(fake.calls(), 2);
    for (rx, lang) in guests.iter_mut().zip(["ja", "ja", "en"]) {
        let last = captions(rx).pop().unwrap();
        assert_eq!(last.tgt_lang, lang);
        assert_eq!(last.text, Some(format!("[{lang}] 좋아요.")));
        assert!(last.is_final);
    }
    assert_eq!(captions(&mut host_rx).pop().unwrap().tgt_lang, "ja");
}

#[tokio::test(start_paused = true)]
async fn failed_translation_finalizes_original_and_notifies() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    let mut fake = Fake::new(vec![ms(100)]);
    fake.fail = true;
    let (p, _fake) = pipeline(&room, fake);
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "좋아요.".into(), true);
    wait(200).await;
    let mut notices = 0;
    let mut last = None;
    while let Ok(msg) = guests[0].try_recv() {
        match msg {
            ServerMsg::Notice { code } => {
                assert_eq!(code, NoticeCode::TranslationFailed);
                notices += 1;
            }
            ServerMsg::Caption { text, is_final, .. } => last = Some((text, is_final)),
            _ => {}
        }
    }
    assert_eq!(notices, 1);
    assert_eq!(last, Some((None, true)));
}

#[tokio::test(start_paused = true)]
async fn nothing_is_sent_when_translation_is_off() {
    let (room, _host_rx, mut guests) = room(&["ja"]);
    room.rooms
        .set_settings(&room.code, &room.host, RoomSettings { translation: false })
        .unwrap();
    while guests[0].try_recv().is_ok() {}
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "좋아요.".into(), true);
    wait(200).await;
    assert!(captions(&mut guests[0]).is_empty());
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn same_language_reader_gets_original_only() {
    // 화자 ko→ko(상대 언어를 같게 둠), 독자 ko
    let (room, _host_rx, mut guests) = room(&["ko"]);
    room.rooms
        .update(&room.code, &room.host, |p| p.partner_lang = "ko".into());
    while guests[0].try_recv().is_ok() {}
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), room.host.clone());

    speaker.push(0, "좋아요.".into(), true);
    wait(10).await;
    let last = captions(&mut guests[0]).pop().unwrap();
    assert_eq!(last.text, None);
    assert!(last.is_final);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn speech_while_waiting_is_never_captioned() {
    let (room, _host_rx, _guests) = room(&[]);
    let (tx, mut guest_rx) = mpsc::channel(256);
    let tx = Outbox::new(tx, Arc::new(Notify::new()));
    let JoinOutcome::Waiting(guest) = room
        .rooms
        .join(&room.code, join_req("B", "ja", "ko"), None, tx)
        .unwrap()
    else {
        panic!("guest waits")
    };
    let (p, fake) = pipeline(&room, Fake::new(vec![ms(100)]));
    let speaker = p.speaker(room.code.clone(), guest.clone());

    speaker.push(0, "まだ".into(), true);
    wait(10).await;
    room.rooms.admit(&room.code, &room.host, &guest).unwrap();
    wait(3000).await;
    assert!(captions(&mut guest_rx).is_empty());
    assert_eq!(fake.calls(), 0);

    // 입장 후에는 정상 동작
    speaker.push(1, "はい。".into(), true);
    wait(200).await;
    assert!(captions(&mut guest_rx).last().unwrap().is_final);
}
