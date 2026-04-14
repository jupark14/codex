//! 📄 이 모듈이 하는 일:
//!   로그인/구독 관련 plan 이름을 읽고, 사람이 보기 좋은 이름이나 분류 helper를 제공한다.
//!   비유로 말하면 회원권 이름표를 읽어서 "이건 팀권", "이건 학교권"처럼 다시 정리해 주는 접수대다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - 인증/로그인 응답 파싱 코드
//!   - 구독 플랜 표시 UI/상태 로직
//!
//! 🧩 핵심 개념:
//!   - `Known`/`Unknown` = 아는 회원권은 enum으로, 처음 보는 이름표는 원문 그대로 보관
//!   - `raw_value` = wire로 오가는 원래 문자열 이름표

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

/// 🍳 이 enum은 plan 문자열을 "아는 플랜"과 "처음 보는 플랜"으로 나눠 담는 분류 상자다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlanType {
    Known(KnownPlan),
    Unknown(String),
}

impl PlanType {
    /// 🍳 이 함수는 서버가 준 원문 문자열을 보고,
    ///   아는 회원권이면 `Known`, 처음 보면 `Unknown`으로 분류한다.
    pub fn from_raw_value(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "free" => Self::Known(KnownPlan::Free),
            "go" => Self::Known(KnownPlan::Go),
            "plus" => Self::Known(KnownPlan::Plus),
            "pro" => Self::Known(KnownPlan::Pro),
            "prolite" => Self::Known(KnownPlan::ProLite),
            "team" => Self::Known(KnownPlan::Team),
            "self_serve_business_usage_based" => {
                Self::Known(KnownPlan::SelfServeBusinessUsageBased)
            }
            "business" => Self::Known(KnownPlan::Business),
            "enterprise_cbp_usage_based" => Self::Known(KnownPlan::EnterpriseCbpUsageBased),
            "enterprise" | "hc" => Self::Known(KnownPlan::Enterprise),
            "education" | "edu" => Self::Known(KnownPlan::Edu),
            _ => Self::Unknown(raw.to_string()),
        }
    }
}

/// 🍳 이 enum은 이미 이름이 정해진 대표 플랜 목록이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnownPlan {
    Free,
    Go,
    Plus,
    Pro,
    ProLite,
    Team,
    #[serde(rename = "self_serve_business_usage_based")]
    SelfServeBusinessUsageBased,
    Business,
    #[serde(rename = "enterprise_cbp_usage_based")]
    EnterpriseCbpUsageBased,
    #[serde(alias = "hc")]
    Enterprise,
    Edu,
}

impl KnownPlan {
    /// 🍳 이 함수는 화면에 보여 줄 예쁜 이름을 꺼낸다.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Go => "Go",
            Self::Plus => "Plus",
            Self::Pro => "Pro",
            Self::ProLite => "Pro Lite",
            Self::Team => "Team",
            Self::SelfServeBusinessUsageBased => "Self Serve Business Usage Based",
            Self::Business => "Business",
            Self::EnterpriseCbpUsageBased => "Enterprise CBP Usage Based",
            Self::Enterprise => "Enterprise",
            Self::Edu => "Edu",
        }
    }

    /// 🍳 이 함수는 서버와 주고받을 원래 문자열 이름표를 돌려준다.
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Go => "go",
            Self::Plus => "plus",
            Self::Pro => "pro",
            Self::ProLite => "prolite",
            Self::Team => "team",
            Self::SelfServeBusinessUsageBased => "self_serve_business_usage_based",
            Self::Business => "business",
            Self::EnterpriseCbpUsageBased => "enterprise_cbp_usage_based",
            Self::Enterprise => "enterprise",
            Self::Edu => "edu",
        }
    }

    /// 🍳 이 함수는 팀/비즈니스/학교처럼 "공용 workspace 성격" 플랜인지 확인한다.
    pub fn is_workspace_account(self) -> bool {
        matches!(
            self,
            Self::Team
                | Self::SelfServeBusinessUsageBased
                | Self::Business
                | Self::EnterpriseCbpUsageBased
                | Self::Enterprise
                | Self::Edu
        )
    }
}

/// 🍳 이 구조체는 refresh token 갱신 실패를 이유표와 함께 묶어 주는 사고 보고서다.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct RefreshTokenFailedError {
    pub reason: RefreshTokenFailedReason,
    pub message: String,
}

impl RefreshTokenFailedError {
    /// 🍳 이 함수는 실패 이유와 설명 문장을 합쳐 새 에러 보고서를 만든다.
    pub fn new(reason: RefreshTokenFailedReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

/// 🍳 이 enum은 refresh token 실패 원인을 큰 분류표로 나눈다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenFailedReason {
    Expired,
    Exhausted,
    Revoked,
    Other,
}
