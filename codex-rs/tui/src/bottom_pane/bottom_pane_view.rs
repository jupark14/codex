//! 📄 이 파일이 하는 일:
//!   bottom pane에 올라올 수 있는 모든 뷰가 따라야 하는 공통 약속(trait)을 정의한다.
//!   비유로 말하면 무대에 올라오는 모든 배우가 "키 입력 받기", "끝났는지 알리기", "붙여넣기 처리" 같은 공통 신호를 맞춰야 한다고 적어 둔 공연 규칙표다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/bottom_pane`
//!   - approval/user-input/modal view 구현체들
//!
//! 🧩 핵심 개념:
//!   - trait = 여러 뷰가 같은 인터페이스로 움직이게 하는 공통 약속
//!   - default method = 특별한 처리가 없으면 기본 행동으로 두는 빈 동작

use crate::bottom_pane::ApprovalRequest;
use crate::bottom_pane::McpServerElicitationFormRequest;
use crate::render::renderable::Renderable;
use codex_protocol::request_user_input::RequestUserInputEvent;
use crossterm::event::KeyEvent;

use super::CancellationEvent;

/// Trait implemented by every view that can be shown in the bottom pane.
/// 🍳 이 trait는 bottom pane 무대에 올라오는 뷰들이 지켜야 할 공통 약속이다.
pub(crate) trait BottomPaneView: Renderable {
    /// Handle a key event while the view is active. A redraw is always
    /// scheduled after this call.
    /// 🍳 키 입력이 들어왔을 때 각 뷰가 직접 처리할 수 있는 훅이다.
    fn handle_key_event(&mut self, _key_event: KeyEvent) {}

    /// Return `true` if the view has finished and should be removed.
    /// 🍳 뷰가 할 일을 다 끝냈으면 `true`를 돌려 bottom pane에서 내려가게 한다.
    fn is_complete(&self) -> bool {
        false
    }

    /// Stable identifier for views that need external refreshes while open.
    /// 🍳 바깥에서 새로고침할 때 같은 뷰를 찾아가기 위한 이름표다.
    fn view_id(&self) -> Option<&'static str> {
        None
    }

    /// Actual item index for list-based views that want to preserve selection
    /// across external refreshes.
    /// 🍳 리스트 뷰가 외부 새로고침 뒤에도 같은 항목을 가리키게 해 주는 현재 선택 번호다.
    fn selected_index(&self) -> Option<usize> {
        None
    }

    /// Handle Ctrl-C while this view is active.
    /// 🍳 Ctrl-C가 눌렸을 때 취소를 어떻게 처리할지 정하는 갈림길이다.
    fn on_ctrl_c(&mut self) -> CancellationEvent {
        CancellationEvent::NotHandled
    }

    /// Return true if Esc should be routed through `handle_key_event` instead
    /// of the `on_ctrl_c` cancellation path.
    /// 🍳 Esc를 일반 키처럼 처리할지, 취소 경로로 보낼지 고르는 스위치다.
    fn prefer_esc_to_handle_key_event(&self) -> bool {
        false
    }

    /// Optional paste handler. Return true if the view modified its state and
    /// needs a redraw.
    /// 🍳 붙여넣기 문자열을 직접 받아 상태를 바꿨으면 `true`를 돌려 다시 그리게 한다.
    fn handle_paste(&mut self, _pasted: String) -> bool {
        false
    }

    /// Flush any pending paste-burst state. Return true if state changed.
    ///
    /// This lets a modal that reuses `ChatComposer` participate in the same
    /// time-based paste burst flushing as the primary composer.
    /// 🍳 paste burst 임시 상태를 시간 맞춰 비우는 훅이다.
    fn flush_paste_burst_if_due(&mut self) -> bool {
        false
    }

    /// Whether the view is currently holding paste-burst transient state.
    ///
    /// When `true`, the bottom pane will schedule a short delayed redraw to
    /// give the burst time window a chance to flush.
    /// 🍳 지금 paste burst 임시 상태를 쥐고 있는지 알려 준다.
    fn is_in_paste_burst(&self) -> bool {
        false
    }

    /// Try to handle approval request; return the original value if not
    /// consumed.
    /// 🍳 approval 요청을 이 뷰가 먹을 수 있으면 소비하고, 아니면 그대로 돌려준다.
    fn try_consume_approval_request(
        &mut self,
        request: ApprovalRequest,
    ) -> Option<ApprovalRequest> {
        Some(request)
    }

    /// Try to handle request_user_input; return the original value if not
    /// consumed.
    /// 🍳 request_user_input 요청도 같은 방식으로 소비 여부를 정한다.
    fn try_consume_user_input_request(
        &mut self,
        request: RequestUserInputEvent,
    ) -> Option<RequestUserInputEvent> {
        Some(request)
    }

    /// Try to handle a supported MCP server elicitation form request; return the original value if
    /// not consumed.
    /// 🍳 MCP elicitation 폼 요청을 이 뷰가 처리 가능하면 받아 먹는다.
    fn try_consume_mcp_server_elicitation_request(
        &mut self,
        request: McpServerElicitationFormRequest,
    ) -> Option<McpServerElicitationFormRequest> {
        Some(request)
    }
}
