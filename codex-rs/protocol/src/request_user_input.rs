//! 📄 이 모듈이 하는 일:
//!   사용자에게 짧은 질문을 보여 주고 답을 다시 받는 도구의 타입을 정의한다.
//!   비유로 말하면 선생님이 학생에게 객관식/주관식 질문지를 건네고 답안지를 회수하는 작은 설문 양식집이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/protocol.rs`
//!   - request_user_input 도구 / UI 렌더링 코드
//!
//! 🧩 핵심 개념:
//!   - question = 질문 한 칸
//!   - response = 질문 id별 답 묶음

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// 🍳 이 구조체는 질문 한 칸에 붙는 선택지 버튼이다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestUserInputQuestionOption {
    pub label: String,
    pub description: String,
}

/// 🍳 이 구조체는 질문 한 칸 전체를 담는 설문 카드다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(rename = "isOther", default)]
    #[schemars(rename = "isOther")]
    #[ts(rename = "isOther")]
    pub is_other: bool,
    #[serde(rename = "isSecret", default)]
    #[schemars(rename = "isSecret")]
    #[ts(rename = "isSecret")]
    pub is_secret: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<RequestUserInputQuestionOption>>,
}

/// 🍳 이 구조체는 질문 여러 개를 한 번에 보내는 질문지다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestUserInputArgs {
    pub questions: Vec<RequestUserInputQuestion>,
}

/// 🍳 이 구조체는 질문 하나에 대한 실제 답 목록이다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestUserInputAnswer {
    pub answers: Vec<String>,
}

/// 🍳 이 구조체는 질문 id마다 답안을 모아 돌려주는 답안지 묶음이다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestUserInputResponse {
    pub answers: HashMap<String, RequestUserInputAnswer>,
}

/// 🍳 이 구조체는 클라이언트에 보여 줄 "질문 요청" 이벤트 카드다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestUserInputEvent {
    /// Responses API call id for the associated tool call, if available.
    pub call_id: String,
    /// Turn ID that this request belongs to.
    /// Uses `#[serde(default)]` for backwards compatibility.
    #[serde(default)]
    pub turn_id: String,
    pub questions: Vec<RequestUserInputQuestion>,
}
