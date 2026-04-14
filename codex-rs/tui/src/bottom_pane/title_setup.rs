//! Terminal title configuration view for customizing the terminal window/tab title.
//!
//! This module provides an interactive picker for selecting which items appear
//! in the terminal title. Users can:
//!
//! - Select items
//! - Reorder items
//! - Preview the rendered title
//!
//! 📄 이 파일이 하는 일:
//!   터미널 제목줄에 어떤 항목을 보여 줄지 고르고 순서를 바꾸는 설정 popup을 제공한다.
//!   비유로 말하면 문패에 프로젝트명, 브랜치, 모델명을 어떤 순서로 붙일지 고르는 문패 편집기다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/bottom_pane`
//!   - terminal title 설정 흐름
//!
//! 🧩 핵심 개념:
//!   - `TerminalTitleItem` = 제목줄에 올릴 수 있는 정보 조각
//!   - preview = 현재 선택 조합이 실제 제목으로 어떻게 보일지 미리 보는 샘플

use itertools::Itertools;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use strum::IntoEnumIterator;
use strum_macros::Display;
use strum_macros::EnumIter;
use strum_macros::EnumString;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::bottom_pane::multi_select_picker::MultiSelectItem;
use crate::bottom_pane::multi_select_picker::MultiSelectPicker;
use crate::render::renderable::Renderable;

/// Available items that can be displayed in the terminal title.
///
/// Variants serialize to kebab-case identifiers (e.g. `AppName` -> `"app-name"`)
/// via strum. These identifiers are persisted in user config files, so renaming
/// or removing a variant is a breaking config change.
/// 🍳 이 enum은 터미널 제목에 넣을 수 있는 항목 종류표다.
#[derive(EnumIter, EnumString, Display, Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum TerminalTitleItem {
    /// Codex app name.
    AppName,
    /// Project root name, or a compact cwd fallback.
    Project,
    /// Animated task spinner while active.
    Spinner,
    /// Compact runtime status text.
    Status,
    /// Current thread title (if available).
    Thread,
    /// Current git branch (if available).
    GitBranch,
    /// Current model name.
    Model,
    /// Latest checklist task progress from `update_plan` (if available).
    TaskProgress,
}

impl TerminalTitleItem {
    /// 🍳 각 제목 항목을 사람이 읽기 쉬운 설명문으로 바꾼다.
    pub(crate) fn description(self) -> &'static str {
        match self {
            TerminalTitleItem::AppName => "Codex app name",
            TerminalTitleItem::Project => "Project name (falls back to current directory name)",
            TerminalTitleItem::Spinner => {
                "Animated task spinner (omitted while idle or when animations are off)"
            }
            TerminalTitleItem::Status => "Compact session status text (Ready, Working, Thinking)",
            TerminalTitleItem::Thread => "Current thread title (omitted until available)",
            TerminalTitleItem::GitBranch => "Current Git branch (omitted when unavailable)",
            TerminalTitleItem::Model => "Current model name",
            TerminalTitleItem::TaskProgress => {
                "Latest task progress from update_plan (omitted until available)"
            }
        }
    }

    /// Example text used when previewing the title picker.
    ///
    /// These are illustrative sample values, not live data from the current
    /// session.
    /// 🍳 미리보기용 예시 문자열을 돌려준다.
    pub(crate) fn preview_example(self) -> &'static str {
        match self {
            TerminalTitleItem::AppName => "codex",
            TerminalTitleItem::Project => "my-project",
            TerminalTitleItem::Spinner => "⠋",
            TerminalTitleItem::Status => "Working",
            TerminalTitleItem::Thread => "Investigate flaky test",
            TerminalTitleItem::GitBranch => "feat/awesome-feature",
            TerminalTitleItem::Model => "gpt-5.2-codex",
            TerminalTitleItem::TaskProgress => "Tasks 2/5",
        }
    }

    /// Returns the separator to place before this item in a rendered title.
    ///
    /// The spinner gets a plain space on either side so it reads as
    /// `my-project <spinner> Working` rather than `my-project | <spinner> | Working`.
    /// All other adjacent items are joined with ` | `.
    /// 🍳 이전 항목과 현재 항목 사이에 어떤 구분자를 넣을지 정한다.
    pub(crate) fn separator_from_previous(self, previous: Option<Self>) -> &'static str {
        match previous {
            None => "",
            Some(previous)
                if previous == TerminalTitleItem::Spinner || self == TerminalTitleItem::Spinner =>
            {
                " "
            }
            Some(_) => " | ",
        }
    }
}

/// 🍳 저장된 id 문자열 목록을 모두 올바른 `TerminalTitleItem` enum으로 바꿔 본다.
fn parse_terminal_title_items<T>(ids: impl Iterator<Item = T>) -> Option<Vec<TerminalTitleItem>>
where
    T: AsRef<str>,
{
    // Treat parsing as all-or-nothing so preview/confirm callbacks never emit
    // a partially interpreted ordering. Invalid ids are ignored when building
    // the picker, but once the user is interacting with the picker we only want
    // to persist or preview a fully valid selection.
    ids.map(|id| id.as_ref().parse::<TerminalTitleItem>())
        .collect::<Result<Vec<_>, _>>()
        .ok()
}

/// Interactive view for configuring terminal-title items.
/// 🍳 `MultiSelectPicker`를 감싸 제목줄 전용 동작을 붙인 설정 뷰다.
pub(crate) struct TerminalTitleSetupView {
    picker: MultiSelectPicker,
}

