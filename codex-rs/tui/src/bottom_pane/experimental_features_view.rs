//! 📄 이 파일이 하는 일:
//!   실험 기능 목록을 보여 주고 사용자가 켜고 끄는 popup view를 제공한다.
//!   비유로 말하면 연구실 실험 스위치판을 띄워서 체크박스로 기능을 토글하게 하는 제어 패널이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/bottom_pane`
//!   - experimental feature 설정 popup
//!
//! 🧩 핵심 개념:
//!   - `ExperimentalFeatureItem` = 기능 하나의 이름/설명/켜짐 상태 카드
//!   - `ScrollState` = 현재 선택과 스크롤 범위를 기억하는 상태

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::Widget;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::key_hint;
use crate::render::Insets;
use crate::render::RectExt as _;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;

use codex_features::Feature;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::measure_rows_height;
use super::selection_popup_common::render_rows;

/// 🍳 이 구조체는 실험 기능 한 줄을 렌더링하기 위한 데이터 카드다.
pub(crate) struct ExperimentalFeatureItem {
    pub feature: Feature,
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

/// 🍳 이 구조체는 실험 기능 popup 전체 상태를 담는 본체다.
pub(crate) struct ExperimentalFeaturesView {
    features: Vec<ExperimentalFeatureItem>,
    state: ScrollState,
    complete: bool,
    app_event_tx: AppEventSender,
    header: Box<dyn Renderable>,
    footer_hint: Line<'static>,
}

impl ExperimentalFeaturesView {
    /// 🍳 이 함수는 기능 목록과 이벤트 송신기를 받아 popup을 초기화한다.
    pub(crate) fn new(
        features: Vec<ExperimentalFeatureItem>,
        app_event_tx: AppEventSender,
    ) -> Self {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Experimental features".bold()));
        header.push(Line::from(
            "Toggle experimental features. Changes are saved to config.toml.".dim(),
        ));

        let mut view = Self {
            features,
            state: ScrollState::new(),
            complete: false,
            app_event_tx,
            header: Box::new(header),
            footer_hint: experimental_popup_hint_line(),
        };
        view.initialize_selection();
        view
    }

    fn initialize_selection(&mut self) {
        if self.visible_len() == 0 {
            self.state.selected_idx = None;
        } else if self.state.selected_idx.is_none() {
            self.state.selected_idx = Some(0);
        }
    }

    /// 🍳 현재 보이는 기능 수를 돌려준다.
    fn visible_len(&self) -> usize {
        self.features.len()
    }

    /// 🍳 이 함수는 기능 목록을 체크박스가 붙은 렌더링 행들로 바꾼다.
    fn build_rows(&self) -> Vec<GenericDisplayRow> {
        let mut rows = Vec::with_capacity(self.features.len());
        let selected_idx = self.state.selected_idx;
        for (idx, item) in self.features.iter().enumerate() {
            let prefix = if selected_idx == Some(idx) {
                '›'
            } else {
                ' '
            };
            let marker = if item.enabled { 'x' } else { ' ' };
            let name = format!("{prefix} [{marker}] {}", item.name);
            rows.push(GenericDisplayRow {
                name,
                description: Some(item.description.clone()),
                ..Default::default()
            });
        }

        rows
    }

    /// 🍳 위쪽 항목으로 선택을 한 칸 움직인다.
    fn move_up(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    /// 🍳 아래쪽 항목으로 선택을 한 칸 움직인다.
    fn move_down(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        self.state.move_down_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    /// 🍳 현재 선택된 기능의 on/off 상태를 뒤집는다.
    fn toggle_selected(&mut self) {
        let Some(selected_idx) = self.state.selected_idx else {
            return;
        };

        if let Some(item) = self.features.get_mut(selected_idx) {
            item.enabled = !item.enabled;
        }
    }

    /// 🍳 전체 popup 너비에서 실제 행 렌더링에 쓸 폭을 계산한다.
    fn rows_width(total_width: u16) -> u16 {
        total_width.saturating_sub(2)
    }
}

impl BottomPaneView for ExperimentalFeaturesView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('\u{0010}'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_up(),
            KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_up(),
            KeyEvent {
                code: KeyCode::Down,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('n'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('\u{000e}'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_down(),
            KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_down(),
            KeyEvent {
                code: KeyCode::Char(' '),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.toggle_selected(),
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.on_ctrl_c();
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        // Save the updates
        if !self.features.is_empty() {
            let updates = self
                .features
                .iter()
                .map(|item| (item.feature, item.enabled))
                .collect();
            self.app_event_tx
                .send(AppEvent::UpdateFeatureFlags { updates });
        }

        self.complete = true;
        CancellationEvent::Handled
    }
}

impl Renderable for ExperimentalFeaturesView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let [content_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);

        Block::default()
            .style(user_message_style())
            .render(content_area, buf);

        let header_height = self
            .header
            .desired_height(content_area.width.saturating_sub(4));
        let rows = self.build_rows();
        let rows_width = Self::rows_width(content_area.width);
        let rows_height = measure_rows_height(
            &rows,
            &self.state,
            MAX_POPUP_ROWS,
            rows_width.saturating_add(1),
        );
        let [header_area, _, list_area] = Layout::vertical([
            Constraint::Max(header_height),
            Constraint::Max(1),
            Constraint::Length(rows_height),
        ])
        .areas(content_area.inset(Insets::vh(/*v*/ 1, /*h*/ 2)));

        self.header.render(header_area, buf);

        if list_area.height > 0 {
            let render_area = Rect {
                x: list_area.x.saturating_sub(2),
                y: list_area.y,
                width: rows_width.max(1),
                height: list_area.height,
            };
            render_rows(
                render_area,
                buf,
                &rows,
                &self.state,
                MAX_POPUP_ROWS,
                "  No experimental features available for now",
            );
        }

        let hint_area = Rect {
            x: footer_area.x + 2,
            y: footer_area.y,
            width: footer_area.width.saturating_sub(2),
            height: footer_area.height,
        };
        self.footer_hint.clone().dim().render(hint_area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        let rows = self.build_rows();
        let rows_width = Self::rows_width(width);
        let rows_height = measure_rows_height(
            &rows,
            &self.state,
            MAX_POPUP_ROWS,
            rows_width.saturating_add(1),
        );

        let mut height = self.header.desired_height(width.saturating_sub(4));
        height = height.saturating_add(rows_height + 3);
        height.saturating_add(1)
    }
}

fn experimental_popup_hint_line() -> Line<'static> {
    Line::from(vec![
        "Press ".into(),
        key_hint::plain(KeyCode::Char(' ')).into(),
        " to select or ".into(),
        key_hint::plain(KeyCode::Enter).into(),
        " to save for next conversation".into(),
    ])
}
