//! 📄 이 파일이 하는 일:
//!   여러 UI 요소가 `AppEvent`를 안전하게 app 루프 채널로 보내도록 도와주는 얇은 송신기다.
//!   비유로 말하면 교실에서 나온 요청 메모를 방송실 우편함에 넣어 주는 심부름꾼이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui`
//!   - 버튼/위젯/overlay가 app 이벤트를 보낼 때
//!
//! 🧩 핵심 개념:
//!   - sender wrapper = 채널 송신기를 편한 helper 함수 묶음으로 감싼 것
//!   - `session_log` = 재생용 로그를 남길 때 `CodexOp`와 일반 UI 이벤트를 구분하는 기록장

use std::path::PathBuf;

use crate::app_command::AppCommand;
use codex_protocol::ThreadId;
use codex_protocol::approvals::ElicitationAction;
use codex_protocol::mcp::RequestId as McpRequestId;
use codex_protocol::protocol::ConversationAudioParams;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::request_permissions::RequestPermissionsResponse;
use codex_protocol::request_user_input::RequestUserInputResponse;
use tokio::sync::mpsc::UnboundedSender;

use crate::app_event::AppEvent;
use crate::session_log;

/// 🍳 이 구조체는 app 이벤트 채널 송신기를 손에 쥔 심부름꾼이다.
#[derive(Clone, Debug)]
pub(crate) struct AppEventSender {
    pub app_event_tx: UnboundedSender<AppEvent>,
}

impl AppEventSender {
    /// 🍳 이 함수는 채널 송신기를 감싸 새 helper 묶음을 만든다.
    pub(crate) fn new(app_event_tx: UnboundedSender<AppEvent>) -> Self {
        Self { app_event_tx }
    }

    /// Send an event to the app event channel. If it fails, we swallow the
    /// error and log it.
    /// 🍳 이 함수는 이벤트를 채널로 보내고, 실패하면 앱을 죽이지 않고 로그만 남긴다.
    pub(crate) fn send(&self, event: AppEvent) {
        // Record inbound events for high-fidelity session replay.
        // Avoid double-logging Ops; those are logged at the point of submission.
        if !matches!(event, AppEvent::CodexOp(_)) {
            session_log::log_inbound_app_event(&event);
        }
        if let Err(e) = self.app_event_tx.send(event) {
            tracing::error!("failed to send event: {e}");
        }
    }

    /// 🍳 아래 helper들은 자주 쓰는 행동을 `AppEvent`/`AppCommand` 조합으로 빠르게 포장하는 단축 버튼들이다.
    pub(crate) fn interrupt(&self) {
        self.send(AppEvent::CodexOp(AppCommand::interrupt().into_core()));
    }

    pub(crate) fn compact(&self) {
        self.send(AppEvent::CodexOp(AppCommand::compact().into_core()));
    }

    pub(crate) fn set_thread_name(&self, name: String) {
        self.send(AppEvent::CodexOp(
            AppCommand::set_thread_name(name).into_core(),
        ));
    }

    pub(crate) fn review(&self, review_request: ReviewRequest) {
        self.send(AppEvent::CodexOp(
            AppCommand::review(review_request).into_core(),
        ));
    }

    pub(crate) fn list_skills(&self, cwds: Vec<PathBuf>, force_reload: bool) {
        self.send(AppEvent::CodexOp(
            AppCommand::list_skills(cwds, force_reload).into_core(),
        ));
    }

    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub(crate) fn realtime_conversation_audio(&self, params: ConversationAudioParams) {
        self.send(AppEvent::CodexOp(
            AppCommand::realtime_conversation_audio(params).into_core(),
        ));
    }

    pub(crate) fn user_input_answer(&self, id: String, response: RequestUserInputResponse) {
        self.send(AppEvent::CodexOp(
            AppCommand::user_input_answer(id, response).into_core(),
        ));
    }

    pub(crate) fn exec_approval(&self, thread_id: ThreadId, id: String, decision: ReviewDecision) {
        self.send(AppEvent::SubmitThreadOp {
            thread_id,
            op: AppCommand::exec_approval(id, /*turn_id*/ None, decision).into_core(),
        });
    }

    pub(crate) fn request_permissions_response(
        &self,
        thread_id: ThreadId,
        id: String,
        response: RequestPermissionsResponse,
    ) {
        self.send(AppEvent::SubmitThreadOp {
            thread_id,
            op: AppCommand::request_permissions_response(id, response).into_core(),
        });
    }

    pub(crate) fn patch_approval(&self, thread_id: ThreadId, id: String, decision: ReviewDecision) {
        self.send(AppEvent::SubmitThreadOp {
            thread_id,
            op: AppCommand::patch_approval(id, decision).into_core(),
        });
    }

    pub(crate) fn resolve_elicitation(
        &self,
        thread_id: ThreadId,
        server_name: String,
        request_id: McpRequestId,
        decision: ElicitationAction,
        content: Option<serde_json::Value>,
        meta: Option<serde_json::Value>,
    ) {
        self.send(AppEvent::SubmitThreadOp {
            thread_id,
            op: AppCommand::resolve_elicitation(server_name, request_id, decision, content, meta)
                .into_core(),
        });
    }
}