impl TerminalTitleSetupView {
    /// Creates the terminal-title picker, preserving the configured item order first.
    ///
    /// Unknown configured ids are skipped here instead of surfaced inline. The
    /// main TUI still warns about them when rendering the actual title, but the
    /// picker itself only exposes the selectable items it can meaningfully
    /// preview and persist.
    /// 🍳 현재 설정을 앞쪽에 배치해 terminal title 설정 popup을 만든다.
    pub(crate) fn new(title_items: Option<&[String]>, app_event_tx: AppEventSender) -> Self {
        let selected_items = title_items
            .into_iter()
            .flatten()
            .filter_map(|id| id.parse::<TerminalTitleItem>().ok())
            .unique()
            .collect_vec();
        let selected_set = selected_items
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let items = selected_items
            .into_iter()
            .map(|item| Self::title_select_item(item, /*enabled*/ true))
            .chain(
                TerminalTitleItem::iter()
                    .filter(|item| !selected_set.contains(item))
                    .map(|item| Self::title_select_item(item, /*enabled*/ false)),
            )
            .collect();

        Self {
            picker: MultiSelectPicker::builder(
                "Configure Terminal Title".to_string(),
                Some("Select which items to display in the terminal title.".to_string()),
                app_event_tx,
            )
            .instructions(vec![
                "Use ↑↓ to navigate, ←→ to move, space to select, enter to confirm, esc to cancel."
                    .into(),
            ])
            .items(items)
            .enable_ordering()
            .on_preview(|items| {
                let items = parse_terminal_title_items(
                    items
                        .iter()
                        .filter(|item| item.enabled)
                        .map(|item| item.id.as_str()),
                )?;
                let mut preview = String::new();
                let mut previous = None;
                for item in items.iter().copied() {
                    preview.push_str(item.separator_from_previous(previous));
                    preview.push_str(item.preview_example());
                    previous = Some(item);
                }
                if preview.is_empty() {
                    None
                } else {
                    Some(Line::from(preview))
                }
            })
            .on_change(|items, app_event| {
                let Some(items) = parse_terminal_title_items(
                    items
                        .iter()
                        .filter(|item| item.enabled)
                        .map(|item| item.id.as_str()),
                ) else {
                    return;
                };
                app_event.send(AppEvent::TerminalTitleSetupPreview { items });
            })
            .on_confirm(|ids, app_event| {
                let Some(items) = parse_terminal_title_items(ids.iter().map(String::as_str)) else {
                    return;
                };
                app_event.send(AppEvent::TerminalTitleSetup { items });
            })
            .on_cancel(|app_event| {
                app_event.send(AppEvent::TerminalTitleSetupCancelled);
            })
            .build(),
        }
    }

    /// 🍳 제목 항목 하나를 multi-select picker가 쓰는 행 데이터로 바꾼다.
    fn title_select_item(item: TerminalTitleItem, enabled: bool) -> MultiSelectItem {
        MultiSelectItem {
            id: item.to_string(),
            name: item.to_string(),
            description: Some(item.description().to_string()),
            enabled,
        }
    }
}

impl BottomPaneView for TerminalTitleSetupView {
    fn handle_key_event(&mut self, key_event: crossterm::event::KeyEvent) {
        self.picker.handle_key_event(key_event);
    }

    fn is_complete(&self) -> bool {
        self.picker.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.picker.close();
        CancellationEvent::Handled
    }
}

impl Renderable for TerminalTitleSetupView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.picker.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.picker.desired_height(width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    fn render_lines(view: &TerminalTitleSetupView, width: u16) -> String {
        let height = view.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let lines: Vec<String> = (0..area.height)
            .map(|row| {
                let mut line = String::new();
                for col in 0..area.width {
                    let symbol = buf[(area.x + col, area.y + row)].symbol();
                    if symbol.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(symbol);
                    }
                }
                line
            })
            .collect();
        lines.join("\n")
    }

    #[test]
    fn renders_title_setup_popup() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let selected = [
            "project".to_string(),
            "spinner".to_string(),
            "status".to_string(),
            "thread".to_string(),
        ];
        let view = TerminalTitleSetupView::new(Some(&selected), tx);
        assert_snapshot!(
            "terminal_title_setup_basic",
            render_lines(&view, /*width*/ 84)
        );
    }

    #[test]
    fn parse_terminal_title_items_preserves_order() {
        let items =
            parse_terminal_title_items(["project", "spinner", "status", "thread"].into_iter());
        assert_eq!(
            items,
            Some(vec![
                TerminalTitleItem::Project,
                TerminalTitleItem::Spinner,
                TerminalTitleItem::Status,
                TerminalTitleItem::Thread,
            ])
        );
    }

    #[test]
    fn parse_terminal_title_items_rejects_invalid_ids() {
        let items = parse_terminal_title_items(["project", "not-a-title-item"].into_iter());
        assert_eq!(items, None);
    }

    #[test]
    fn parse_terminal_title_items_accepts_kebab_case_variants() {
        let items = parse_terminal_title_items(["app-name", "git-branch"].into_iter());
        assert_eq!(
            items,
            Some(vec![
                TerminalTitleItem::AppName,
                TerminalTitleItem::GitBranch,
            ])
        );
    }
}
