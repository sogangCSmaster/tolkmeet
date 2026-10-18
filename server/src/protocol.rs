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
    /// 이 사람이 말하는 언어. 주최자가 누구인지 가늠하도록 알림에 함께 보인다.
    pub lang: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaState {
    pub mic: bool,
    pub cam: bool,
    pub screen: bool,
}

/// `RTCPeerConnection`의 `iceServers` 항목. TURN 자격은 참가자마다 시간제한으로 발급한다(`ice.rs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credential: Option<String>,
}

/// SDP offer/answer 또는 ICE 후보 중 하나(브라우저 `toJSON()` 형식). 서버는 모양만 검증해 같은 방 상대에게
/// 중계한다. 임의 JSON을 받지 않는 것은 중계 큐에 쌓이는 메모리를 글자 수만큼으로 묶기 위해서다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SignalData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<SessionDescription>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub candidate: Option<IceCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionDescription {
    #[serde(rename = "type")]
    pub kind: SdpType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sdp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum SdpType {
    Offer,
    Answer,
    Pranswer,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IceCandidate {
    pub candidate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub sdp_mid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub sdp_m_line_index: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub username_fragment: Option<String>,
}

impl SignalData {
    #[cfg(test)]
    pub fn candidate(candidate: &str) -> Self {
        SignalData {
            description: None,
            candidate: Some(IceCandidate {
                candidate: candidate.into(),
                sdp_mid: None,
                sdp_m_line_index: None,
                username_fragment: None,
            }),
        }
    }

    /// 둘 중 정확히 하나만 있어야 한다.
    pub fn is_valid(&self) -> bool {
        self.description.is_some() != self.candidate.is_some()
    }
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
        /// 연결이 끊겼다 다시 들어올 때, 지난 `welcome`에서 받은 값. 맞으면 주최자 수락 없이 바로 입장한다.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        resume: Option<String>,
    },
    Lang {
        lang: String,
        partner_lang: String,
    },
    Media {
        media: MediaState,
    },
    /// SDP offer/answer 또는 ICE candidate. 같은 방 상대에게만 중계한다.
    Signal {
        to: PeerId,
        data: SignalData,
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
    /// 내 음성 인식 결과. `seg`는 STT 구간 번호이고, 같은 구간의 중간 결과는 "지금까지의 전체 문장"이다.
    /// `final`이 오면 그 구간은 확정되고 다음 구간 번호로 넘어간다. 바뀐 경우에만 보낸다.
    Stt {
        seg: u32,
        text: String,
        #[serde(rename = "final")]
        is_final: bool,
    },
    /// 음성 인식 세션 하나를 열 1회용 토큰을 요청한다. 입장했고 번역이 켜진 방에서만 발급된다.
    /// 답은 같은 `id`의 `stt_token` 또는 `stt_token_denied`다(발급은 비동기라 답 순서가 바뀔 수 있다).
    SttToken {
        id: u32,
    },
    /// 나가기 버튼으로 나간다(창을 닫거나 연결이 끊긴 것과 구별한다). 마지막 남은 주최자 연결이 보내면 회의를 끝낸다.
    Leave,
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
        /// 이 참가자 전용 ICE 서버(TURN 자격 포함). 입장이 확정된 참가자만 받는다.
        ice_servers: Vec<IceServer>,
        /// 연결이 끊기면 이 값으로 다시 입장한다(1회용, 서버가 끊김을 알아챈 뒤 2분까지). 메모리에만 둔다.
        resume: String,
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
        data: SignalData,
    },
    /// 자막 한 줄(한 문장). 같은 `seg`는 같은 줄이고, 더 큰 `rev`만 반영한다.
    /// A final caption with an empty `src` retracts the previous revision of that segment.
    Caption {
        seg: String,
        speaker: PeerId,
        src_lang: String,
        src: String,
        tgt_lang: String,
        /// 번역. 아직 없거나 번역할 필요가 없거나(같은 언어) 실패하면 null이다.
        /// 확정 전에는 이전 원문에 대한 번역일 수 있다.
        text: Option<String>,
        #[serde(rename = "final")]
        is_final: bool,
        rev: u32,
    },
    /// Gemini Live `BidiGenerateContentConstrained`에 `access_token`으로 쓰는 1회용 토큰.
    /// 모델·전사 언어(요청 시점의 내 `lang`)·수동 활동 모드가 토큰에 잠겨 있다. 받은 즉시(1분 안에) 세션을 열어야 한다.
    SttToken {
        id: u32,
        token: String,
        model: String,
    },
    /// 토큰을 주지 않았다. `retry`가 false면(키 없음·입장 전·번역 꺼짐) 이 연결에서 다시 요청하지 않는다.
    /// true면 일시적 오류나 요청 과다라서 잠시 뒤 다시 요청한다.
    SttTokenDenied {
        id: u32,
        retry: bool,
    },
    Notice {
        code: NoticeCode,
    },
    /// 회의가 끝났다(주최자가 나가기를 눌렀거나, 연결이 끊긴 주최자가 다시 들어올 수 있는 시간 안에 돌아오지 않았다).
    /// 받은 쪽은 다시 연결하지 않는다. 서버는 방을 지웠다.
    Ended,
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
    /// 같은 종류의 메시지를 너무 자주 보내 버렸다.
    RateLimited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NoticeCode {
    /// 번역에 실패해 원문만 확정했다.
    TranslationFailed,
    /// 주최자의 이번 결제 기간 번역 시간이 다 떨어졌다. 음성 인식이 멈춘다.
    /// 입장(`welcome`) 직후에도 그때 상태로 다시 보낸다(다시 연결한 화면이 옛 상태에 머물지 않게).
    QuotaExceeded,
    /// 번역 시간이 다시 생겼다(추가 구매·새 기간). 음성 인식을 다시 시작해도 된다.
    QuotaRestored,
    /// 주최자 계정으로 들어온 사람이 한동안(10분) 없다. 남은 사람의 음성 인식·번역이 멈춘다(주최자의 시간을 쓰지 않게).
    /// 그런 방에 들어오면 `welcome` 직후에 보낸다.
    HostAway,
    /// 주최자가 돌아왔다. 음성 인식을 다시 시작해도 된다.
    HostBack,
    /// 서비스 전체의 오늘(UTC) 번역 지출이 운영자 상한을 넘었다. 모든 방의 음성 인식·번역이 멈춘다(영상·음성은 그대로).
    /// 다음 UTC 날짜에 풀린다. 그런 방에 들어오면 `welcome` 직후에 보낸다.
    SpendLimit,
    /// 지출 상한으로 멈췄던 자막이 다시 켜졌다. 음성 인식을 다시 시작해도 된다.
    SpendRestored,
}

#[cfg(test)]
mod signal_tests {
    use super::*;

    /// 브라우저 `toJSON()`이 실제로 보내는 모양(Firefox·Safari는 null 필드를 그대로 보낸다).
    #[test]
    fn browser_signal_shapes_parse() {
        for raw in [
            r#"{"t":"signal","to":"p","data":{"candidate":{"candidate":"candidate:1 1 udp 2122260223 192.168.0.2 5000 typ host","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"abcd"}}}"#,
            r#"{"t":"signal","to":"p","data":{"candidate":{"candidate":"","sdpMid":null,"sdpMLineIndex":null,"usernameFragment":null}}}"#,
            r#"{"t":"signal","to":"p","data":{"description":{"type":"offer","sdp":"v=0\r\n"}}}"#,
            r#"{"t":"signal","to":"p","data":{"description":{"type":"rollback"}}}"#,
        ] {
            let msg: ClientMsg = serde_json::from_str(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            let ClientMsg::Signal { data, .. } = msg else {
                panic!("expected signal")
            };
            assert!(data.is_valid(), "{raw}");
        }
    }
}
