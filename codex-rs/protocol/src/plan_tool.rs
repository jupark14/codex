//! 📄 이 모듈이 하는 일:
//!   `update_plan` 도구가 받는 체크리스트 인자 타입을 정의한다.
//!   비유로 말하면 할 일 보드에서 각 칸이 "대기/진행중/완료" 중 어디에 있는지 적는 상태표다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/protocol.rs`
//!   - plan 업데이트 도구 호출 코드
//!
//! 🧩 핵심 개념:
//!   - `StepStatus` = 할 일 카드 상태 스티커
//!   - `UpdatePlanArgs` = 설명문 + 체크리스트 묶음

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

// Types for the TODO tool arguments matching codex-vscode/todo-mcp/src/main.rs
/// 🍳 이 enum은 계획 단계 한 칸의 상태를 나타내는 신호등이다.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

/// 🍳 이 구조체는 계획 보드 한 줄을 담는 할 일 카드다.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PlanItemArg {
    pub step: String,
    pub status: StepStatus,
}

/// 🍳 이 구조체는 `update_plan` 도구가 한 번에 받는 전체 계획판이다.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlanArgs {
    /// Arguments for the `update_plan` todo/checklist tool (not plan mode).
    #[serde(default)]
    pub explanation: Option<String>,
    pub plan: Vec<PlanItemArg>,
}
