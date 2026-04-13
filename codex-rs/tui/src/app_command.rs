//! 📄 이 모듈이 하는 일:
//!   TUI에서 발생한 사용자 행동을 `Op` 기반 앱 명령으로 감싸서 한 곳으로 모은다.
//!   비유로 말하면 버튼 클릭, 승인 응답, 새 세션 시작 같은 일을 "행동 주문서"로 적어 전달하는 접수함이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/app.rs`
//!   - 여러 위젯/overlay가 app 레벨에 명령을 보낼 때
//!
//! 🧩 핵심 개념:
//!   - `AppCommand` = 실제 전송 가능한 포장된 명령
//!   - `AppCommandView` = 명령을 읽기 좋게 분해해서 match 하기 위한 관찰창

use std::path::PathBuf;

use codex_config::types::ApprovalsReviewer;
use codex_protocol::approvals::ElicitationAction;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::mcp::RequestId as McpRequestId;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ConversationAudioParams;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::ConversationTextParams;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::request_permissions::RequestPermissionsResponse;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_protocol::user_input::UserInput;
use serde::Serialize;
use serde_json::Value;

/// 🍳 이 구조체는 실제 `Op`를 앱 전용 명령 봉투에 넣은 래퍼다.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct AppCommand(Op);

/// 🍳 이 enum은 `AppCommand`를 읽기 쉽게 펼쳐 본 관찰 모드다.
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub(crate) enum AppCommandView<'a> {
    Interrupt,
    CleanBackgroundTerminals,
    RealtimeConversationStart(&'a ConversationStartParams),
    RealtimeConversationAudio(&'a ConversationAudioParams),
    RealtimeConversationText(&'a ConversationTextParams),
    RealtimeConversationClose,
    RunUserShellCommand {
        command: &'a str,
    },
    UserTurn {
        items: &'a [UserInput],
        cwd: &'a PathBuf,
        approval_policy: AskForApproval,
        approvals_reviewer: &'a Option<ApprovalsReviewer>,
        sandbox_policy: &'a SandboxPolicy,
        model: &'a str,
        effort: Option<ReasoningEffortConfig>,
        summary: &'a Option<ReasoningSummaryConfig>,
        service_tier: &'a Option<Option<ServiceTier>>,
        final_output_json_schema: &'a Option<Value>,
        collaboration_mode: &'a Option<CollaborationMode>,
        personality: &'a Option<Personality>,
    },
    OverrideTurnContext {
        cwd: &'a Option<PathBuf>,
        approval_policy: &'a Option<AskForApproval>,
        approvals_reviewer: &'a Option<ApprovalsReviewer>,
        sandbox_policy: &'a Option<SandboxPolicy>,
        windows_sandbox_level: &'a Option<WindowsSandboxLevel>,
        model: &'a Option<String>,
        effort: &'a Option<Option<ReasoningEffortConfig>>,
        summary: &'a Option<ReasoningSummaryConfig>,
        service_tier: &'a Option<Option<ServiceTier>>,
        collaboration_mode: &'a Option<CollaborationMode>,
        personality: &'a Option<Personality>,
    },
    ExecApproval {
        id: &'a str,
        turn_id: &'a Option<String>,
        decision: &'a ReviewDecision,
    },
    PatchApproval {
        id: &'a str,
        decision: &'a ReviewDecision,
    },
    ResolveElicitation {
        server_name: &'a str,
        request_id: &'a McpRequestId,
        decision: &'a ElicitationAction,
        content: &'a Option<Value>,
        meta: &'a Option<Value>,
    },
    UserInputAnswer {
        id: &'a str,
        response: &'a RequestUserInputResponse,
    },
    RequestPermissionsResponse {
        id: &'a str,
        response: &'a RequestPermissionsResponse,
    },
    ReloadUserConfig,
    ListSkills {
        cwds: &'a [PathBuf],
        force_reload: bool,
    },
    Compact,
    SetThreadName {
        name: &'a str,
    },
    Shutdown,
    ThreadRollback {
        num_turns: u32,
    },
    Review {
        review_request: &'a ReviewRequest,
    },
    Other(&'a Op),
}

impl AppCommand {
    /// 🍳 이 함수는 인터럽트 명령서를 만든다.
    pub(crate) fn interrupt() -> Self {
        Self(Op::Interrupt)
    }

    /// 🍳 이 함수는 백그라운드 터미널 청소 명령서를 만든다.
    pub(crate) fn clean_background_terminals() -> Self {
        Self(Op::CleanBackgroundTerminals)
    }

    pub(crate) fn realtime_conversation_start(params: ConversationStartParams) -> Self {
        Self(Op::RealtimeConversationStart(params))
    }

    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub(crate) fn realtime_conversation_audio(params: ConversationAudioParams) -> Self {
        Self(Op::RealtimeConversationAudio(params))
    }

    #[allow(dead_code)]
    pub(crate) fn realtime_conversation_text(params: ConversationTextParams) -> Self {
        Self(Op::RealtimeConversationText(params))
    }

    pub(crate) fn realtime_conversation_close() -> Self {
        Self(Op::RealtimeConversationClose)
    }

    pub(crate) fn run_user_shell_command(command: String) -> Self {
        Self(Op::RunUserShellCommand { command })
    }

    #[allow(clippy::too_many_arguments)]
    /// 🍳 이 함수는 사용자 입력 턴 한 번을 보내는 주문서를 만든다.
    pub(crate) fn user_turn(
        items: Vec<UserInput>,
        cwd: PathBuf,
        approval_policy: AskForApproval,
        sandbox_policy: SandboxPolicy,
        model: String,
        effort: Option<ReasoningEffortConfig>,
        summary: Option<ReasoningSummaryConfig>,
        service_tier: Option<Option<ServiceTier>>,
        final_output_json_schema: Option<Value>,
        collaboration_mode: Option<CollaborationMode>,
        personality: Option<Personality>,
    ) -> Self {
        Self(Op::UserTurn {
            items,
            cwd,
            approval_policy,
            approvals_reviewer: None,
            sandbox_policy,
            model,
            effort,
            summary,
            service_tier,
            final_output_json_schema,
            collaboration_mode,
            personality,
        })
    }

    #[allow(clippy::too_many_arguments)]
    /// 🍳 이 함수는 현재 턴의 실행 환경만 부분적으로 덮어쓰는 명령서를 만든다.
    pub(crate) fn override_turn_context(
        cwd: Option<PathBuf>,
        approval_policy: Option<AskForApproval>,
        approvals_reviewer: Option<ApprovalsReviewer>,
        sandbox_policy: Option<SandboxPolicy>,
        windows_sandbox_level: Option<WindowsSandboxLevel>,
        model: Option<String>,
        effort: Option<Option<ReasoningEffortConfig>>,
        summary: Option<ReasoningSummaryConfig>,
        service_tier: Option<Option<ServiceTier>>,
        collaboration_mode: Option<CollaborationMode>,
        personality: Option<Personality>,
    ) -> Self {
        Self(Op::OverrideTurnContext {
            cwd,
            approval_policy,
            approvals_reviewer,
            sandbox_policy,
            windows_sandbox_level,
            model,
            effort,
            summary,
            service_tier,
            collaboration_mode,
            personality,
        })
    }

    pub(crate) fn exec_approval(
        id: String,
        turn_id: Option<String>,
        decision: ReviewDecision,
    ) -> Self {
        Self(Op::ExecApproval {
            id,
            turn_id,
            decision,
        })
    }

    pub(crate) fn patch_approval(id: String, decision: ReviewDecision) -> Self {
        Self(Op::PatchApproval { id, decision })
    }

    pub(crate) fn resolve_elicitation(
        server_name: String,
        request_id: McpRequestId,
        decision: ElicitationAction,
        content: Option<Value>,
        meta: Option<Value>,
    ) -> Self {
        Self(Op::ResolveElicitation {
            server_name,
            request_id,
            decision,
            content,
            meta,
        })
    }

    pub(crate) fn user_input_answer(id: String, response: RequestUserInputResponse) -> Self {
        Self(Op::UserInputAnswer { id, response })
    }

    pub(crate) fn request_permissions_response(
        id: String,
        response: RequestPermissionsResponse,
    ) -> Self {
        Self(Op::RequestPermissionsResponse { id, response })
    }

    pub(crate) fn reload_user_config() -> Self {
        Self(Op::ReloadUserConfig)
    }

    pub(crate) fn list_skills(cwds: Vec<PathBuf>, force_reload: bool) -> Self {
        Self(Op::ListSkills { cwds, force_reload })
    }

    pub(crate) fn compact() -> Self {
        Self(Op::Compact)
    }

    pub(crate) fn set_thread_name(name: String) -> Self {
        Self(Op::SetThreadName { name })
    }

    pub(crate) fn thread_rollback(num_turns: u32) -> Self {
        Self(Op::ThreadRollback { num_turns })
    }

    pub(crate) fn review(review_request: ReviewRequest) -> Self {
        Self(Op::Review { review_request })
    }

    #[allow(dead_code)]
    pub(crate) fn kind(&self) -> &'static str {
        self.0.kind()
    }

    #[allow(dead_code)]
    pub(crate) fn as_core(&self) -> &Op {
        &self.0
    }

    pub(crate) fn into_core(self) -> Op {
        self.0
    }

    pub(crate) fn is_review(&self) -> bool {
        matches!(self.view(), AppCommandView::Review { .. })
    }

    pub(crate) fn view(&self) -> AppCommandView<'_> {
        match &self.0 {
            Op::Interrupt => AppCommandView::Interrupt,
            Op::CleanBackgroundTerminals => AppCommandView::CleanBackgroundTerminals,
            Op::RealtimeConversationStart(params) => {
                AppCommandView::RealtimeConversationStart(params)
            }
            Op::RealtimeConversationAudio(params) => {
                AppCommandView::RealtimeConversationAudio(params)
            }
            Op::RealtimeConversationText(params) => {
                AppCommandView::RealtimeConversationText(params)
            }
            Op::RealtimeConversationClose => AppCommandView::RealtimeConversationClose,
            Op::RunUserShellCommand { command } => AppCommandView::RunUserShellCommand { command },
            Op::UserTurn {
                items,
                cwd,
                approval_policy,
                approvals_reviewer,
                sandbox_policy,
                model,
                effort,
                summary,
                service_tier,
                final_output_json_schema,
                collaboration_mode,
                personality,
            } => AppCommandView::UserTurn {
                items,
                cwd,
                approval_policy: *approval_policy,
                approvals_reviewer,
                sandbox_policy,
                model,
                effort: *effort,
                summary,
                service_tier,
                final_output_json_schema,
                collaboration_mode,
                personality,
            },
            Op::OverrideTurnContext {
                cwd,
                approval_policy,
                approvals_reviewer,
                sandbox_policy,
                windows_sandbox_level,
                model,
                effort,
                summary,
                service_tier,
                collaboration_mode,
                personality,
            } => AppCommandView::OverrideTurnContext {
                cwd,
                approval_policy,
                approvals_reviewer,
                sandbox_policy,
                windows_sandbox_level,
                model,
                effort,
                summary,
                service_tier,
                collaboration_mode,
                personality,
            },
            Op::ExecApproval {
                id,
                turn_id,
                decision,
            } => AppCommandView::ExecApproval {
                id,
                turn_id,
                decision,
            },
            Op::PatchApproval { id, decision } => AppCommandView::PatchApproval { id, decision },
            Op::ResolveElicitation {
                server_name,
                request_id,
                decision,
                content,
                meta,
            } => AppCommandView::ResolveElicitation {
                server_name,
                request_id,
                decision,
                content,
                meta,
            },
            Op::UserInputAnswer { id, response } => {
                AppCommandView::UserInputAnswer { id, response }
            }
            Op::RequestPermissionsResponse { id, response } => {
                AppCommandView::RequestPermissionsResponse { id, response }
            }
            Op::ReloadUserConfig => AppCommandView::ReloadUserConfig,
            Op::ListSkills { cwds, force_reload } => AppCommandView::ListSkills {
                cwds,
                force_reload: *force_reload,
            },
            Op::Compact => AppCommandView::Compact,
            Op::SetThreadName { name } => AppCommandView::SetThreadName { name },
            Op::Shutdown => AppCommandView::Shutdown,
            Op::ThreadRollback { num_turns } => AppCommandView::ThreadRollback {
                num_turns: *num_turns,
            },
            Op::Review { review_request } => AppCommandView::Review { review_request },
            op => AppCommandView::Other(op),
        }
    }
}

impl From<Op> for AppCommand {
    fn from(value: Op) -> Self {
        Self(value)
    }
}

impl From<&Op> for AppCommand {
    fn from(value: &Op) -> Self {
        Self(value.clone())
    }
}

impl From<&AppCommand> for AppCommand {
    fn from(value: &AppCommand) -> Self {
        value.clone()
    }
}

impl From<AppCommand> for Op {
    fn from(value: AppCommand) -> Self {
        value.0
    }
}
