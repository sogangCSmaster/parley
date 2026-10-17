//! 방과 참가자 상태. 단일 프로세스 메모리에 둔다(MVP).
//!
//! 권한 규칙
//! - 방은 로그인한 주최자만 만든다(`create`).
//! - 주최자 계정으로 들어온 참가자는 바로 입장한다. 그 외(게스트)는 대기열에 들어가고,
//!   주최자가 수락해야 입장한다.
//! - 수락·거절·설정 변경은 그 방의 주최자 참가자만 할 수 있다. 모든 검사는 이 모듈에서 한다.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rand::RngExt;
use tokio::sync::{Notify, mpsc, watch};

use crate::ice::Ice;
use crate::limit::TokenBucket;
use crate::protocol::{
    ErrorCode, Knock, MediaState, Peer, PeerId, RoomSettings, ServerMsg, SignalData,
};

/// P2P mesh는 인원이 늘면 업링크가 (n-1)배가 되므로 4명으로 제한한다.
pub const MAX_PEERS: usize = 4;
/// 대기열 스팸 방지
pub const MAX_WAITING: usize = 8;
/// 주최자 한 명이 동시에 가질 수 있는 방
pub const MAX_ROOMS_PER_HOST: usize = 20;
/// 아무도 없는 방(만들고 아무도 안 들어왔거나 모두 나간 방)을 지우기까지의 시간
pub const ROOM_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
/// 연결이 끊긴 참가자가 주최자 수락 없이 돌아올 수 있는 시간
pub const RESUME_TTL: Duration = Duration::from_secs(2 * 60);
/// 음성 인식 토큰 발급 빈도(참가자당). 세션은 3분 넘게 조용하거나 언어가 바뀔 때 새로 연다. 평소 몇 분에 1개면 충분하다.
/// 연결이 아니라 참가자에 묶는다: 재입장 토큰으로 연결만 바꿔 가며 새 한도를 받지 못하게.
pub const STT_TOKENS_PER_SEC: f64 = 1.0 / 30.0;
pub const STT_TOKEN_BURST: f64 = 4.0;

/// 혼동하기 쉬운 문자(l, o, 0, 1)를 뺀 32자. 4-4-4 형식이면 60bit.
const CODE_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";

#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<ServerMsg>,
    disconnect: Arc<Notify>,
}

impl Outbox {
    pub fn new(tx: mpsc::Sender<ServerMsg>, disconnect: Arc<Notify>) -> Self {
        Self { tx, disconnect }
    }

    /// 이 연결을 닫게 한다(같은 참가자가 새 연결로 돌아와 옛 연결을 밀어낼 때).
    pub fn close(&self) {
        self.disconnect.notify_one();
    }

    pub fn send(&self, msg: ServerMsg) {
        if let Err(err) = self.tx.try_send(msg) {
            // 제어 메시지가 누락된 연결은 복구할 수 없으므로 닫아 상태 불일치를 알린다.
            tracing::warn!(%err, "disconnecting slow or closed client");
            self.disconnect.notify_one();
        }
    }
}

struct Member {
    caption_epoch: u64,
    peer: Peer,
    tx: Outbox,
    stt_tokens: TokenBucket,
}

fn new_stt_tokens() -> TokenBucket {
    TokenBucket::new(STT_TOKENS_PER_SEC, STT_TOKEN_BURST)
}

struct Room {
    host_account: String,
    settings: RoomSettings,
    caption_epoch: u64,
    caption_changes: watch::Sender<u64>,
    members: HashMap<PeerId, Member>,
    waiting: HashMap<PeerId, Member>,
    /// 방이 비게 된 시점(생성 직후 포함). 사람이 있으면 None.
    idle_since: Option<Instant>,
    /// 재입장 토큰 → 참가자. 입장할 때 하나씩 발급하고, 쓰면 없어진다.
    resumes: HashMap<String, Resume>,
}

struct Resume {
    peer: PeerId,
    /// 주최자가 수락한 이름. 다시 들어올 때 요청의 이름 대신 쓴다(수락 없이 이름을 바꾸지 못하게).
    name: String,
    /// 그 참가자가 나간 시점. 아직 입장해 있으면 None(끊긴 걸 서버가 아직 모르는 경우 포함).
    left_at: Option<Instant>,
    /// 나갈 때 남은 음성 인식 토큰 한도. 다시 들어오면 이어 받는다.
    stt_tokens: Option<TokenBucket>,
}

