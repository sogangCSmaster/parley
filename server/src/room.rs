//! 방과 참가자 상태. 단일 프로세스 메모리에 둔다(MVP).
//!
//! 권한 규칙
//! - 방은 로그인한 주최자만 만든다(`create`).
//! - 주최자 계정으로 들어온 참가자는 바로 입장한다. 그 외(게스트)는 대기열에 들어가고,
//!   주최자가 수락해야 입장한다.
//! - 수락·거절·설정 변경은 그 방의 주최자 참가자만 할 수 있다. 모든 검사는 이 모듈에서 한다.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rand::RngExt;
use tokio::sync::mpsc;

use crate::protocol::{ErrorCode, Knock, MediaState, Peer, PeerId, RoomSettings, ServerMsg};

/// P2P mesh는 인원이 늘면 업링크가 (n-1)배가 되므로 4명으로 제한한다.
pub const MAX_PEERS: usize = 4;
/// 대기열 스팸 방지
pub const MAX_WAITING: usize = 8;
/// 주최자 한 명이 동시에 가질 수 있는 방
pub const MAX_ROOMS_PER_HOST: usize = 20;
/// 아무도 없는 방(만들고 아무도 안 들어왔거나 모두 나간 방)을 지우기까지의 시간
pub const ROOM_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// 혼동하기 쉬운 문자(l, o, 0, 1)를 뺀 32자. 4-4-4 형식이면 60bit.
const CODE_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";

pub type Outbox = mpsc::Sender<ServerMsg>;

struct Member {
    peer: Peer,
    tx: Outbox,
}

struct Room {
    host_account: String,
    settings: RoomSettings,
    members: HashMap<PeerId, Member>,
    waiting: HashMap<PeerId, Member>,
    /// 방이 비게 된 시점(생성 직후 포함). 사람이 있으면 None.
    idle_since: Option<Instant>,
}

impl Room {
    fn is_host_member(&self, id: &str) -> bool {
        self.members.get(id).is_some_and(|m| m.peer.host)
    }

    fn notify_hosts(&self, msg: ServerMsg) {
        for m in self.members.values().filter(|m| m.peer.host) {
            send(&m.tx, msg.clone());
        }
    }

    fn knocks(&self) -> Vec<Knock> {
        self.waiting
            .values()
            .map(|w| Knock {
                id: w.peer.id.clone(),
                name: w.peer.name.clone(),
            })
            .collect()
    }
}

#[derive(Default)]
pub struct Rooms {
    rooms: Mutex<HashMap<String, Room>>,
}

pub struct JoinRequest {
    pub name: String,
    pub lang: String,
    pub partner_lang: String,
    pub media: MediaState,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JoinOutcome {
    /// 입장 완료. Welcome은 이미 보냈다.
    Joined(PeerId),
    /// 주최자 수락 대기. Waiting은 이미 보냈다.
    Waiting(PeerId),
}

impl Rooms {
    pub fn create(&self, host_account: &str, settings: RoomSettings) -> Result<String, ErrorCode> {
        let mut rooms = self.rooms.lock().unwrap();
        if rooms
            .values()
            .filter(|r| r.host_account == host_account)
            .count()
            >= MAX_ROOMS_PER_HOST
        {
            return Err(ErrorCode::Forbidden);
        }
        let code = loop {
            let code = new_room_code();
            if !rooms.contains_key(&code) {
                break code;
            }
        };
        let room = Room {
            host_account: host_account.to_owned(),
            settings,
            members: HashMap::new(),
            waiting: HashMap::new(),
            idle_since: Some(Instant::now()),
        };
        rooms.insert(code.clone(), room);
        Ok(code)
    }

    pub fn settings(&self, code: &str) -> Option<RoomSettings> {
        self.rooms.lock().unwrap().get(code).map(|r| r.settings)
    }

    /// `account`는 요청자의 로그인 계정(없으면 게스트). 방 주최자 계정과 같을 때만 주최자로 입장한다.
    pub fn join(
        &self,
        code: &str,
        req: JoinRequest,
        account: Option<&str>,
        tx: Outbox,
    ) -> Result<JoinOutcome, ErrorCode> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        if room.members.len() >= MAX_PEERS {
            return Err(ErrorCode::RoomFull);
        }
        let is_host = account == Some(room.host_account.as_str());
        if !is_host && room.waiting.len() >= MAX_WAITING {
            return Err(ErrorCode::TooManyWaiting);
        }
        let id = random_token(12);
        let peer = Peer {
            id: id.clone(),
            name: req.name,
            lang: req.lang,
            partner_lang: req.partner_lang,
            media: req.media,
            host: is_host,
        };
        room.idle_since = None;

