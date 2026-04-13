//! 📄 이 파일이 하는 일:
//!   session header에 보여 줄 현재 모델 이름 한 줄 상태를 담는다.
//!   비유로 말하면 수업 이름표 칸에 지금 어느 모델로 대화 중인지 적어 두는 작은 표찰이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/chatwidget.rs`
//!   - session header 렌더링 흐름
//!
//! 🧩 핵심 개념:
//!   - `SessionHeader` = 모델명 한 줄만 들고 있는 얇은 상태 상자

/// 🍳 session header의 현재 모델 이름표를 담는 작은 상자다.
pub(crate) struct SessionHeader {
    model: String,
}

impl SessionHeader {
    /// 🍳 초기 모델 이름으로 새 header를 만든다.
    pub(crate) fn new(model: String) -> Self {
        Self { model }
    }

    /// Updates the header's model text.
    /// 🍳 모델명이 바뀌었을 때만 새 문자열로 갈아 끼운다.
    pub(crate) fn set_model(&mut self, model: &str) {
        if self.model != model {
            self.model = model.to_string();
        }
    }
}
