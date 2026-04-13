//! Renders and formats unified-exec background session summary text.
//!
//! This module provides one canonical summary string so the bottom pane can
//! either render a dedicated footer row or reuse the same text inline in the
//! status row without duplicating copy/grammar logic.
//!
//! 📄 이 파일이 하는 일:
//!   백그라운드 unified-exec 프로세스가 몇 개 돌고 있는지 짧은 한 줄 요약으로 보여 준다.
//!   비유로 말하면 뒤에서 돌아가는 작업 기계 수와 "어디서 관리할지"를 적어 두는 상태 전광판이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/bottom_pane/mod.rs`
//!   - bottom pane footer/status row 렌더링
//!
//! 🧩 핵심 개념:
//!   - `summary_text` = footer와 status row가 같이 재사용하는 공통 안내 문장
//!   - `processes` = 현재 살아 있는 background terminal 목록

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::live_wrap::take_prefix_by_width;
use crate::render::renderable::Renderable;

/// Tracks active unified-exec processes and renders a compact summary.
/// 🍳 이 구조체는 백그라운드 unified-exec 목록을 들고 요약문을 그리는 전광판이다.
pub(crate) struct UnifiedExecFooter {
    processes: Vec<String>,
}

impl UnifiedExecFooter {
    /// 🍳 이 함수는 빈 unified-exec footer 상태를 만든다.
    pub(crate) fn new() -> Self {
        Self {
            processes: Vec::new(),
        }
    }

    /// 🍳 프로세스 목록이 바뀐 경우에만 새 목록으로 교체한다.
    pub(crate) fn set_processes(&mut self, processes: Vec<String>) -> bool {
        if self.processes == processes {
            return false;
        }
        self.processes = processes;
        true
    }

    /// 🍳 현재 보여 줄 background process가 하나도 없는지 확인한다.
    pub(crate) fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }

    /// Returns the unindented summary text used by both footer and status-row rendering.
    ///
    /// The returned string intentionally omits leading spaces and separators so
    /// callers can choose layout-specific framing (inline separator vs. row
    /// indentation). Returning `None` means there is nothing to surface.
    /// 🍳 footer/status row가 같이 쓸 수 있는 공통 요약 문장을 만든다.
    pub(crate) fn summary_text(&self) -> Option<String> {
        if self.processes.is_empty() {
            return None;
        }

        let count = self.processes.len();
        let plural = if count == 1 { "" } else { "s" };
        Some(format!(
            "{count} background terminal{plural} running · /ps to view · /stop to close"
        ))
    }

    /// 🍳 현재 폭에 맞게 잘린 렌더링용 줄들을 만든다.
    fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width < 4 {
            return Vec::new();
        }
        let Some(summary) = self.summary_text() else {
            return Vec::new();
        };
        let message = format!("  {summary}");
        let (truncated, _, _) = take_prefix_by_width(&message, width as usize);
        vec![Line::from(truncated.dim())]
    }
}

impl Renderable for UnifiedExecFooter {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        Paragraph::new(self.render_lines(area.width)).render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;

    #[test]
    fn desired_height_empty() {
        let footer = UnifiedExecFooter::new();
        assert_eq!(footer.desired_height(/*width*/ 40), 0);
    }

    #[test]
    fn render_more_sessions() {
        let mut footer = UnifiedExecFooter::new();
        footer.set_processes(vec!["rg \"foo\" src".to_string()]);
        let width = 50;
        let height = footer.desired_height(width);
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        footer.render(Rect::new(0, 0, width, height), &mut buf);
        assert_snapshot!("render_more_sessions", format!("{buf:?}"));
    }

    #[test]
    fn render_many_sessions() {
        let mut footer = UnifiedExecFooter::new();
        footer.set_processes((0..123).map(|idx| format!("cmd {idx}")).collect());
        let width = 50;
        let height = footer.desired_height(width);
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        footer.render(Rect::new(0, 0, width, height), &mut buf);
        assert_snapshot!("render_many_sessions", format!("{buf:?}"));
    }
}
