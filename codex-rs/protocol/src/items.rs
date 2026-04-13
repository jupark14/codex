//! 📄 이 모듈이 하는 일:
//!   한 턴 안에서 오가는 사용자 메시지, agent 메시지, 계획, 검색 같은 항목을 공통 타입으로 정리한다.
//!   비유로 말하면 수업 시간에 나오는 말풍선, 메모지, 체크리스트, 안내 방송을 같은 공책 형식으로 정리한 기록장이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/protocol.rs`
//!   - turn history / UI 렌더링 / 직렬화 코드
//!
//! 🧩 핵심 개념:
//!   - `TurnItem` = 턴 안에서 등장하는 큰 카드 종류
//!   - legacy event 변환 = 새 카드 형식을 예전 이벤트 모양으로도 다시 적어 주는 호환 다리

use crate::memory_citation::MemoryCitation;
use crate::models::ContentItem;
use crate::models::MessagePhase;
use crate::models::ResponseItem;
use crate::models::WebSearchAction;
use crate::protocol::AgentMessageEvent;
use crate::protocol::AgentReasoningEvent;
use crate::protocol::AgentReasoningRawContentEvent;
use crate::protocol::ContextCompactedEvent;
use crate::protocol::EventMsg;
use crate::protocol::ImageGenerationEndEvent;
use crate::protocol::UserMessageEvent;
use crate::protocol::WebSearchEndEvent;
use crate::user_input::ByteRange;
use crate::user_input::TextElement;
use crate::user_input::UserInput;
use quick_xml::de::from_str as from_xml_str;
use quick_xml::se::to_string as to_xml_string;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// 🍳 이 enum은 한 턴 안에서 등장할 수 있는 큰 카드 종류 모음이다.
#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema)]
#[serde(tag = "type")]
#[ts(tag = "type")]
pub enum TurnItem {
    UserMessage(UserMessageItem),
    HookPrompt(HookPromptItem),
    AgentMessage(AgentMessageItem),
    Plan(PlanItem),
    Reasoning(ReasoningItem),
    WebSearch(WebSearchItem),
    ImageGeneration(ImageGenerationItem),
    ContextCompaction(ContextCompactionItem),
}

/// 🍳 이 구조체는 사용자가 보낸 입력 조각 묶음을 담는 카드다.
#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema)]
pub struct UserMessageItem {
    pub id: String,
    pub content: Vec<UserInput>,
}

/// 🍳 이 구조체는 hook이 끼어든 프롬프트 조각들을 한 묶음으로 담는다.
#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq, Eq)]
pub struct HookPromptItem {
    pub id: String,
    pub fragments: Vec<HookPromptFragment>,
}

/// 🍳 이 구조체는 hook 문장 한 조각과 그 실행 id를 붙여 둔 메모지다.
#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct HookPromptFragment {
    pub text: String,
    pub hook_run_id: String,
}

/// 🍳 이 구조체는 XML `<hook_prompt>` 한 조각을 읽고 쓰는 임시 봉투다.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename = "hook_prompt")]
struct HookPromptXml {
    #[serde(rename = "@hook_run_id")]
    hook_run_id: String,
    #[serde(rename = "$text")]
    text: String,
}

/// 🍳 이 enum은 agent 메시지 안의 내용 칸 종류를 고른다.
#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema)]
#[serde(tag = "type")]
#[ts(tag = "type")]
pub enum AgentMessageContent {
    Text { text: String },
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema)]
/// Assistant-authored message payload used in turn-item streams.
///
/// `phase` is optional because not all providers/models emit it. Consumers
/// should use it when present, but retain legacy completion semantics when it
/// is `None`.
pub struct AgentMessageItem {
    pub id: String,
    pub content: Vec<AgentMessageContent>,
    /// Optional phase metadata carried through from `ResponseItem::Message`.
    ///
    /// This is currently used by TUI rendering to distinguish mid-turn
    /// commentary from a final answer and avoid status-indicator jitter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub phase: Option<MessagePhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub memory_citation: Option<MemoryCitation>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema)]
pub struct PlanItem {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema)]
pub struct ReasoningItem {
    pub id: String,
    pub summary_text: Vec<String>,
    #[serde(default)]
    pub raw_content: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq)]