        if is_host {
            let welcome = ServerMsg::Welcome {
                you: id.clone(),
                host: true,
                settings: room.settings,
                peers: room.members.values().map(|m| m.peer.clone()).collect(),
                knocks: room.knocks(),
            };
            send(&tx, welcome);
            broadcast(
                &room.members,
                &id,
                ServerMsg::PeerJoined { peer: peer.clone() },
            );
            room.members.insert(id.clone(), Member { peer, tx });
            return Ok(JoinOutcome::Joined(id));
        }

        send(&tx, ServerMsg::Waiting);
        room.notify_hosts(ServerMsg::Knock {
            knock: Knock {
                id: id.clone(),
                name: peer.name.clone(),
            },
        });
        room.waiting.insert(id.clone(), Member { peer, tx });
        Ok(JoinOutcome::Waiting(id))
    }

    pub fn admit(&self, code: &str, by: &str, guest: &str) -> Result<(), ErrorCode> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        if !room.is_host_member(by) {
            return Err(ErrorCode::Forbidden);
        }
        if !room.waiting.contains_key(guest) {
            return Err(ErrorCode::UnknownPeer);
        }
        // 가득 찬 상태에서 수락하면 게스트는 대기열에 그대로 남는다(자리가 나면 다시 수락 가능).
        if room.members.len() >= MAX_PEERS {
            return Err(ErrorCode::RoomFull);
        }
        let member = room.waiting.remove(guest).expect("checked above");
        let welcome = ServerMsg::Welcome {
            you: guest.to_owned(),
            host: false,
            settings: room.settings,
            peers: room.members.values().map(|m| m.peer.clone()).collect(),
            knocks: Vec::new(),
        };
        send(&member.tx, welcome);
        broadcast(
            &room.members,
            guest,
            ServerMsg::PeerJoined {
                peer: member.peer.clone(),
            },
        );
        room.members.insert(guest.to_owned(), member);
        room.notify_hosts(ServerMsg::KnockResolved {
            id: guest.to_owned(),
        });
        Ok(())
    }

    pub fn deny(&self, code: &str, by: &str, guest: &str) -> Result<(), ErrorCode> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        if !room.is_host_member(by) {
            return Err(ErrorCode::Forbidden);
        }
        let member = room.waiting.remove(guest).ok_or(ErrorCode::UnknownPeer)?;
        send(&member.tx, ServerMsg::Denied);
        room.notify_hosts(ServerMsg::KnockResolved {
            id: guest.to_owned(),
        });
        Ok(())
    }

    pub fn set_settings(
        &self,
        code: &str,
        by: &str,
        settings: RoomSettings,
    ) -> Result<(), ErrorCode> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        if !room.is_host_member(by) {
            return Err(ErrorCode::Forbidden);
        }
        room.settings = settings;
        for m in room.members.values() {
            send(&m.tx, ServerMsg::Settings { settings });
        }
        Ok(())
    }

    /// 입장한 참가자만 자기 정보를 바꿀 수 있다(대기 중인 게스트는 무시).
    pub fn update(&self, code: &str, id: &str, f: impl FnOnce(&mut Peer)) {
        let mut rooms = self.rooms.lock().unwrap();
        let Some(room) = rooms.get_mut(code) else {
            return;
        };
        let Some(member) = room.members.get_mut(id) else {
            return;
        };
        f(&mut member.peer);
        let peer = member.peer.clone();
        broadcast(&room.members, id, ServerMsg::PeerUpdated { peer });
    }

    /// 입장한 참가자끼리만 시그널을 중계한다. 대기 중인 게스트·다른 방 참가자는 거부한다.
    pub fn relay_signal(
        &self,
        code: &str,
        from: &str,
        to: &str,
        data: serde_json::Value,
    ) -> Result<(), ErrorCode> {
        let rooms = self.rooms.lock().unwrap();
        let room = rooms.get(code).ok_or(ErrorCode::UnknownPeer)?;
        if !room.members.contains_key(from) || from == to {
            return Err(ErrorCode::UnknownPeer);
        }
        let target = room.members.get(to).ok_or(ErrorCode::UnknownPeer)?;
        send(
            &target.tx,
            ServerMsg::Signal {
                from: from.to_owned(),
                data,
            },
        );
        Ok(())
    }

    pub fn leave(&self, code: &str, id: &str) {
        let mut rooms = self.rooms.lock().unwrap();
        let Some(room) = rooms.get_mut(code) else {
            return;
        };
        if room.members.remove(id).is_some() {
            broadcast(&room.members, id, ServerMsg::PeerLeft { id: id.to_owned() });
        } else if room.waiting.remove(id).is_some() {
            room.notify_hosts(ServerMsg::KnockResolved { id: id.to_owned() });
        }
        if room.members.is_empty() && room.waiting.is_empty() {
            room.idle_since = Some(Instant::now());
        }
    }

    /// 오래 빈 방을 지운다(주기 작업).
    pub fn sweep(&self, now: Instant) {
        self.rooms.lock().unwrap().retain(|_, r| {
            r.idle_since
                .is_none_or(|t| now.duration_since(t) < ROOM_IDLE_TTL)
        });
    }
}

