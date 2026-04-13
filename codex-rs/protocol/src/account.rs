//! 📄 이 모듈이 하는 일:
//!   계정이 어떤 요금제 묶음에 속하는지 공통 enum으로 정의한다.
//!   비유로 말하면 학생증, 교직원증, VIP 카드처럼 회원증 종류를 한 표에 정리한 안내판이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/lib.rs`
//!   - 계정/권한 상태를 표시하거나 직렬화하는 crate들
//!
//! 🧩 핵심 개념:
//!   - `serde` rename = Rust 이름과 wire 문자열 이름표를 맞춰 붙이는 변환 규칙
//!   - `is_team_like`/`is_business_like` = 여러 세부 플랜을 큰 가족 묶음으로 다시 분류하는 helper

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// 🍳 이 enum은 여러 구독 플랜을 회원권 종류표처럼 정리한 목록이다.
///   계정 플랜 문자열 ↔ 프로토콜 공통 `PlanType`
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, JsonSchema, TS, Default)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum PlanType {
    #[default]
    Free,
    Go,
    Plus,
    Pro,
    ProLite,
    Team,
    #[serde(rename = "self_serve_business_usage_based")]
    #[ts(rename = "self_serve_business_usage_based")]
    SelfServeBusinessUsageBased,
    Business,
    #[serde(rename = "enterprise_cbp_usage_based")]
    #[ts(rename = "enterprise_cbp_usage_based")]
    EnterpriseCbpUsageBased,
    Enterprise,
    Edu,
    #[serde(other)]
    Unknown,
}

impl PlanType {
    /// 🍳 이 함수는 "팀 묶음 회원권"인지 빠르게 확인하는 도장 검사기다.
    ///   플랜 종류 → team 계열 여부
    pub fn is_team_like(self) -> bool {
        matches!(self, Self::Team | Self::SelfServeBusinessUsageBased)
    }

    /// 🍳 이 함수는 "비즈니스 묶음 회원권"인지 확인하는 분류함이다.
    ///   플랜 종류 → business 계열 여부
    pub fn is_business_like(self) -> bool {
        matches!(self, Self::Business | Self::EnterpriseCbpUsageBased)
    }
}

#[cfg(test)]
mod tests {
    use super::PlanType;
    use pretty_assertions::assert_eq;

    #[test]
    fn usage_based_plan_types_use_expected_wire_names() {
        assert_eq!(
            serde_json::to_string(&PlanType::SelfServeBusinessUsageBased)
                .expect("self-serve business usage based should serialize"),
            "\"self_serve_business_usage_based\""
        );
        assert_eq!(
            serde_json::to_string(&PlanType::EnterpriseCbpUsageBased)
                .expect("enterprise cbp usage based should serialize"),
            "\"enterprise_cbp_usage_based\""
        );
        assert_eq!(
            serde_json::to_string(&PlanType::ProLite).expect("prolite should serialize"),
            "\"prolite\""
        );
        assert_eq!(
            serde_json::from_str::<PlanType>("\"self_serve_business_usage_based\"")
                .expect("self-serve business usage based should deserialize"),
            PlanType::SelfServeBusinessUsageBased
        );
        assert_eq!(
            serde_json::from_str::<PlanType>("\"prolite\"").expect("prolite should deserialize"),
            PlanType::ProLite
        );
        assert_eq!(
            serde_json::from_str::<PlanType>("\"enterprise_cbp_usage_based\"")
                .expect("enterprise cbp usage based should deserialize"),
            PlanType::EnterpriseCbpUsageBased
        );
    }

    #[test]
    fn plan_family_helpers_group_usage_based_variants_with_existing_plans() {
        assert_eq!(PlanType::Team.is_team_like(), true);
        assert_eq!(PlanType::SelfServeBusinessUsageBased.is_team_like(), true);
        assert_eq!(PlanType::Business.is_team_like(), false);

        assert_eq!(PlanType::Business.is_business_like(), true);
        assert_eq!(PlanType::EnterpriseCbpUsageBased.is_business_like(), true);
        assert_eq!(PlanType::Team.is_business_like(), false);
    }
}
