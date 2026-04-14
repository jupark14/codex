//! 📄 이 파일이 하는 일:
//!   `@` 파일 검색 popup의 질의, 결과 목록, 선택 스크롤 상태를 관리한다.
//!   비유로 말하면 서랍 이름을 입력하면 맞는 파일 카드만 보여 주고 위아래로 고르게 해 주는 카드 정리함이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/bottom_pane/chat_composer.rs`
//!   - 파일 검색 popup 렌더링 흐름
//!
//! 🧩 핵심 개념:
//!   - `pending_query` = 아직 결과를 기다리는 최신 검색어
//!   - `display_query` = 지금 화면에 보이는 결과가 어떤 검색어 기준인지 알려 주는 꼬리표

use std::path::PathBuf;

use codex_file_search::FileMatch;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::WidgetRef;

use crate::render::Insets;
use crate::render::RectExt;

use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::render_rows;

/// Visual state for the file-search popup.
/// 🍳 이 구조체는 파일 검색 popup의 현재 검색어/결과/선택 상태를 들고 있다.
pub(crate) struct FileSearchPopup {
    /// Query corresponding to the `matches` currently shown.
    display_query: String,
    /// Latest query typed by the user. May differ from `display_query` when
    /// a search is still in-flight.
    pending_query: String,
    /// When `true` we are still waiting for results for `pending_query`.
    waiting: bool,
    /// Cached matches; paths relative to the search dir.
    matches: Vec<FileMatch>,
    /// Shared selection/scroll state.
    state: ScrollState,
}

impl FileSearchPopup {
    /// 🍳 이 함수는 빈 파일 검색 popup을 만든다.
    pub(crate) fn new() -> Self {
        Self {
            display_query: String::new(),
            pending_query: String::new(),
            waiting: true,
            matches: Vec::new(),
            state: ScrollState::new(),
        }
    }

    /// Update the query and reset state to *waiting*.
    /// 🍳 이 함수는 새 검색어를 기억하고 "결과 기다리는 중" 상태로 바꾼다.
    pub(crate) fn set_query(&mut self, query: &str) {
        if query == self.pending_query {
            return;
        }

        self.pending_query.clear();
        self.pending_query.push_str(query);

        self.waiting = true; // waiting for new results
    }

    /// Put the popup into an "idle" state used for an empty query (just "@").
    /// Shows a hint instead of matches until the user types more characters.
    /// 🍳 이 함수는 검색어가 비었을 때 결과 대신 안내 문구를 보여 주는 idle 상태로 돌린다.
    pub(crate) fn set_empty_prompt(&mut self) {
        self.display_query.clear();
        self.pending_query.clear();
        self.waiting = false;
        self.matches.clear();
        // Reset selection/scroll state when showing the empty prompt.
        self.state.reset();
    }

    /// Replace matches when a `FileSearchResult` arrives.
    /// Replace matches. Only applied when `query` matches `pending_query`.
    /// 🍳 이 함수는 현재 기다리던 검색어와 응답 검색어가 같을 때만 결과 목록을 갈아 끼운다.
    pub(crate) fn set_matches(&mut self, query: &str, matches: Vec<FileMatch>) {
        if query != self.pending_query {
            return; // stale
        }

        self.display_query = query.to_string();
        self.matches = matches.into_iter().take(MAX_POPUP_ROWS).collect();
        self.waiting = false;
        let len = self.matches.len();
        self.state.clamp_selection(len);
        self.state.ensure_visible(len, len.min(MAX_POPUP_ROWS));
    }

    /// Move selection cursor up.
    /// 🍳 선택을 위쪽 파일로 한 칸 움직인다.
    pub(crate) fn move_up(&mut self) {
        let len = self.matches.len();
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, len.min(MAX_POPUP_ROWS));
    }

    /// Move selection cursor down.
    /// 🍳 선택을 아래쪽 파일로 한 칸 움직인다.
    pub(crate) fn move_down(&mut self) {
        let len = self.matches.len();
        self.state.move_down_wrap(len);
        self.state.ensure_visible(len, len.min(MAX_POPUP_ROWS));
    }

    /// 🍳 현재 선택된 파일 경로가 있으면 돌려준다.
    pub(crate) fn selected_match(&self) -> Option<&PathBuf> {
        self.state
            .selected_idx
            .and_then(|idx| self.matches.get(idx))
            .map(|file_match| &file_match.path)
    }

    /// 🍳 이 함수는 결과 개수와 상태에 따라 popup 높이를 계산한다.
    pub(crate) fn calculate_required_height(&self) -> u16 {
        // Row count depends on whether we already have matches. If no matches
        // yet (e.g. initial search or query with no results) reserve a single
        // row so the popup is still visible. When matches are present we show
        // up to MAX_RESULTS regardless of the waiting flag so the list
        // remains stable while a newer search is in-flight.

        self.matches.len().clamp(1, MAX_POPUP_ROWS) as u16
    }
}

impl WidgetRef for &FileSearchPopup {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        // Convert matches to GenericDisplayRow, translating indices to usize at the UI boundary.
        let rows_all: Vec<GenericDisplayRow> = if self.matches.is_empty() {
            Vec::new()
        } else {
            self.matches
                .iter()
                .map(|m| GenericDisplayRow {
                    name: m.path.to_string_lossy().to_string(),
                    name_prefix_spans: Vec::new(),
                    match_indices: m
                        .indices
                        .as_ref()
                        .map(|v| v.iter().map(|&i| i as usize).collect()),
                    display_shortcut: None,
                    description: None,
                    category_tag: None,
                    wrap_indent: None,
                    is_disabled: false,
                    disabled_reason: None,
                })
                .collect()
        };

        let empty_message = if self.waiting {
            "loading..."
        } else {
            "no matches"
        };

        render_rows(
            area.inset(Insets::tlbr(
                /*top*/ 0, /*left*/ 2, /*bottom*/ 0, /*right*/ 0,
            )),
            buf,
            &rows_all,
            &self.state,
            MAX_POPUP_ROWS,
            empty_message,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_file_search::MatchType;
    use pretty_assertions::assert_eq;

    fn file_match(index: usize) -> FileMatch {
        FileMatch {
            score: index as u32,
            path: PathBuf::from(format!("src/file_{index:02}.rs")),
            match_type: MatchType::File,
            root: PathBuf::from("/tmp/repo"),
            indices: None,
        }
    }

    #[test]
    fn set_matches_keeps_only_the_first_page_of_results() {
        let mut popup = FileSearchPopup::new();
        popup.set_query("file");
        popup.set_matches("file", (0..(MAX_POPUP_ROWS + 2)).map(file_match).collect());

        assert_eq!(
            popup.matches,
            (0..MAX_POPUP_ROWS).map(file_match).collect::<Vec<_>>()
        );
        assert_eq!(popup.calculate_required_height(), MAX_POPUP_ROWS as u16);
    }
}