fn broadcast(members: &HashMap<PeerId, Member>, except: &str, msg: ServerMsg) {
    for (id, member) in members {
        if id != except {
            send(&member.tx, msg.clone());
        }
    }
}

fn send(tx: &Outbox, msg: ServerMsg) {
    // 느린 클라이언트 때문에 방 전체가 막히지 않도록 버퍼가 차면 버린다. 연결이 끊기면 leave로 정리된다.
    if let Err(err) = tx.try_send(msg) {
        tracing::warn!(%err, "dropping message to slow or closed client");
    }
}

fn random_token(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// `abcd-efgh-ijkm` 형식의 새 회의 코드.
fn new_room_code() -> String {
    let t = random_token(12);
    format!("{}-{}-{}", &t[0..4], &t[4..8], &t[8..12])
}

pub fn is_valid_room_code(code: &str) -> bool {
    let parts: Vec<&str> = code.split('-').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| p.len() == 4 && p.bytes().all(|b| CODE_ALPHABET.contains(&b)))
}

/// 이름은 앞뒤 공백 제거 후 1~40자, 제어 문자 불가.
pub fn normalize_name(name: &str) -> Option<String> {
    let name = name.trim();
    let len = name.chars().count();
    ((1..=40).contains(&len) && !name.chars().any(char::is_control)).then(|| name.to_owned())
}

