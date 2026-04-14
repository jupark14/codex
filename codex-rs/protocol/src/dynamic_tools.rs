//! 📄 이 모듈이 하는 일:
//!   런타임에 늦게 로드되는 도구들의 스펙, 호출 요청, 응답 모양을 정의한다.
//!   비유로 말하면 행사 당일에 추가 참가한 부스를 위해 이름표, 신청서, 결과표 양식을 준비해 두는 안내 데스크다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/lib.rs`
//!   - dynamic tool registry / app-server 연동 코드
//!
//! 🧩 핵심 개념:
//!   - `DynamicToolSpec` = 도구 소개 카드
//!   - legacy `expose_to_context` = 예전 필드를 새 `defer_loading` 의미로 뒤집어 읽는 호환 다리

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

/// 🍳 이 구조체는 동적으로 붙는 도구의 설명서다.
///   이름/설명/입력 스키마 → 도구 소개 카드
#[derive(Debug, Clone, Serialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DynamicToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: JsonValue,
    #[serde(default)]
    pub defer_loading: bool,
}

/// 🍳 이 구조체는 dynamic tool 한 번 호출해 달라는 요청서다.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DynamicToolCallRequest {
    pub call_id: String,
    pub turn_id: String,
    pub tool: String,
    pub arguments: JsonValue,
}

/// 🍳 이 구조체는 dynamic tool이 돌려준 결과 꾸러미다.
///   content item들 + 성공 여부 → 호출 응답
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DynamicToolResponse {
    pub content_items: Vec<DynamicToolCallOutputContentItem>,
    pub success: bool,
}

/// 🍳 이 enum은 도구 응답 안에 텍스트를 넣을지, 이미지를 넣을지 고르는 상자 칸막이다.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type")]
pub enum DynamicToolCallOutputContentItem {
    #[serde(rename_all = "camelCase")]
    InputText { text: String },
    #[serde(rename_all = "camelCase")]
    InputImage { image_url: String },
}

/// 🍳 이 구조체는 역직렬화할 때만 쓰는 임시 접수용 봉투다.
///   새 필드와 옛 필드를 함께 받아 마지막에 `DynamicToolSpec`로 정리한다.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DynamicToolSpecDe {
    name: String,
    description: String,
    input_schema: JsonValue,
    defer_loading: Option<bool>,
    expose_to_context: Option<bool>,
}

impl<'de> Deserialize<'de> for DynamicToolSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let DynamicToolSpecDe {
            name,
            description,
            input_schema,
            defer_loading,
            expose_to_context,
        } = DynamicToolSpecDe::deserialize(deserializer)?;

        Ok(Self {
            name,
            description,
            input_schema,
            // 🔄 새 `deferLoading` 값이 있으면 그걸 그대로 쓰고,
            //    없으면 예전 `exposeToContext` 의미를 뒤집어서 호환시킨다.
            defer_loading: defer_loading
                .unwrap_or_else(|| expose_to_context.map(|visible| !visible).unwrap_or(false)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DynamicToolSpec;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn dynamic_tool_spec_deserializes_defer_loading() {
        let value = json!({
            "name": "lookup_ticket",
            "description": "Fetch a ticket",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                }
            },
            "deferLoading": true,
        });

        let actual: DynamicToolSpec = serde_json::from_value(value).expect("deserialize");

        assert_eq!(
            actual,
            DynamicToolSpec {
                name: "lookup_ticket".to_string(),
                description: "Fetch a ticket".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" }
                    }
                }),
                defer_loading: true,
            }
        );
    }

    #[test]
    fn dynamic_tool_spec_legacy_expose_to_context_inverts_to_defer_loading() {
        let value = json!({
            "name": "lookup_ticket",
            "description": "Fetch a ticket",
            "inputSchema": {
                "type": "object",
                "properties": {}
            },
            "exposeToContext": false,
        });

        let actual: DynamicToolSpec = serde_json::from_value(value).expect("deserialize");

        assert!(actual.defer_loading);
    }
}