impl Room {
    fn issue_resume(&mut self, peer: &str, name: &str) -> String {
        let token = random_token(RESUME_LEN);
        self.resumes.insert(
            token.clone(),
            Resume {
                peer: peer.to_owned(),
                name: name.to_owned(),
                left_at: None,
                stt_tokens: None,
            },
        );
        token
    }
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
                lang: w.peer.lang.clone(),
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptionEpoch(pub u64, pub u64);

#[derive(Clone)]
pub struct Audience {
    pub epoch: CaptionEpoch,
    pub src_lang: String,
    pub partner_lang: String,
    /// 화자 자신을 포함한 입장한 참가자 전원
    pub readers: Vec<Reader>,
}

#[derive(Clone)]
pub struct Reader {
    pub id: PeerId,
    pub lang: String,
    pub tx: Outbox,
}

impl Audience {
    /// 이 독자에게 보여줄 번역 언어. 원문과 다른 자기 언어가 있으면 그것, 아니면 화자가 고른 상대 언어.
    /// 원문과 같은 언어가 나오면 번역할 필요가 없다.
    pub fn target_for<'a>(&'a self, reader_lang: &'a str) -> Option<&'a str> {
        let tgt = if reader_lang != self.src_lang {
            reader_lang
        } else {
            self.partner_lang.as_str()
        };
        (tgt != self.src_lang).then_some(tgt)
    }
}

#[derive(Default)]
pub struct Rooms {
    rooms: Mutex<HashMap<String, Room>>,
    ice: Ice,
}

pub struct JoinRequest {
    pub name: String,
    pub lang: String,
    pub partner_lang: String,
    pub media: MediaState,
    pub resume: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JoinOutcome {
    /// 입장 완료. Welcome은 이미 보냈다.
    Joined(PeerId),
    /// 주최자 수락 대기. Waiting은 이미 보냈다.
    Waiting(PeerId),
}

impl Rooms {
    pub fn new(ice: Ice) -> Self {
        Self {
            rooms: Mutex::default(),
            ice,
        }
    }

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
            caption_epoch: 0,
            caption_changes: watch::channel(0).0,
            members: HashMap::new(),
            waiting: HashMap::new(),
            resumes: HashMap::new(),
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
        let id = random_token(12);
        // 자격은 잠금 밖에서 만든다. 주최자가 아니면 수락 때 다시 만들므로 여기 것은 버려진다(수 μs).
        let ice_servers = self.ice.servers_for(&id, SystemTime::now());
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(code).ok_or(ErrorCode::RoomNotFound)?;
        let now = Instant::now();
        // 1회용: 맞든 틀리든 쓴 토큰은 지운다.
        let mut resumed = req
            .resume
            .as_deref()
            .and_then(|token| room.resumes.remove(token))
            .filter(|r| r.left_at.is_none_or(|t| now.duration_since(t) < RESUME_TTL));
        // 같은 참가자의 남은 음성 인식 토큰 한도를 이어 받는다(옛 연결이 아직 있으면 그쪽이 최신).
        let mut stt_tokens = resumed.as_mut().and_then(|r| r.stt_tokens.take());
        if let Some(r) = &resumed
            && let Some(old) = room.members.remove(&r.peer)
        {
            // 서버가 아직 끊긴 줄 모르는 옛 연결(반쯤 열린 TCP)을 밀어낸다.
            old.tx.close();
            stt_tokens = Some(old.stt_tokens);
            room.caption_changes.send_modify(|v| *v += 1);
            broadcast(&room.members, &r.peer, ServerMsg::PeerLeft { id: r.peer.clone() });
        }
        if room.members.len() >= MAX_PEERS {
            return Err(ErrorCode::RoomFull);
        }
        let is_host = account == Some(room.host_account.as_str());
        if !is_host && resumed.is_none() && room.waiting.len() >= MAX_WAITING {
            return Err(ErrorCode::TooManyWaiting);
        }
        let peer = Peer {
            id: id.clone(),
            name: resumed.as_ref().map_or(req.name, |r| r.name.clone()),
            lang: req.lang,
            partner_lang: req.partner_lang,
            media: req.media,
            host: is_host,
        };
        room.idle_since = None;