/// BCP-47 형태만 확인한다(`ko`, `ja`, `en-US`, `zh-Hant`). 지원 여부는 번역 단계에서 판단.
pub fn is_valid_lang(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let primary_ok = parts
        .next()
        .is_some_and(|p| (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()));
    primary_ok
        && tag.len() <= 35
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HOST: &str = "alice";
    const ON: RoomSettings = RoomSettings { translation: true };

    fn req(name: &str) -> JoinRequest {
        JoinRequest {
            name: name.into(),
            lang: "ko".into(),
            partner_lang: "ja".into(),
            media: MediaState::default(),
        }
    }

    struct Client {
        tx: Outbox,
        rx: mpsc::Receiver<ServerMsg>,
    }

    fn client() -> Client {
        let (tx, rx) = mpsc::channel(32);
        Client { tx, rx }
    }

    impl Client {
        fn drain(&mut self) -> Vec<ServerMsg> {
            std::iter::from_fn(|| self.rx.try_recv().ok()).collect()
        }
    }

    fn joined(outcome: JoinOutcome) -> PeerId {
        match outcome {
            JoinOutcome::Joined(id) => id,
            other => panic!("expected joined, got {other:?}"),
        }
    }

    fn waiting(outcome: JoinOutcome) -> PeerId {
        match outcome {
            JoinOutcome::Waiting(id) => id,
            other => panic!("expected waiting, got {other:?}"),
        }
    }

    /// 방 하나 + 주최자 입장까지 해 둔 상태
    fn room_with_host() -> (Rooms, String, PeerId, Client) {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let mut host = client();
        let host_id = joined(
            rooms
                .join(&code, req("Host"), Some(HOST), host.tx.clone())
                .unwrap(),
        );
        host.drain();
        (rooms, code, host_id, host)
    }

    #[test]
    fn joining_unknown_room_is_rejected() {
        let rooms = Rooms::default();
        let c = client();
        assert_eq!(
            rooms
                .join("abcd-efgh-ijkm", req("A"), Some(HOST), c.tx)
                .unwrap_err(),
            ErrorCode::RoomNotFound
        );
    }

    #[test]
    fn host_account_joins_immediately_and_gets_settings() {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let mut host = client();
        joined(
            rooms
                .join(&code, req("Host"), Some(HOST), host.tx.clone())
                .unwrap(),
        );
        assert!(
            matches!(&host.drain()[..], [ServerMsg::Welcome { host: true, settings, .. }] if settings.translation)
        );
    }

    #[test]
    fn guest_and_other_accounts_wait_and_host_is_notified() {
        let (rooms, code, _host_id, mut host) = room_with_host();

        let mut guest = client();
        let guest_id = waiting(
            rooms
                .join(&code, req("Guest"), None, guest.tx.clone())
                .unwrap(),
        );
        assert!(matches!(&guest.drain()[..], [ServerMsg::Waiting]));
        assert!(matches!(&host.drain()[..], [ServerMsg::Knock { knock }] if knock.id == guest_id));

        // 로그인했더라도 다른 주최자 계정이면 이 방에서는 게스트다.
        let other = client();
        waiting(
            rooms
                .join(&code, req("Bob"), Some("bob"), other.tx)
                .unwrap(),
        );
    }

    #[test]
    fn host_admits_guest() {
        let (rooms, code, host_id, mut host) = room_with_host();
        let mut guest = client();
        let guest_id = waiting(
            rooms
                .join(&code, req("Guest"), None, guest.tx.clone())
                .unwrap(),
        );
        guest.drain();
        host.drain();

        rooms.admit(&code, &host_id, &guest_id).unwrap();
        let msgs = guest.drain();
        assert!(
            matches!(&msgs[..], [ServerMsg::Welcome { host: false, peers, .. }] if peers.len() == 1)
        );
        let host_msgs = host.drain();
        assert!(
            host_msgs
                .iter()
                .any(|m| matches!(m, ServerMsg::PeerJoined { peer } if peer.id == guest_id))
        );
        assert!(
            host_msgs
                .iter()
                .any(|m| matches!(m, ServerMsg::KnockResolved { id } if *id == guest_id))
        );

        // 입장 후에는 시그널을 주고받을 수 있다.
        rooms
            .relay_signal(&code, &guest_id, &host_id, json!({}))
            .unwrap();
    }

    #[test]
    fn host_denies_guest() {
        let (rooms, code, host_id, _host) = room_with_host();
        let mut guest = client();
        let guest_id = waiting(
            rooms
                .join(&code, req("Guest"), None, guest.tx.clone())
                .unwrap(),
        );
        guest.drain();

        rooms.deny(&code, &host_id, &guest_id).unwrap();
        assert!(matches!(&guest.drain()[..], [ServerMsg::Denied]));
        assert_eq!(
            rooms.admit(&code, &host_id, &guest_id).unwrap_err(),
            ErrorCode::UnknownPeer
        );
    }

    #[test]
    fn non_hosts_cannot_admit_deny_or_change_settings() {
        let (rooms, code, host_id, _host) = room_with_host();
        let g1 = client();
        let g1_id = waiting(rooms.join(&code, req("G1"), None, g1.tx).unwrap());
        rooms.admit(&code, &host_id, &g1_id).unwrap();
        let g2 = client();
        let g2_id = waiting(rooms.join(&code, req("G2"), None, g2.tx).unwrap());

        // 입장한 게스트도, 대기 중인 게스트도 주최자 권한이 없다.
        for by in [&g1_id, &g2_id] {
            assert_eq!(
                rooms.admit(&code, by, &g2_id).unwrap_err(),
                ErrorCode::Forbidden
            );
            assert_eq!(
                rooms.deny(&code, by, &g2_id).unwrap_err(),
                ErrorCode::Forbidden
            );
            assert_eq!(
                rooms
                    .set_settings(&code, by, RoomSettings { translation: false })
                    .unwrap_err(),
                ErrorCode::Forbidden
            );
        }
        assert!(rooms.settings(&code).unwrap().translation);
    }

    #[test]
    fn waiting_guest_cannot_signal() {
        let (rooms, code, host_id, _host) = room_with_host();
        let g = client();
        let g_id = waiting(rooms.join(&code, req("G"), None, g.tx).unwrap());
        assert_eq!(
            rooms
                .relay_signal(&code, &g_id, &host_id, json!({}))
                .unwrap_err(),
            ErrorCode::UnknownPeer
        );
        assert_eq!(
            rooms
                .relay_signal(&code, &host_id, &g_id, json!({}))
                .unwrap_err(),
            ErrorCode::UnknownPeer
        );
    }

    #[test]
    fn host_changes_settings_and_everyone_is_told() {
        let (rooms, code, host_id, mut host) = room_with_host();
        let mut g = client();
        let g_id = waiting(rooms.join(&code, req("G"), None, g.tx.clone()).unwrap());
        rooms.admit(&code, &host_id, &g_id).unwrap();
        g.drain();
        host.drain();

        let off = RoomSettings { translation: false };
        rooms.set_settings(&code, &host_id, off).unwrap();
        assert!(
            matches!(&g.drain()[..], [ServerMsg::Settings { settings }] if !settings.translation)
        );
        assert!(matches!(&host.drain()[..], [ServerMsg::Settings { .. }]));
        assert_eq!(rooms.settings(&code), Some(off));
    }

    #[test]
    fn guest_leaving_queue_notifies_host() {
        let (rooms, code, _host_id, mut host) = room_with_host();
        let g = client();
        let g_id = waiting(rooms.join(&code, req("G"), None, g.tx).unwrap());
        host.drain();
        rooms.leave(&code, &g_id);
        assert!(matches!(&host.drain()[..], [ServerMsg::KnockResolved { id }] if *id == g_id));
    }

    #[test]
    fn knocks_waiting_before_host_arrives_are_listed_on_host_join() {
        let rooms = Rooms::default();
        let code = rooms.create(HOST, ON).unwrap();
        let g = client();
        let g_id = waiting(rooms.join(&code, req("Early"), None, g.tx).unwrap());

        let mut host = client();
        joined(
            rooms
                .join(&code, req("Host"), Some(HOST), host.tx.clone())
                .unwrap(),
        );
        assert!(
            matches!(&host.drain()[..], [ServerMsg::Welcome { knocks, .. }] if knocks[0].id == g_id)
        );
    }

    #[test]
    fn limits_room_size_queue_and_rooms_per_host() {
        let (rooms, code, host_id, _host) = room_with_host();
        let mut keep = Vec::new();
        for i in 1..MAX_PEERS {
            let c = client();
            let id = waiting(
                rooms
                    .join(&code, req(&i.to_string()), None, c.tx.clone())
                    .unwrap(),
            );
            rooms.admit(&code, &host_id, &id).unwrap();
            keep.push(c);
        }
        let c = client();
        assert_eq!(
            rooms.join(&code, req("late"), None, c.tx).unwrap_err(),
            ErrorCode::RoomFull
        );

        let (rooms, code, _host_id, _host) = room_with_host();
        for i in 0..MAX_WAITING {
            let c = client();
            waiting(
                rooms
                    .join(&code, req(&i.to_string()), None, c.tx.clone())
                    .unwrap(),
            );
            keep.push(c);
        }
        let c = client();
        assert_eq!(
            rooms.join(&code, req("spam"), None, c.tx).unwrap_err(),
            ErrorCode::TooManyWaiting
        );

        let rooms = Rooms::default();
        for _ in 0..MAX_ROOMS_PER_HOST {
            rooms.create(HOST, ON).unwrap();
        }
        assert_eq!(rooms.create(HOST, ON).unwrap_err(), ErrorCode::Forbidden);
    }

    #[test]
    fn idle_rooms_are_swept_but_occupied_rooms_stay() {
        let (rooms, busy, _host_id, _host) = room_with_host();
        let idle = rooms.create(HOST, ON).unwrap();
        rooms.sweep(Instant::now() + ROOM_IDLE_TTL + Duration::from_secs(1));
        assert!(rooms.settings(&idle).is_none());
        assert!(rooms.settings(&busy).is_some());
    }

    #[test]
    fn validates_inputs() {
        assert!(is_valid_room_code(&new_room_code()));
        assert!(!is_valid_room_code("abcd-efgh"));
        assert!(!is_valid_room_code("ABCD-efgh-ijkm"));
        assert_eq!(normalize_name("  유승재 ").as_deref(), Some("유승재"));
        assert!(normalize_name("   ").is_none());
        assert!(normalize_name(&"가".repeat(41)).is_none());
        assert!(is_valid_lang("ko") && is_valid_lang("zh-Hant") && is_valid_lang("en-US"));
        assert!(!is_valid_lang("") && !is_valid_lang("k") && !is_valid_lang("ko_KR"));
    }
}
