//! 클라이언트와 주고받는 WebSocket 메시지.
//! `cargo test` 실행 시 ts-rs가 `web/src/protocol/`에 TypeScript 타입을 생성한다.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub type PeerId = String;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Peer {
    pub id: PeerId,
    pub name: String,
    /// 내 언어: 내가 말하고 읽는 언어 (BCP-47)
    pub lang: String,
    /// 상대 언어: 내 발화를 번역할 대상 언어 (BCP-47)
    pub partner_lang: String,
    pub media: MediaState,
    /// 회의를 만든 주최자 계정으로 들어온 참가자
    pub host: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomSettings {
    /// 번역 자막 사용 여부. 주최자만 바꿀 수 있다.
    pub translation: bool,
}

/// 입장 대기 중인 게스트. 주최자에게만 보인다.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Knock {
    pub id: PeerId,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaState {
    pub mic: bool,
    pub cam: bool,
    pub screen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "t", rename_all = "snake_case")]
#[ts(export)]
pub enum ClientMsg {
    Join {
        room: String,
        name: String,
        lang: String,
        partner_lang: String,
        media: MediaState,
    },
    Lang {
        lang: String,
        partner_lang: String,
    },
    Media {
        media: MediaState,
    },
    /// SDP offer/answer 또는 ICE candidate. 서버는 내용을 해석하지 않고 같은 방 상대에게만 중계한다.
    Signal {
        to: PeerId,
        data: serde_json::Value,
    },
    /// 주최자 전용: 대기 중인 게스트 입장 수락/거절
    Admit {
        id: PeerId,
    },
    Deny {
        id: PeerId,
    },
    /// 주최자 전용: 방 설정 변경
    Settings {
        settings: RoomSettings,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "t", rename_all = "snake_case")]
#[ts(export)]
pub enum ServerMsg {
    Welcome {
        you: PeerId,
        host: bool,
        settings: RoomSettings,
        peers: Vec<Peer>,
        /// 주최자에게만 채워진다: 이미 대기 중인 게스트들
        knocks: Vec<Knock>,
    },
    /// 게스트: 주최자 수락을 기다리는 중
    Waiting,
    /// 게스트: 주최자가 거절함
    Denied,
    /// 주최자: 게스트가 입장을 요청함
    Knock {
        knock: Knock,
    },
    /// 주최자: 대기 요청이 처리됨(수락·거절·취소). 다른 주최자 창의 알림을 닫는 데 쓴다.
    KnockResolved {
        id: PeerId,
    },
    Settings {
        settings: RoomSettings,
    },
    PeerJoined {
        peer: Peer,
    },
    PeerUpdated {
        peer: Peer,
    },
    PeerLeft {
        id: PeerId,
    },
    Signal {
        from: PeerId,
        data: serde_json::Value,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    BadRequest,
    RoomFull,
    NotJoined,
    UnknownPeer,
    RoomNotFound,
    Forbidden,
    TooManyWaiting,
}