pub struct WebSearchItem {
    pub id: String,
    pub query: String,
    pub action: WebSearchAction,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq)]
pub struct ImageGenerationItem {
    pub id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub revised_prompt: Option<String>,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub saved_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema)]
pub struct ContextCompactionItem {
    pub id: String,
}

impl ContextCompactionItem {
    /// 🍳 이 함수는 새 context compaction 카드를 UUID 번호와 함께 만든다.
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// 🍳 이 함수는 새 카드 형식을 예전 `EventMsg` 형식으로 바꿔 호환시킨다.
    pub fn as_legacy_event(&self) -> EventMsg {
        EventMsg::ContextCompacted(ContextCompactedEvent {})
    }
}

impl Default for ContextCompactionItem {
    fn default() -> Self {
        Self::new()
    }
}

impl UserMessageItem {
    /// 🍳 이 함수는 여러 `UserInput` 조각을 새 사용자 메시지 카드로 묶는다.
    pub fn new(content: &[UserInput]) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content: content.to_vec(),
        }
    }

    /// 🍳 이 함수는 새 사용자 메시지 카드를 예전 legacy 이벤트 모양으로 펼친다.
    pub fn as_legacy_event(&self) -> EventMsg {
        // Legacy user-message events flatten only text inputs into `message` and
        // rebase text element ranges onto that concatenated text.
        EventMsg::UserMessage(UserMessageEvent {
            message: self.message(),
            images: Some(self.image_urls()),
            local_images: self.local_image_paths(),
            text_elements: self.text_elements(),
        })
    }

    /// 🍳 이 함수는 여러 입력 조각 중 텍스트만 이어 붙여 최종 메시지 문장으로 만든다.
    pub fn message(&self) -> String {
        self.content
            .iter()
            .map(|c| match c {
                UserInput::Text { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect::<Vec<String>>()
            .join("")
    }

    /// 🍳 이 함수는 각 텍스트 조각 안의 범위를
    ///   합쳐진 전체 메시지 좌표계로 다시 맞춘다.
    pub fn text_elements(&self) -> Vec<TextElement> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        for input in &self.content {
            if let UserInput::Text {
                text,
                text_elements,
            } = input
            {
                // Text element ranges are relative to each text chunk; offset them so they align
                // with the concatenated message returned by `message()`.
                for elem in text_elements {
                    let byte_range = ByteRange {
                        start: offset + elem.byte_range.start,
                        end: offset + elem.byte_range.end,
                    };
                    out.push(TextElement::new(
                        byte_range,
                        elem.placeholder(text).map(str::to_string),
                    ));
                }
                offset += text.len();
            }
        }
        out
    }

    /// 🍳 이 함수는 입력 조각들 중 원격 이미지 URL만 모아 꺼낸다.
    pub fn image_urls(&self) -> Vec<String> {
        self.content
            .iter()
            .filter_map(|c| match c {
                UserInput::Image { image_url } => Some(image_url.clone()),
                _ => None,
            })
            .collect()
    }

    /// 🍳 이 함수는 로컬 파일로 붙은 이미지 경로만 따로 모은다.
    pub fn local_image_paths(&self) -> Vec<std::path::PathBuf> {
        self.content
            .iter()
            .filter_map(|c| match c {
                UserInput::LocalImage { path } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }
}

impl HookPromptItem {
    /// 🍳 이 함수는 hook 조각 여러 개를 하나의 hook prompt 카드로 묶는다.
    pub fn from_fragments(id: Option<&String>, fragments: Vec<HookPromptFragment>) -> Self {
        Self {
            id: id
                .cloned()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            fragments,
        }
    }
}

impl HookPromptFragment {
    /// 🍳 이 함수는 텍스트 한 조각과 hook 실행 id를 받아 단일 fragment를 만든다.
    pub fn from_single_hook(text: impl Into<String>, hook_run_id: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            hook_run_id: hook_run_id.into(),
        }
    }
}

/// 🍳 이 함수는 hook fragment들을 `ResponseItem` 한 장으로 직렬화해 모델 입력에 넣는다.
pub fn build_hook_prompt_message(fragments: &[HookPromptFragment]) -> Option<ResponseItem> {
    let content = fragments
        .iter()
        .filter(|fragment| !fragment.hook_run_id.trim().is_empty())
        .filter_map(|fragment| {
            serialize_hook_prompt_fragment(&fragment.text, &fragment.hook_run_id)
                .map(|text| ContentItem::InputText { text })
        })
        .collect::<Vec<_>>();

    if content.is_empty() {
        return None;
    }

    Some(ResponseItem::Message {
        id: Some(uuid::Uuid::new_v4().to_string()),
        role: "user".to_string(),
        content,
        end_turn: None,
        phase: None,
    })
}

/// 🍳 이 함수는 `ResponseItem` 안의 텍스트 조각들이 모두 hook prompt XML인지 읽어 보고,
///   맞으면 `HookPromptItem`으로 다시 조립한다.
pub fn parse_hook_prompt_message(
    id: Option<&String>,
    content: &[ContentItem],
) -> Option<HookPromptItem> {
    let fragments = content
        .iter()
        .map(|content_item| {
            let ContentItem::InputText { text } = content_item else {
                return None;
            };
            parse_hook_prompt_fragment(text)
        })
        .collect::<Option<Vec<_>>>()?;

    if fragments.is_empty() {
        return None;
    }

    Some(HookPromptItem::from_fragments(id, fragments))
}

/// 🍳 이 함수는 XML 문자열 한 조각을 hook fragment 카드로 되돌린다.
pub fn parse_hook_prompt_fragment(text: &str) -> Option<HookPromptFragment> {
    let trimmed = text.trim();
    let HookPromptXml { text, hook_run_id } = from_xml_str::<HookPromptXml>(trimmed).ok()?;
    if hook_run_id.trim().is_empty() {
        return None;
    }

    Some(HookPromptFragment { text, hook_run_id })
}

/// 🍳 이 함수는 텍스트와 run id를 XML `<hook_prompt>` 한 조각으로 감싼다.
fn serialize_hook_prompt_fragment(text: &str, hook_run_id: &str) -> Option<String> {
    if hook_run_id.trim().is_empty() {
        return None;
    }
    to_xml_string(&HookPromptXml {
        text: text.to_string(),
        hook_run_id: hook_run_id.to_string(),
    })
    .ok()
}

impl AgentMessageItem {
    /// 🍳 이 함수는 agent 메시지 내용 조각들로 새 메시지 카드를 만든다.
    pub fn new(content: &[AgentMessageContent]) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content: content.to_vec(),
            phase: None,
            memory_citation: None,
        }
    }

    /// 🍳 이 함수는 agent 메시지 카드를 예전 `EventMsg` 여러 장으로 펼친다.
    pub fn as_legacy_events(&self) -> Vec<EventMsg> {
        self.content
            .iter()
            .map(|c| match c {
                AgentMessageContent::Text { text } => EventMsg::AgentMessage(AgentMessageEvent {
                    message: text.clone(),
                    phase: self.phase.clone(),
                    memory_citation: self.memory_citation.clone(),
                }),
            })
            .collect()
    }
}

