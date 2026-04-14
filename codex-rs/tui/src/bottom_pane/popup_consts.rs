//! Shared popup-related constants for bottom pane widgets.
//!
//! 📄 이 파일이 하는 일:
//!   bottom pane popup들이 공통으로 쓰는 행 수 제한과 표준 힌트 문장을 모아 둔다.
//!   비유로 말하면 여러 팝업 창이 같은 크기 규칙과 같은 "Enter/Esc 사용법" 안내문을 쓰게 하는 공용 표지판 창고다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/bottom_pane`
//!   - selection/popup 계열 뷰들
//!
//! 🧩 핵심 개념:
//!   - `MAX_POPUP_ROWS` = 너무 긴 팝업이 화면을 다 덮지 않게 하는 최대 줄수
//!   - `standard_popup_hint_line` = 대부분 팝업이 재사용하는 공통 하단 안내문

use crossterm::event::KeyCode;
use ratatui::text::Line;

use crate::key_hint;

/// Maximum number of rows any popup should attempt to display.
/// Keep this consistent across all popups for a uniform feel.
/// 🍳 팝업이 한 번에 보여 줄 최대 줄 수다.
pub(crate) const MAX_POPUP_ROWS: usize = 8;

/// Standard footer hint text used by popups.
/// 🍳 대부분 팝업이 공통으로 쓰는 "확인/뒤로가기" 안내 문장을 만든다.
pub(crate) fn standard_popup_hint_line() -> Line<'static> {
    Line::from(vec![
        "Press ".into(),
        key_hint::plain(KeyCode::Enter).into(),
        " to confirm or ".into(),
        key_hint::plain(KeyCode::Esc).into(),
        " to go back".into(),
    ])
}
