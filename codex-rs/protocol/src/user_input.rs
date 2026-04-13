//! 📄 이 모듈이 하는 일:
//!   사용자가 보낼 수 있는 입력 조각(text, image, skill, mention 등)의 공통 타입을 정의한다.
//!   비유로 말하면 가방 안에 메모지, 사진, 스티커를 각각 어떤 칸에 넣을지 정해 둔 입력 정리함이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/items.rs`
//!   - 요청 직렬화 / UI 입력 조립 코드
//!
//! 🧩 핵심 개념:
//!   - `UserInput` = 사용자가 보낼 수 있는 입력 종류 모음
//!   - `TextElement` = 긴 텍스트 안에서 특별 취급할 구간을 가리키는 투명 스티커

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// Conservative cap so one user message cannot monopolize a large context window.
/// 🍳 이 숫자는 사용자 입력 한 번이 컨텍스트 창을 혼자 다 차지하지 못하게 막는 최대 길이 울타리다.
pub const MAX_USER_INPUT_TEXT_CHARS: usize = 1 << 20;

/// User input
/// 🍳 이 enum은 사용자가 보낼 수 있는 입력 재료 종류를 담은 메뉴판이다.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInput {
    Text {
        text: String,
        /// UI-defined spans within `text` that should be treated as special elements.
        /// These are byte ranges into the UTF-8 `text` buffer and are used to render
        /// or persist rich input markers (e.g., image placeholders) across history
        /// and resume without mutating the literal text.
        #[serde(default)]
        text_elements: Vec<TextElement>,
    },
    /// Pre‑encoded data: URI image.
    Image { image_url: String },

    /// Local image path provided by the user.  This will be converted to an
    /// `Image` variant (base64 data URL) during request serialization.
    LocalImage { path: std::path::PathBuf },

    /// Skill selected by the user (name + path to SKILL.md).
    Skill {
        name: String,
        path: std::path::PathBuf,
    },
    /// Explicit structured mention selected by the user.
    ///
    /// `path` identifies the exact mention target, for example
    /// `app://<connector-id>` or `plugin://<plugin-name>@<marketplace-name>`.
    Mention { name: String, path: String },
}

/// 🍳 이 구조체는 텍스트 안 특별 구간의 범위와 표시 문구를 담는 표식 카드다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, TS, JsonSchema)]
pub struct TextElement {
    /// Byte range in the parent `text` buffer that this element occupies.
    pub byte_range: ByteRange,
    /// Optional human-readable placeholder for the element, displayed in the UI.
    placeholder: Option<String>,
}

impl TextElement {
    /// 🍳 이 함수는 범위와 placeholder를 받아 새 표식 카드를 만든다.
    pub fn new(byte_range: ByteRange, placeholder: Option<String>) -> Self {
        Self {
            byte_range,
            placeholder,
        }
    }

    /// Returns a copy of this element with a remapped byte range.
    ///
    /// The placeholder is preserved as-is; callers must ensure the new range
    /// still refers to the same logical element (and same placeholder)
    /// within the new text.
    /// 🍳 이 함수는 표식 카드의 범위를 새 좌표계로 옮긴 복사본을 만든다.
    pub fn map_range<F>(&self, map: F) -> Self
    where
        F: FnOnce(ByteRange) -> ByteRange,
    {
        Self {
            byte_range: map(self.byte_range),
            placeholder: self.placeholder.clone(),
        }
    }

    /// 🍳 이 함수는 placeholder 문구를 바꿔 끼운다.
    pub fn set_placeholder(&mut self, placeholder: Option<String>) {
        self.placeholder = placeholder;
    }

    /// Returns the stored placeholder without falling back to the text buffer.
    ///
    /// This must only be used inside `From<TextElement>` implementations on equivalent
    /// protocol types where the source text is unavailable. Prefer `placeholder(text)`
    /// everywhere else.
    #[doc(hidden)]
    pub fn _placeholder_for_conversion_only(&self) -> Option<&str> {
        self.placeholder.as_deref()
    }

    /// 🍳 이 함수는 저장된 placeholder가 있으면 그걸 쓰고,
    ///   없으면 원문 텍스트 범위를 잘라 임시 placeholder처럼 돌려준다.
    pub fn placeholder<'a>(&'a self, text: &'a str) -> Option<&'a str> {
        self.placeholder
            .as_deref()
            .or_else(|| text.get(self.byte_range.start..self.byte_range.end))
    }
}

/// 🍳 이 구조체는 UTF-8 텍스트 안 시작/끝 바이트 좌표 두 개를 묶은 자다.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, TS, JsonSchema)]
pub struct ByteRange {
    /// Start byte offset (inclusive) within the UTF-8 text buffer.
    pub start: usize,
    /// End byte offset (exclusive) within the UTF-8 text buffer.
    pub end: usize,
}

impl From<std::ops::Range<usize>> for ByteRange {
    fn from(range: std::ops::Range<usize>) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}