        // 주최자 계정이거나, 수락받은 적 있는 참가자가 재입장 토큰으로 돌아온 경우 바로 입장한다.
        if is_host || resumed.is_some() {
            let resume = room.issue_resume(&id, &peer.name);
            let welcome = ServerMsg::Welcome {
                you: id.clone(),
                host: is_host,
                settings: room.settings,
                peers: room.members.values().map(|m| m.peer.clone()).collect(),
                knocks: if is_host { room.knocks() } else { Vec::new() },
                ice_servers,
                resume,
            };
            send(&tx, welcome);
            broadcast(
                &room.members,
                &id,
                ServerMsg::PeerJoined { peer: peer.clone() },
            );
            room.members.insert(
                id.clone(),
                Member {
                    caption_epoch: 0,
                    peer,
                    tx,
                    stt_tokens: stt_tokens.unwrap_or_else(new_stt_tokens),
                },
            );
            room.caption_changes.send_modify(|v| *v += 1);
            return Ok(JoinOutcome::Joined(id));
        }

        send(&tx, ServerMsg::Waiting);
        room.notify_hosts(ServerMsg::Knock {
            knock: Knock {
                id: id.clone(),
                name: peer.name.clone(),
                lang: peer.lang.clone(),
            },
        });
        room.waiting.insert(
            id.clone(),
            Member {
                caption_epoch: 0,
                peer,
                tx,
                stt_tokens: new_stt_tokens(),
            },
        );
        Ok(JoinOutcome::Waiting(id))
    }

