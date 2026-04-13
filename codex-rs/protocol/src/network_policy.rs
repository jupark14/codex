//! 📄 이 모듈이 하는 일:
//!   네트워크 정책 판단 결과를 프로토콜에 실어 나를 때 쓰는 작은 카드 타입을 정의한다.
//!   비유로 말하면 경비실에서 "이 주소는 통과/보류/차단"이라고 적어 보내는 판정 메모다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/error.rs`
//!   - 승인/guardian/네트워크 정책 표시 코드
//!
//! 🧩 핵심 개념:
//!   - `decision` = 최종 판정
//!   - `source` = 누가 그 판정을 내렸는지 적는 발신자 표기

use crate::approvals::NetworkApprovalProtocol;
use codex_network_proxy::NetworkDecisionSource;
use codex_network_proxy::NetworkPolicyDecision;
use serde::Deserialize;

/// 🍳 이 구조체는 네트워크 요청 한 건에 대한 판정 결과 카드다.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicyDecisionPayload {
    pub decision: NetworkPolicyDecision,
    pub source: NetworkDecisionSource,
    #[serde(default)]
    pub protocol: Option<NetworkApprovalProtocol>,
    pub host: Option<String>,
    pub reason: Option<String>,
    pub port: Option<u16>,
}

impl NetworkPolicyDecisionPayload {
    /// 🍳 이 함수는 "decider가 직접 사용자에게 다시 물어보라"고 보낸 ask인지 확인한다.
    pub fn is_ask_from_decider(&self) -> bool {
        self.decision == NetworkPolicyDecision::Ask && self.source == NetworkDecisionSource::Decider
    }
}
