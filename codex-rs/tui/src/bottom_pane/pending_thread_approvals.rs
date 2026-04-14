//! 📄 이 파일이 하는 일:
//!   현재 보고 있지 않은 thread들 중 승인 대기 중인 것이 있으면 짧은 목록으로 보여 준다.
//!   비유로 말하면 다른 교실에서 "선생님 확인 필요" 메모가 쌓였다고 알려 주는 복도 알림판이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/bottom_pane/mod.rs`
//!   - bottom pane 대기 승인 미리보기 UI
//!
//! 🧩 핵심 개념:
//!   - inactive thread = 지금 화면에 안 보이지만 따로 존재하는 다른 대화방
//!   - `/agent` hint = 승인 처리가 필요한 thread로 이동하는 빠른 길 안내

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::render::renderable::Renderable;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_lines;

/// Widget that lists inactive threads with outstanding approval requests.
/// 🍳 이 구조체는 승인 대기 중인 다른 thread 이름표들을 보여 주는 작은 알림판이다.
pub(crate) struct PendingThreadApprovals {
    threads: Vec<String>,
}

impl PendingThreadApprovals {
    /// 🍳 이 함수는 빈 알림판을 만든다.
    pub(crate) fn new() -> Self {
        Self {
            threads: Vec::new(),
        }
    }

    /// 🍳 이 함수는 thread 목록이 실제로 바뀐 경우에만 새 목록으로 교체한다.
    pub(crate) fn set_threads(&mut self, threads: Vec<String>) -> bool {
        if self.threads == threads {
            return false;
        }
        self.threads = threads;
        true
    }

    /// 🍳 현재 표시할 thread가 하나도 없는지 확인한다.
    pub(crate) fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn threads(&self) -> &[String] {
        &self.threads
    }

    /// 🍳 이 함수는 thread 목록을 실제 렌더링 가능한 문단으로 바꾼다.
    fn as_renderable(&self, width: u16) -> Box<dyn Renderable> {
        if self.threads.is_empty() || width < 4 {
            return Box::new(());
        }

        let mut lines = Vec::new();
        for thread in self.threads.iter().take(3) {
            let wrapped = adaptive_wrap_lines(
                std::iter::once(Line::from(format!("Approval needed in {thread}"))),
                RtOptions::new(width as usize)
                    .initial_indent(Line::from(vec!["  ".into(), "!".red().bold(), " ".into()]))
                    .subsequent_indent(Line::from("    ")),
            );
            lines.extend(wrapped);
        }

        if self.threads.len() > 3 {
            lines.push(Line::from("    ...".dim().italic()));
        }

        lines.push(
            Line::from(vec![
                "    ".into(),
                "/agent".cyan().bold(),
                " to switch threads".dim(),
            ])
            .dim(),
        );

        Paragraph::new(lines).into()
    }
}

impl Renderable for PendingThreadApprovals {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        self.as_renderable(area.width).render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.as_renderable(width).desired_height(width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;

    fn snapshot_rows(widget: &PendingThreadApprovals, width: u16) -> String {
        let height = widget.desired_height(width);
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        widget.render(Rect::new(0, 0, width, height), &mut buf);

        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn desired_height_empty() {
        let widget = PendingThreadApprovals::new();
        assert_eq!(widget.desired_height(/*width*/ 40), 0);
    }

    #[test]
    fn render_single_thread_snapshot() {
        let mut widget = PendingThreadApprovals::new();
        widget.set_threads(vec!["Robie [explorer]".to_string()]);

        assert_snapshot!(
            snapshot_rows(&widget, /*width*/ 40).replace(' ', "."),
            @r"
        ..!.Approval.needed.in.Robie.[explorer].
        ..../agent.to.switch.threads............
        "
        );
    }

    #[test]
    fn render_multiple_threads_snapshot() {
        let mut widget = PendingThreadApprovals::new();
        widget.set_threads(vec![
            "Main [default]".to_string(),
            "Robie [explorer]".to_string(),
            "Inspector".to_string(),
            "Extra agent".to_string(),
        ]);

        assert_snapshot!(
            snapshot_rows(&widget, /*width*/ 44).replace(' ', "."),
            @r"
        ..!.Approval.needed.in.Main.[default].......
        ..!.Approval.needed.in.Robie.[explorer].....
        ..!.Approval.needed.in.Inspector............
        ............................................
        ..../agent.to.switch.threads................
        "
        );
    }
}