    pub fn admit(&self, code: &str, by: &str, guest: &str) -> Result<(), ErrorCode> {
        // 자격은 잠금 밖에서 만든다. 수락이 거절되면 버려진다.
        // 권한 확인 전이라 아무 문자열에나 계산될 수 있지만 HMAC 한 번이고 Welcome으로만 나간다.
        let ice_servers = self.ice.servers_for(guest, SystemTime::now());
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
        let resume = room.issue_resume(guest, &member.peer.name);
        let welcome = ServerMsg::Welcome {
            you: guest.to_owned(),
            host: false,
            settings: room.settings,
            peers: room.members.values().map(|m| m.peer.clone()).collect(),
            knocks: Vec::new(),
            ice_servers,
            resume,
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
        room.caption_changes.send_modify(|v| *v += 1);
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
        if room.settings.translation != settings.translation {
            room.caption_epoch += 1;
            room.caption_changes.send_modify(|v| *v += 1);
        }
        room.settings = settings;
        for m in room.members.values() {
            send(&m.tx, ServerMsg::Settings { settings });
        }
        Ok(())
    }

    /// 입장한 참가자만 자기 정보를 바꿀 수 있다(대기 중인 게스트는 무시).
    pub fn update(&self, code: &str, id: &str, f: impl FnOnce(&mut Peer)) {
        self.apply(code, id, true, f);
    }

    /// 상태만 바꾸고 다른 참가자에게 알리는 것은 미룬다(빈도 제한 중). 나중에 `announce`로 최신 상태를 한 번 보낸다.
    pub fn update_quietly(&self, code: &str, id: &str, f: impl FnOnce(&mut Peer)) {
        self.apply(code, id, false, f);
    }

    /// 이 참가자의 현재 상태를 다른 참가자에게 알린다.
    pub fn announce(&self, code: &str, id: &str) {
        let rooms = self.rooms.lock().unwrap();
        if let Some(room) = rooms.get(code)
            && let Some(member) = room.members.get(id)
        {
            let peer = member.peer.clone();
            broadcast(&room.members, id, ServerMsg::PeerUpdated { peer });
        }
    }

    fn apply(&self, code: &str, id: &str, announce: bool, f: impl FnOnce(&mut Peer)) {
        let mut rooms = self.rooms.lock().unwrap();
        let Some(room) = rooms.get_mut(code) else {
            return;
        };
        let Some(member) = room.members.get_mut(id) else {
            return;
        };
        let previous_lang = member.peer.lang.clone();
        let previous_partner = member.peer.partner_lang.clone();
        f(&mut member.peer);
        if previous_lang != member.peer.lang {
            member.caption_epoch += 1;
        }
        if previous_lang != member.peer.lang || previous_partner != member.peer.partner_lang {
            room.caption_changes.send_modify(|v| *v += 1);
        }
        if announce {
            let peer = member.peer.clone();
            broadcast(&room.members, id, ServerMsg::PeerUpdated { peer });
        }
    }

    /// 입장한 참가자끼리만 시그널을 중계한다. 대기 중인 게스트·다른 방 참가자는 거부한다.
    pub fn relay_signal(
        &self,
        code: &str,
        from: &str,
        to: &str,
        data: SignalData,
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
        if let Some(member) = room.members.remove(id) {
            room.caption_changes.send_modify(|v| *v += 1);
            broadcast(&room.members, id, ServerMsg::PeerLeft { id: id.to_owned() });
            let now = Instant::now();
            for r in room.resumes.values_mut().filter(|r| r.peer == id) {
                r.left_at.get_or_insert(now);
                r.stt_tokens = Some(member.stt_tokens.clone());
            }
        } else if room.waiting.remove(id).is_some() {
            room.notify_hosts(ServerMsg::KnockResolved { id: id.to_owned() });
        }
        if room.members.is_empty() && room.waiting.is_empty() {
            room.idle_since = Some(Instant::now());
        }
    }

    pub fn caption_changes(&self, code: &str) -> Option<watch::Receiver<u64>> {
        self.rooms
            .lock()
            .unwrap()
            .get(code)
            .map(|r| r.caption_changes.subscribe())
    }

    /// 화자의 자막을 받을 사람들. 번역이 꺼진 방이거나 화자가 입장한 참가자가 아니면 None.
    /// 잠금은 복사하는 동안만 잡는다(보내는 쪽 채널은 복제 비용이 작다).
    pub fn caption_audience(&self, code: &str, speaker: &str) -> Option<Audience> {
        let rooms = self.rooms.lock().unwrap();
        let room = rooms.get(code)?;
        if !room.settings.translation {
            return None;
        }
        let me = room.members.get(speaker)?;
        Some(Audience {
            epoch: CaptionEpoch(room.caption_epoch, me.caption_epoch),
            src_lang: me.peer.lang.clone(),
            partner_lang: me.peer.partner_lang.clone(),
            readers: room
                .members
                .values()
                .map(|m| Reader {
                    id: m.peer.id.clone(),
                    lang: m.peer.lang.clone(),
                    tx: m.tx.clone(),
                })
                .collect(),
        })
    }

    /// 이 참가자가 음성 인식 토큰을 하나 더 받아도 되는지(받으면 한도를 하나 쓴다). 입장한 참가자만.
    pub fn take_stt_token(&self, code: &str, id: &str, now: Instant) -> bool {
        let mut rooms = self.rooms.lock().unwrap();
        rooms
            .get_mut(code)
            .and_then(|r| r.members.get_mut(id))
            .is_some_and(|m| m.stt_tokens.take(now))
    }

    /// 음성 인식 토큰에 잠글 언어: 내 언어, 그리고 다르면 상대 언어(상대 언어로 한 말도 그 언어 글자로 적게).
    /// 입장한 참가자이고 번역이 켜진 방이어야 한다(아니면 None).
    pub fn stt_langs(&self, code: &str, id: &str) -> Option<Vec<String>> {
        let rooms = self.rooms.lock().unwrap();
        let room = rooms.get(code).filter(|r| r.settings.translation)?;
        let peer = &room.members.get(id)?.peer;
        let mut langs = vec![peer.lang.clone()];
        if peer.partner_lang != peer.lang {
            langs.push(peer.partner_lang.clone());
        }
        Some(langs)
    }

    /// 오래 빈 방을 지운다(주기 작업).
    pub fn sweep(&self, now: Instant) {
        let mut rooms = self.rooms.lock().unwrap();
        rooms.retain(|_, r| {
            r.idle_since
                .is_none_or(|t| now.duration_since(t) < ROOM_IDLE_TTL)
        });
        for room in rooms.values_mut() {
            room.resumes
                .retain(|_, r| r.left_at.is_none_or(|t| now.duration_since(t) < RESUME_TTL));
        }
    }
}

fn broadcast(members: &HashMap<PeerId, Member>, except: &str, msg: ServerMsg) {
    for (id, member) in members {
        if id != except {
            send(&member.tx, msg.clone());
        }
    }
}

pub fn send(tx: &Outbox, msg: ServerMsg) {
    tx.send(msg);
}

/// 재입장 토큰 모양(`issue_resume`이 만드는 24자). 다르면 찾아보지도 않는다.
pub fn is_valid_resume(token: &str) -> bool {
    token.len() == RESUME_LEN && token.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

const RESUME_LEN: usize = 24;

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

    const HOST: &str = "alice";
    const ON: RoomSettings = RoomSettings { translation: true };

    fn req(name: &str) -> JoinRequest {
        JoinRequest {
            name: name.into(),
            lang: "ko".into(),
            partner_lang: "ja".into(),
            media: MediaState::default(),
            resume: None,
        }
    }

    struct Client {
        tx: Outbox,
        rx: mpsc::Receiver<ServerMsg>,
    }

    fn client() -> Client {
        let (tx, rx) = mpsc::channel(32);
        Client {
            tx: Outbox::new(tx, Arc::new(Notify::new())),
            rx,
        }
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
    fn turn_credentials_go_only_to_admitted_participants() {
        let ice = Ice::new(
            Some("[]"),
            Some("turn:t.example:3478"),
            Some("0123456789abcdef0123456789abcdef"),
        )
        .unwrap();
        let rooms = Rooms::new(ice);
        let code = rooms.create(HOST, ON).unwrap();
        let turn_user = |msgs: Vec<ServerMsg>| match &msgs[..] {
            [ServerMsg::Welcome { ice_servers, .. }] => ice_servers[0].username.clone().unwrap(),
            other => panic!("expected welcome, got {other:?}"),
        };

        let mut host = client();
        let host_id = joined(
            rooms
                .join(&code, req("Host"), Some(HOST), host.tx.clone())
                .unwrap(),
        );
        assert!(turn_user(host.drain()).ends_with(&format!(":{host_id}")));

        let mut guest = client();
        let guest_id = waiting(
            rooms
                .join(&code, req("Guest"), None, guest.tx.clone())
                .unwrap(),
        );
        assert!(matches!(&guest.drain()[..], [ServerMsg::Waiting]));
        rooms.admit(&code, &host_id, &guest_id).unwrap();
        assert!(turn_user(guest.drain()).ends_with(&format!(":{guest_id}")));
    }

    fn resume_req(name: &str, token: &str) -> JoinRequest {
        JoinRequest {
            resume: Some(token.into()),
            ..req(name)
        }
    }

    /// 주최자 방 + 수락된 게스트. (rooms, code, host_id, host, guest_id, guest, 게스트의 재입장 토큰)
    fn room_with_admitted_guest() -> (Rooms, String, PeerId, Client, PeerId, Client, String) {
        let (rooms, code, host_id, mut host) = room_with_host();
        let mut guest = client();
        let guest_id = waiting(rooms.join(&code, req("Guest"), None, guest.tx.clone()).unwrap());
        rooms.admit(&code, &host_id, &guest_id).unwrap();
        let token = match &guest.drain()[..] {
            [ServerMsg::Waiting, ServerMsg::Welcome { resume, .. }] => resume.clone(),
            other => panic!("expected welcome, got {other:?}"),
        };
        host.drain();
        (rooms, code, host_id, host, guest_id, guest, token)
    }

    #[test]
    fn admitted_guest_comes_back_without_admission_once() {
        let (rooms, code, _, mut host, guest_id, _, token) = room_with_admitted_guest();
        rooms.leave(&code, &guest_id);
        host.drain();

        let mut back = client();
        let new_id = joined(rooms.join(&code, resume_req("CEO", &token), None, back.tx.clone()).unwrap());
        assert!(matches!(&back.drain()[..], [ServerMsg::Welcome { host: false, .. }]));
        let rejoined = host.drain();
        assert!(
            rejoined.iter().any(|m| matches!(m, ServerMsg::PeerJoined { peer } if peer.name == "Guest")),
            "keeps the name the host admitted"
        );
        assert!(rejoined.iter().any(|m| matches!(m, ServerMsg::PeerJoined { peer } if peer.id == new_id)));

        // 1회용: 같은 토큰으로는 다시 대기열로 간다.
        rooms.leave(&code, &new_id);
        let again = client();
        waiting(rooms.join(&code, resume_req("Guest", &token), None, again.tx).unwrap());
    }

    #[test]
    fn stt_token_limit_survives_reconnecting_with_a_resume_token() {
        let (rooms, code, _, _, guest_id, _old, token) = room_with_admitted_guest();
        let now = Instant::now();
        for _ in 0..STT_TOKEN_BURST as usize {
            assert!(rooms.take_stt_token(&code, &guest_id, now));
        }
        assert!(!rooms.take_stt_token(&code, &guest_id, now), "burst used up");

        // 정상적으로 나갔다가 토큰으로 돌아와도 한도가 다시 차지 않는다.
        rooms.leave(&code, &guest_id);
        let mut back = client();
        let id2 = joined(rooms.join(&code, resume_req("Guest", &token), None, back.tx.clone()).unwrap());
        assert!(!rooms.take_stt_token(&code, &id2, now), "not refilled by leave + resume");
        let token2 = match &back.drain()[..] {
            [ServerMsg::Welcome { resume, .. }] => resume.clone(),
            other => panic!("expected welcome, got {other:?}"),
        };

        // 옛 연결을 밀어내며 돌아와도 마찬가지다(반쯤 열린 연결).
        let again = client();
        let id3 = joined(rooms.join(&code, resume_req("Guest", &token2), None, again.tx).unwrap());
        assert!(!rooms.take_stt_token(&code, &id3, now), "not refilled by evicting a half-open connection");

        // 시간이 지나면 정상적으로 다시 찬다.
        assert!(rooms.take_stt_token(&code, &id3, now + Duration::from_secs(31)));
        assert!(!rooms.take_stt_token(&code, "nobody", now), "only members get tokens");
    }

    #[test]
    fn resume_expires_and_wrong_tokens_wait_for_admission() {
        let (rooms, code, _, _, guest_id, _, token) = room_with_admitted_guest();
        let wrong = client();
        waiting(rooms.join(&code, resume_req("Mallory", "not-a-token"), None, wrong.tx).unwrap());

        rooms.leave(&code, &guest_id);
        rooms.sweep(Instant::now() + RESUME_TTL + Duration::from_secs(1));
        let late = client();
        waiting(rooms.join(&code, resume_req("Guest", &token), None, late.tx).unwrap());
    }

    #[test]
    fn resume_replaces_a_half_open_old_connection() {
        let (rooms, code, host_id, mut host, guest_id, _old, token) = room_with_admitted_guest();
        // 서버는 옛 연결이 끊긴 걸 아직 모른다(leave 없음).
        let mut back = client();
        let new_id = joined(rooms.join(&code, resume_req("Guest", &token), None, back.tx.clone()).unwrap());
        back.drain();
        let msgs = host.drain();
        assert!(msgs.iter().any(|m| matches!(m, ServerMsg::PeerLeft { id } if *id == guest_id)));
        assert!(msgs.iter().any(|m| matches!(m, ServerMsg::PeerJoined { peer } if peer.id == new_id)));
        // 옛 연결이 나중에 정리돼도 새 참가자에는 영향이 없다.
        rooms.leave(&code, &guest_id);
        assert!(!host.drain().iter().any(|m| matches!(m, ServerMsg::PeerLeft { id } if *id == new_id)));
        assert!(rooms.relay_signal(&code, &host_id, &new_id, SignalData::candidate("c")).is_ok());
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
            .relay_signal(&code, &guest_id, &host_id, SignalData::candidate("c"))
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
                .relay_signal(&code, &g_id, &host_id, SignalData::candidate("c"))
                .unwrap_err(),
            ErrorCode::UnknownPeer
        );
        assert_eq!(
            rooms
                .relay_signal(&code, &host_id, &g_id, SignalData::candidate("c"))
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