impl ReasoningItem {
    /// 🍳 이 함수는 reasoning 요약/원문을 설정에 맞춰 legacy 이벤트 줄들로 바꾼다.
    pub fn as_legacy_events(&self, show_raw_agent_reasoning: bool) -> Vec<EventMsg> {
        let mut events = Vec::new();
        for summary in &self.summary_text {
            events.push(EventMsg::AgentReasoning(AgentReasoningEvent {
                text: summary.clone(),
            }));
        }

        if show_raw_agent_reasoning {
            for entry in &self.raw_content {
                events.push(EventMsg::AgentReasoningRawContent(
                    AgentReasoningRawContentEvent {
                        text: entry.clone(),
                    },
                ));
            }
        }

        events
    }
}

impl WebSearchItem {
    /// 🍳 이 함수는 web search 카드 한 장을 예전 종료 이벤트 한 장으로 바꾼다.
    pub fn as_legacy_event(&self) -> EventMsg {
        EventMsg::WebSearchEnd(WebSearchEndEvent {
            call_id: self.id.clone(),
            query: self.query.clone(),
            action: self.action.clone(),
        })
    }
}

impl ImageGenerationItem {
    /// 🍳 이 함수는 이미지 생성 카드도 예전 완료 이벤트 형식으로 변환한다.
    pub fn as_legacy_event(&self) -> EventMsg {
        EventMsg::ImageGenerationEnd(ImageGenerationEndEvent {
            call_id: self.id.clone(),
            status: self.status.clone(),
            revised_prompt: self.revised_prompt.clone(),
            result: self.result.clone(),
            saved_path: self.saved_path.clone(),
        })
    }
}

