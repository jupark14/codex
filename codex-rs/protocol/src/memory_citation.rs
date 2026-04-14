//! 📄 이 모듈이 하는 일:
//!   모델 답변이 어떤 메모/로그 줄을 근거로 삼았는지 표시하는 인용 카드 구조를 정의한다.
//!   비유로 말하면 독후감 아래에 "몇 쪽 몇 줄을 참고했는지" 적는 출처 메모지다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/items.rs`
//!   - memory trace / rollout 근거 표시 코드
//!
//! 🧩 핵심 개념:
//!   - `entries` = 실제 파일/줄 출처 목록
//!   - `rollout_ids` = 어떤 대화/추적 묶음에서 근거가 나왔는지 가리키는 번호표

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// 🍳 이 구조체는 메모 인용 전체 묶음을 담는 출처 상자다.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCitation {
    pub entries: Vec<MemoryCitationEntry>,
    pub rollout_ids: Vec<String>,
}

/// 🍳 이 구조체는 출처 한 줄 한 줄을 적는 인용 카드다.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCitationEntry {
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub note: String,
}
