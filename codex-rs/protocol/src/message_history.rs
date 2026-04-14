//! 📄 이 모듈이 하는 일:
//!   메시지 히스토리 한 줄 항목의 최소 공통 모양을 정의한다.
//!   비유로 말하면 대화 노트에 "어느 대화였는지, 언제였는지, 무슨 말이었는지" 적는 출석부 한 칸이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/core`
//!   - 메시지 기록 저장/조회 코드
//!
//! 🧩 핵심 개념:
//!   - `conversation_id` = 어느 대화방 기록인지 알려 주는 반 번호
//!   - `ts` = 시간순 정렬용 타임스탬프

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// 🍳 이 구조체는 메시지 기록 한 줄을 담는 공책 칸이다.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, TS)]
pub struct HistoryEntry {
    pub conversation_id: String,
    pub ts: u64,
    pub text: String,
}