impl TurnItem {
    /// 🍳 이 함수는 turn item이 어떤 종류든 공통 id만 꺼내는 번호표 읽기 함수다.
    pub fn id(&self) -> String {
        match self {
            TurnItem::UserMessage(item) => item.id.clone(),
            TurnItem::HookPrompt(item) => item.id.clone(),
            TurnItem::AgentMessage(item) => item.id.clone(),
            TurnItem::Plan(item) => item.id.clone(),
            TurnItem::Reasoning(item) => item.id.clone(),
            TurnItem::WebSearch(item) => item.id.clone(),
            TurnItem::ImageGeneration(item) => item.id.clone(),
            TurnItem::ContextCompaction(item) => item.id.clone(),
        }
    }

    /// 🍳 이 함수는 각 turn item을 예전 이벤트 줄 묶음으로 바꾼다.
    pub fn as_legacy_events(&self, show_raw_agent_reasoning: bool) -> Vec<EventMsg> {
        match self {
            TurnItem::UserMessage(item) => vec![item.as_legacy_event()],
            TurnItem::HookPrompt(_) => Vec::new(),
            TurnItem::AgentMessage(item) => item.as_legacy_events(),
            TurnItem::Plan(_) => Vec::new(),
            TurnItem::WebSearch(item) => vec![item.as_legacy_event()],
            TurnItem::ImageGeneration(item) => vec![item.as_legacy_event()],
            TurnItem::Reasoning(item) => item.as_legacy_events(show_raw_agent_reasoning),
            TurnItem::ContextCompaction(item) => vec![item.as_legacy_event()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn hook_prompt_roundtrips_multiple_fragments() {
        let original = vec![
            HookPromptFragment::from_single_hook("Retry with care & joy.", "hook-run-1"),
            HookPromptFragment::from_single_hook("Then summarize cleanly.", "hook-run-2"),
        ];
        let message = build_hook_prompt_message(&original).expect("hook prompt");

        let ResponseItem::Message { content, .. } = message else {
            panic!("expected hook prompt message");
        };

        let parsed = parse_hook_prompt_message(/*id*/ None, &content).expect("parsed hook prompt");
        assert_eq!(parsed.fragments, original);
    }

    #[test]
    fn hook_prompt_parses_legacy_single_hook_run_id() {
        let parsed = parse_hook_prompt_fragment(
            r#"<hook_prompt hook_run_id="hook-run-1">Retry with tests.</hook_prompt>"#,
        )
        .expect("legacy hook prompt");

        assert_eq!(
            parsed,
            HookPromptFragment {
                text: "Retry with tests.".to_string(),
                hook_run_id: "hook-run-1".to_string(),
            }
        );
    }
}
