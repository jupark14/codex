//! ## 📐 Architecture Overview
//!
//! 이 모듈은 **세션 한 턴 동안 돌아가는 백그라운드 일감(=태스크)의 라이프사이클 관리자**다.
//!
//! 비유하면 **공장 라인 매니저**:
//! - 새 주문(`spawn_task`) 들어오면 기존 라인 정리 후 새 라인 가동.
//! - 어떤 종류의 일감인지(Regular / Compact / Review / GhostSnapshot / Undo /
//!   UserShell) 트레잇 객체로 추상화 → 매니저는 일감 종류를 몰라도 된다.
//! - 종료(`abort_all_tasks`) 신호가 오면 협조적 정지 → 100ms 안에 안 멈추면 강제 종료.
//! - 끝나면(`on_task_finished`) 토큰 usage 메트릭 발사하고 다음 큐에 들어있는 작업
//!   확인 후 자동으로 다음 턴 시작.
//!
//! ```text
//!   spawn_task(...)
//!         │
//!         ▼
//!   abort_all_tasks (이전 라인 정리)
//!         │
//!         ▼
//!   start_task ────────────────┐
//!         │ tokio::spawn        │
//!         │ + AbortOnDropHandle │
//!         ▼                     │
//!   SessionTask::run            │
//!     (RegularTask / Compact /  │
//!      Review / Undo / ...)     │
//!         │                     │
//!         ▼                     │
//!   on_task_finished ◀──────────┘
//!     - flush_rollout
//!     - emit token usage metrics
//!     - send TurnComplete event
//!     - maybe_start_turn_for_pending_work (큐 비어있지 않으면 다음 턴 자동 시작)
//! ```
//!
//! 데이터 플로우:
//! - 인풋: `Vec<UserInput>` (사용자 메시지/입력) + `TurnContext` (모델/도구 설정).
//! - 상태 보관: [[codex-rs/core/src/state/turn.rs::ActiveTurn]] / `RunningTask`.
//! - 아웃풋: `EventMsg::TurnComplete` / `TurnAborted` 를 클라이언트에게.

mod compact;
mod ghost_snapshot;
mod regular;
mod review;
mod undo;
mod user_shell;

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use futures::future::BoxFuture;
use tokio::select;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;
use tracing::info_span;
use tracing::trace;
use tracing::warn;

use crate::codex::Session;
use crate::codex::TurnContext;
use crate::contextual_user_message::TURN_ABORTED_CLOSE_TAG;
use crate::contextual_user_message::TURN_ABORTED_OPEN_TAG;
use crate::hook_runtime::PendingInputHookDisposition;
use crate::hook_runtime::inspect_pending_input;
use crate::hook_runtime::record_additional_contexts;
use crate::hook_runtime::record_pending_input;
use crate::state::ActiveTurn;
use crate::state::RunningTask;
use crate::state::TaskKind;
use codex_login::AuthManager;
use codex_models_manager::manager::ModelsManager;
use codex_otel::SessionTelemetry;
use codex_otel::TURN_E2E_DURATION_METRIC;
use codex_otel::TURN_NETWORK_PROXY_METRIC;
use codex_otel::TURN_TOKEN_USAGE_METRIC;
use codex_otel::TURN_TOOL_CALL_METRIC;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::protocol::TurnAbortedEvent;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::WarningEvent;
use codex_protocol::user_input::UserInput;

use codex_features::Feature;
pub(crate) use compact::CompactTask;
pub(crate) use ghost_snapshot::GhostSnapshotTask;
pub(crate) use regular::RegularTask;
pub(crate) use review::ReviewTask;
pub(crate) use undo::UndoTask;
pub(crate) use user_shell::UserShellCommandMode;
pub(crate) use user_shell::UserShellCommandTask;
pub(crate) use user_shell::execute_user_shell_command;

const GRACEFULL_INTERRUPTION_TIMEOUT_MS: u64 = 100;
const TURN_ABORTED_INTERRUPTED_GUIDANCE: &str = "The user interrupted the previous turn on purpose. Any running unified exec processes may still be running in the background. If any tools/commands were aborted, they may have partially executed.";

/// Shared model-visible marker used by both the real interrupt path and
/// interrupted fork snapshots.
///
/// "이 턴은 사용자가 중단시켰음" 을 모델 컨텍스트에 남기는 마커 메시지를 만든다.
/// ### Why
/// 모델은 인터럽트 사실을 모르면 "방금 자기가 보낸 응답이 그대로 쓰인 것" 처럼
/// 다음 턴을 시작한다. user role 의 마커를 끼워넣어 모델에게 "직전 턴은 깨졌고,
/// 일부 명령이 부분 실행됐을 수 있다" 는 가이드를 주입.
/// 실제 인터럽트 경로와 fork snapshot 모두에서 같은 마커를 써야 일관성 유지.
pub(crate) fn interrupted_turn_history_marker() -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: format!(
                "{TURN_ABORTED_OPEN_TAG}\n{TURN_ABORTED_INTERRUPTED_GUIDANCE}\n{TURN_ABORTED_CLOSE_TAG}"
            ),
        }],
        end_turn: None,
        phase: None,
    }
}

/// 턴 시작 시 "이 턴 동안 네트워크 프록시가 켜져 있었나?" 를 OTEL 카운터로 1 증가.
/// 단순 텔레메트리 헬퍼.
fn emit_turn_network_proxy_metric(
    session_telemetry: &SessionTelemetry,
    network_proxy_active: bool,
    tmp_mem: (&str, &str),
) {
    let active = if network_proxy_active {
        "true"
    } else {
        "false"
    };
    session_telemetry.counter(
        TURN_NETWORK_PROXY_METRIC,
        /*inc*/ 1,
        &[("active", active), tmp_mem],
    );
}

/// Thin wrapper that exposes the parts of [`Session`] task runners need.
///
/// ### Layer 1 — What
/// 태스크 러너에게 *Session 의 일부분만* 보여주는 절제된 facade.
///
/// ### Layer 4 — Why
/// 태스크 코드가 Session 의 모든 메서드를 자유롭게 부르면, 결합도가 폭발해
/// 리팩터링이 어려워진다. 필요한 핸들(auth_manager, models_manager 등)만 노출하는
/// 작은 인터페이스로 가둠 → 태스크가 "이 인터페이스만 의존" 한다는 명세가 분명해짐.
/// 비유하면 비밀번호 전체를 알려주는 대신 *임시 출입카드*만 발급하는 셈.
///
/// ### Layer 5 — Why Not
/// - **`Arc<Session>` 직접 전달?** → 의존성 폭발. 작은 facade 가 testability/
///   리팩터링 안전성 모두 ↑.
#[derive(Clone)]
pub(crate) struct SessionTaskContext {
    session: Arc<Session>,
}

impl SessionTaskContext {
    /// 단순 생성자.
    pub(crate) fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// Session Arc 을 복제해 돌려준다 — 내부 짙은 작업이 필요한 일부 task 가 사용.
    pub(crate) fn clone_session(&self) -> Arc<Session> {
        Arc::clone(&self.session)
    }

    /// 인증 매니저 핸들 위임.
    pub(crate) fn auth_manager(&self) -> Arc<AuthManager> {
        Arc::clone(&self.session.services.auth_manager)
    }

    /// 모델 카탈로그 매니저 위임.
    pub(crate) fn models_manager(&self) -> Arc<ModelsManager> {
        Arc::clone(&self.session.services.models_manager)
    }
}

/// Async task that drives a [`Session`] turn.
///
/// ### Layer 1 — What (한국어 튜터 주석)
/// "한 턴 동안 비동기로 돌아가는 워크플로우의 형식 계약". 일감의 종류는 다양하지만
/// (Regular chat / Review / Compact / Undo / GhostSnapshot / UserShell) 매니저
/// 입장에선 모두 똑같은 인터페이스로 다루고 싶어서 이 트레잇이 만들어졌다.
///
/// ### Layer 2 — How
/// 3개 메서드만 있으면 됨:
/// - `kind()` — UI/텔레메트리에 보여줄 분류 라벨.
/// - `span_name()` — tracing/OTEL span 이름.
/// - `run()` — 실제 일을 한다. cancellation_token 을 폴링해서 협조적 종료 보장.
/// - `abort()` (default 제공) — 추가 정리가 필요한 경우 오버라이드.
///
/// ### Layer 3 — Macro Role
/// `Session::start_task` 가 이 트레잇 객체를 받아 `tokio::spawn` 으로 띄우고,
/// `RunningTask` 에 핸들 + 토큰 + 컨텍스트를 묶어 보관한다. 트레잇이 추상화 경계.
///
/// ### Layer 4 — Why
/// 비유하면 *Strategy Pattern*. "유튜브 알림처럼 눌러보는 채널 전환" — 매니저는
/// 채널 종류를 모르고 그냥 `run()` 만 누른다. 새 워크플로우를 추가할 때 매니저
/// 코드를 안 바꿔도 됨.
///
/// ### Layer 5 — Why Not
/// - **enum + match?** → 새 variant 마다 `match` 6곳을 동시에 고쳐야 함. 트레잇은
///   "구현 추가 = 새 파일 추가" 로 닫힌 확장(open-closed).
/// - **`async fn` in trait?** → 안정 Rust 에서 가능하지만, 여기선 `impl Future`
///   리턴 타입이 캡처/lifetime 컨트롤에 더 유연. (관용적 패턴)
///
/// ### Layer 6 — Lesson
/// 📌 *"한 라이프사이클(spawn → run → abort) 을 공유하는 다양한 일감은 트레잇으로
/// 묶고, 매니저는 트레잇 객체로만 다뤄라"* — 새 일감 타입 추가 시 매니저는 무지(無知).
/// 교과서: *Strategy Pattern* (Gang of Four).
///
/// Implementations encapsulate a specific Codex workflow (regular chat,
/// reviews, ghost snapshots, etc.). Each task instance is owned by a
/// [`Session`] and executed on a background Tokio task. The trait is
/// intentionally small: implementers identify themselves via
/// [`SessionTask::kind`], perform their work in [`SessionTask::run`], and may
/// release resources in [`SessionTask::abort`].
pub(crate) trait SessionTask: Send + Sync + 'static {
    /// Describes the type of work the task performs so the session can
    /// surface it in telemetry and UI.
    fn kind(&self) -> TaskKind;

    /// Returns the tracing name for a spawned task span.
    fn span_name(&self) -> &'static str;

    /// Executes the task until completion or cancellation.
    ///
    /// Implementations typically stream protocol events using `session` and
    /// `ctx`, returning an optional final agent message when finished. The
    /// provided `cancellation_token` is cancelled when the session requests an
    /// abort; implementers should watch for it and terminate quickly once it
    /// fires. Returning [`Some`] yields a final message that
    /// [`Session::on_task_finished`] will emit to the client.
    fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> impl std::future::Future<Output = Option<String>> + Send;

    /// Gives the task a chance to perform cleanup after an abort.
    ///
    /// The default implementation is a no-op; override this if additional
    /// teardown or notifications are required once
    /// [`Session::abort_all_tasks`] cancels the task.
    fn abort(
        &self,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
    ) -> impl std::future::Future<Output = ()> + Send {
        async move {
            let _ = (session, ctx);
        }
    }
}

/// `SessionTask` 의 dyn-compatible 형제 트레잇.
///
/// ### Layer 1 — What
/// `dyn AnySessionTask` 으로 trait object 를 만들 수 있는 버전.
///
/// ### Layer 2 — How
/// 차이는 메서드 리턴 타입:
/// - `SessionTask::run` 은 `impl Future<Output = ...>` (각 구현마다 타입이 달라
///   trait object 화 불가).
/// - `AnySessionTask::run` 은 `BoxFuture<'static, ...>` (heap-allocated 동일 타입).
/// `impl<T: SessionTask> AnySessionTask for T` 블랭킷 impl 로 모든 SessionTask 가
/// 자동으로 AnySessionTask 도 됨.
///
/// ### Layer 4 — Why
/// 매니저는 `Arc<dyn AnySessionTask>` 로 다양한 워크플로우를 한 컨테이너에 담고
/// 싶다. trait object 가 되려면 메서드 시그니처가 *object-safe* 해야 한다 →
/// `impl Trait` 리턴 금지 → BoxFuture 로 우회.
///
/// ### Layer 5 — Why Not
/// - **`SessionTask` 만 두고 BoxFuture 쓰기?** → 직접 호출 시 매번 box alloc 비용.
///   "정적 호출자는 zero-cost, 동적 호출자만 비용" 이라는 둘 다 만족시키는 패턴.
/// - **`async-trait` 매크로?** → 비슷한 효과지만 매크로 의존이 추가됨. 수동 구현이
///   더 명시적.
///
/// ### Layer 6 — Lesson
/// 📌 *"object-safety 를 깰 수밖에 없는 트레잇은 sibling object-safe trait 을
/// blanket impl 로 자동 변환되게 만들라"* — 사용자(impl 작성자) 는 친화적 버전을
/// 쓰고, 라이브러리는 dyn 버전을 보관.
/// 교과서: *Object-Safe Wrapper Trait* — Rust idiom.
pub(crate) trait AnySessionTask: Send + Sync + 'static {
    fn kind(&self) -> TaskKind;

    fn span_name(&self) -> &'static str;

    fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> BoxFuture<'static, Option<String>>;

    fn abort<'a>(
        &'a self,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
    ) -> BoxFuture<'a, ()>;
}

// 블랭킷 impl: 모든 `SessionTask` 는 자동으로 `AnySessionTask` 도 된다.
// 사용자는 SessionTask 만 작성하면 끝, dyn 변환은 컴파일러가 알아서.
impl<T> AnySessionTask for T
where
    T: SessionTask,
{
    fn kind(&self) -> TaskKind {
        SessionTask::kind(self)
    }

    fn span_name(&self) -> &'static str {
        SessionTask::span_name(self)
    }

    fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> BoxFuture<'static, Option<String>> {
        Box::pin(SessionTask::run(
            self,
            session,
            ctx,
            input,
            cancellation_token,
        ))
    }

    fn abort<'a>(
        &'a self,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(SessionTask::abort(self, session, ctx))
    }
}

impl Session {
    /// 새 태스크를 띄우는 외부 진입점.
    ///
    /// ### Layer 1 — What
    /// "지금 도는 거 다 끄고 새 거 시작" 의 단축키.
    ///
    /// ### Layer 2 — How
    /// 1. `abort_all_tasks(Replaced)` — 진행 중 라인 정리. 사유 라벨로 "교체됨" 전송.
    /// 2. `clear_connector_selection` — 이전 턴에서 잡혀있던 connector 선택 해제.
    /// 3. `start_task` — 새 라인 시작.
    ///
    /// ### Layer 4 — Why
    /// 일감 종류가 다양해도 외부 호출자는 "그냥 새 거 돌려줘" 만 알면 충분하게.
    /// 정리 → 클리어 → 시작의 3단계 순서를 한 곳에 가둬 race condition 방지.
    ///
    /// ### Layer 5 — Why Not
    /// - **호출자가 abort + start 직접?** → 호출 지점마다 순서 실수 가능. 한 번에 묶어두면 안전.
    pub async fn spawn_task<T: SessionTask>(
        self: &Arc<Self>,
        turn_context: Arc<TurnContext>,
        input: Vec<UserInput>,
        task: T,
    ) {
        self.abort_all_tasks(TurnAbortReason::Replaced).await;
        self.clear_connector_selection().await;
        self.start_task(turn_context, input, task).await;
    }

    /// 실제 태스크 spawn — `spawn_task` 와 `maybe_start_turn_for_pending_work` 가 공유.
    ///
    /// ### Layer 1 — What
    /// `tokio::spawn` 으로 백그라운드에서 SessionTask 를 돌리고, 그 핸들/취소 토큰/
    /// 컨텍스트를 `RunningTask` 로 묶어 `ActiveTurn::tasks` 에 등록.
    ///
    /// ### Layer 2 — How
    /// 1. 태스크를 `Arc<dyn AnySessionTask>` 로 박싱 — 종류를 잊은 채 다룰 수 있게.
    /// 2. 턴 시작 시각, 시작 시점 토큰 usage 스냅샷 기록 (나중에 turn 단위 delta 계산용).
    /// 3. 새 `CancellationToken` + `Notify`(완료 신호) 생성.
    /// 4. 큐에 쌓여있던 next-turn 입력 + 메일박스 입력을 모두 끄집어 `TurnState::pending_input`
    ///    에 적재. 💡 두 락(active_turn / turn_state) 을 잠시 잡고 분리해서 풀어준다 — 락 holding 최소화.
    /// 5. tracing span 생성 (turn 라이프사이클 동안 살아있을 task-owned span).
    /// 6. `tokio::spawn` — 안에서: SessionTask::run → flush_rollout → on_task_finished
    ///    호출. cancel 됐으면 on_task_finished 스킵 (abort 측에서 별도 emit).
    /// 7. spawn 핸들을 `AbortOnDropHandle` 로 감싸 RAII abort 보장.
    /// 8. OTEL timer 시작 (`_timer` 가 drop 되면 duration 메트릭 기록).
    /// 9. `RunningTask` 묶음 만들어 ActiveTurn 에 등록.
    ///
    /// ### Layer 3 — Macro Role
    /// 모든 턴 시작 경로의 *최종 합류점*. 어디서 들어오든 여기서 라이프사이클이 일관.
    ///
    /// ### Layer 4 — Why
    /// 핵심은 "두 종류 큐(next-turn 응답 / 메일박스) 를 turn_state 에 미리 다 쏟아부어
    /// 두는 것" — 태스크가 시작되자마자 완전한 입력 세트를 보고 모델 호출을 만들 수
    /// 있게. 이게 중간에 들어오면 race 가 생긴다.
    ///
    /// ### Layer 5 — Why Not
    /// - **`tokio::task::spawn_local`?** → multi-thread runtime 에서 불가. spawn 이 표준.
    /// - **`done` 시그널을 oneshot 으로?** → 기다리는 측이 여러 명일 수 있어 Notify 가 적합.
    /// - **task 내부에서 직접 `on_task_finished` 호출 안 하고 outer drop?** → drop 은
    ///   sync 라 async 작업 못 돌림. 명시적 호출이 더 안전.
    ///
    /// ### Layer 6 — Lesson
    /// 📌 *"비동기 워커 spawn 의 7가지 부수효과(시각/usage/cancel/done/span/RAII/timer)
    /// 를 한 함수에 모아라"* — 분산되면 새 태스크 종류 추가 시 누락 발생.
    /// 교과서: *Structured Concurrency Bootstrap*.
    async fn start_task<T: SessionTask>(
        self: &Arc<Self>,
        turn_context: Arc<TurnContext>,
        input: Vec<UserInput>,
        task: T,
    ) {
        let task: Arc<dyn AnySessionTask> = Arc::new(task);
        let task_kind = task.kind();
        let span_name = task.span_name();
        let started_at = Instant::now();
        turn_context
            .turn_timing_state
            .mark_turn_started(started_at)
            .await;
        let token_usage_at_turn_start = self.total_token_usage().await.unwrap_or_default();

        let cancellation_token = CancellationToken::new();
        let done = Arc::new(Notify::new());

        let queued_response_items = self.take_queued_response_items_for_next_turn().await;
        let mailbox_items = self.get_pending_input().await;
        let turn_state = {
            let mut active = self.active_turn.lock().await;
            let turn = active.get_or_insert_with(ActiveTurn::default);
            debug_assert!(turn.tasks.is_empty());
            Arc::clone(&turn.turn_state)
        };
        {
            let mut turn_state = turn_state.lock().await;
            turn_state.token_usage_at_turn_start = token_usage_at_turn_start;
            for item in queued_response_items {
                turn_state.push_pending_input(item);
            }
            for item in mailbox_items {
                turn_state.push_pending_input(item);
            }
        }

        let mut active = self.active_turn.lock().await;
        let turn = active.get_or_insert_with(ActiveTurn::default);
        debug_assert!(turn.tasks.is_empty());
        let done_clone = Arc::clone(&done);
        let session_ctx = Arc::new(SessionTaskContext::new(Arc::clone(self)));
        let ctx = Arc::clone(&turn_context);
        let task_for_run = Arc::clone(&task);
        let task_cancellation_token = cancellation_token.child_token();
        // Task-owned turn spans keep a core-owned span open for the
        // full task lifecycle after the submission dispatch span ends.
        let task_span = info_span!(
            "turn",
            otel.name = span_name,
            thread.id = %self.conversation_id,
            turn.id = %turn_context.sub_id,
            model = %turn_context.model_info.slug,
        );
        let handle = tokio::spawn(
            async move {
                let ctx_for_finish = Arc::clone(&ctx);
                let last_agent_message = task_for_run
                    .run(
                        Arc::clone(&session_ctx),
                        ctx,
                        input,
                        task_cancellation_token.child_token(),
                    )
                    .await;
                let sess = session_ctx.clone_session();
                if let Err(err) = sess.flush_rollout().await {
                    warn!("failed to flush rollout before completing turn: {err}");
                    sess.send_event(
                        ctx_for_finish.as_ref(),
                        EventMsg::Warning(WarningEvent {
                            message: format!(
                                "Failed to save the conversation transcript; Codex will continue retrying. Error: {err}"
                            ),
                        }),
                    )
                    .await;
                }
                if !task_cancellation_token.is_cancelled() {
                    // Emit completion uniformly from spawn site so all tasks share the same lifecycle.
                    sess.on_task_finished(Arc::clone(&ctx_for_finish), last_agent_message)
                        .await;
                }
                done_clone.notify_waiters();
            }
            .instrument(task_span),
        );
        let timer = turn_context
            .session_telemetry
            .start_timer(TURN_E2E_DURATION_METRIC, &[])
            .ok();
        let running_task = RunningTask {
            done,
            handle: Arc::new(AbortOnDropHandle::new(handle)),
            kind: task_kind,
            task,
            cancellation_token,
            turn_context: Arc::clone(&turn_context),
            _timer: timer,
        };
        turn.add_task(running_task);
    }

    /// Starts a regular turn when the session is idle and pending work is waiting.
    ///
    /// Pending work currently includes queued next-turn items and mailbox mail marked with
    /// `trigger_turn`.
    ///
    /// This helper generates a fresh sub-id for the synthetic turn before delegating to the
    /// explicit-sub-id variant.
    ///
    /// 세션이 idle 인데 큐에 일감이 쌓여 있으면 자동으로 새 턴을 시작.
    /// ### Why
    /// 비유: 식당에서 손님 응대가 끝났는데 주방에 미처리 주문이 있으면 자동으로 다음
    /// 주문을 들어가는 매니저. 사용자 입력 없이도 "기억된 일감" 을 처리해주는 자동
    /// 와인딩.
    /// 신규 sub_id 를 UUID 로 만들어 explicit-id 버전에 위임.
    pub(crate) async fn maybe_start_turn_for_pending_work(self: &Arc<Self>) {
        self.maybe_start_turn_for_pending_work_with_sub_id(uuid::Uuid::new_v4().to_string())
            .await;
    }

    /// Starts a regular turn with the provided sub-id when pending work should wake an idle
    /// session.
    ///
    /// The turn is created only when there are queued next-turn items or mailbox mail marked with
    /// `trigger_turn`, and only if the session is currently idle.
    ///
    /// `maybe_start_turn_for_pending_work` 의 sub_id 명시 버전.
    ///
    /// ### Layer 1 — What
    /// 큐가 비어있지 않고 세션이 idle 일 때만 새 RegularTask 를 띄운다. 두 조건이
    /// 모두 만족돼야 race-free.
    ///
    /// ### Layer 2 — How
    /// 1. 큐 두 종류 모두 비어있으면 즉시 return — alloc 없음.
    /// 2. `active_turn` 락 잡고 이미 `Some` 이면 return — 다른 경로가 이미 시작했을
    ///    수 있다.
    /// 3. `Some(ActiveTurn::default())` 로 슬롯 점유 → 다른 경로 차단.
    /// 4. 새 turn_context 만들고 `start_task` 호출.
    ///
    /// ### Layer 4 — Why
    /// "비어있지 않은 큐 + idle" 의 두 조건 체크는 아토믹하게 묶여야 한다 → 락 안에서
    /// 둘 다 확인하고 슬롯 점유를 한 번에 처리. 그래서 별도 함수로 분리해 단일 진입점.
    ///
    /// ### Layer 5 — Why Not
    /// - **락 밖에서 큐 검사?** → 검사와 점유 사이에 다른 경로가 끼어들 수 있다.
    pub(crate) async fn maybe_start_turn_for_pending_work_with_sub_id(
        self: &Arc<Self>,
        sub_id: String,
    ) {
        if !self.has_queued_response_items_for_next_turn().await
            && !self.has_trigger_turn_mailbox_items().await
        {
            return;
        }

        {
            let mut active_turn = self.active_turn.lock().await;
            if active_turn.is_some() {
                return;
            }
            *active_turn = Some(ActiveTurn::default());
        }

        let turn_context = self.new_default_turn_with_sub_id(sub_id).await;
        self.maybe_emit_unknown_model_warning_for_turn(turn_context.as_ref())
            .await;
        self.start_task(turn_context, Vec::new(), RegularTask::new())
            .await;
    }

    /// 진행 중 모든 태스크를 종료시킨다.
    ///
    /// ### Layer 1 — What
    /// "라인 전체 정지" 버튼. 사유(Replaced / Interrupted / ...) 라벨로 종료 이벤트를
    /// 클라이언트에 보낸다.
    ///
    /// ### Layer 2 — How
    /// 1. `take_active_turn` 으로 ActiveTurn 슬롯을 빼낸다 — `None` 이면 할 일 없음.
    /// 2. `drain_tasks` 로 모든 RunningTask 빼서 각각 `handle_task_abort` 호출 (cancel
    ///    토큰 켜고 graceful 대기 → 강제 abort 의 2단계).
    /// 3. ⚠️ **순서 중요**: `clear_pending` 을 abort *후*에 호출. 먼저 clear 하면
    ///    승인 대기 중이던 receiver 가 RecvError 받아 모델에게 "거부됨" 이벤트가
    ///    먼저 나갈 수 있다 → TurnAborted 이벤트보다 빨라 UI 가 혼란.
    /// 4. 사유가 `Interrupted` 면 큐에 쌓인 일감으로 자동 다음 턴 시작 — 사용자가
    ///    인터럽트 후 다시 입력하지 않아도 흐름이 이어짐.
    ///
    /// ### Layer 5 — Why Not
    /// - **abort 와 clear_pending 동시?** → 위 ⚠️ 의 race 가 발생.
    /// - **인터럽트 후 자동 재개 안 함?** → 사용자가 명시적 재개를 매번 해야 해 UX 저하.
    pub async fn abort_all_tasks(self: &Arc<Self>, reason: TurnAbortReason) {
        if let Some(mut active_turn) = self.take_active_turn().await {
            for task in active_turn.drain_tasks() {
                self.handle_task_abort(task, reason.clone()).await;
            }
            // Let interrupted tasks observe cancellation before dropping pending approvals, or an
            // in-flight approval wait can surface as a model-visible rejection before TurnAborted.
            active_turn.clear_pending().await;
        }
        if reason == TurnAbortReason::Interrupted {
            self.maybe_start_turn_for_pending_work().await;
        }
    }

    /// 태스크가 정상 완료됐을 때 부수 처리를 모두 진행한다.
    ///
    /// ### Layer 1 — What
    /// 태스크 종료 직후의 "체크아웃 카운터" — 1) git enrichment 취소, 2) pending input
    /// 들을 후속 hook 으로 흘리고, 3) 토큰 usage 메트릭 발사, 4) `TurnComplete` 이벤트
    /// 발사, 5) 큐에 일감 남았으면 자동 다음 턴.
    ///
    /// ### Layer 2 — How
    /// 1. `cancel_git_enrichment_task` — 백그라운드로 돌던 git diff 작업 정리.
    /// 2. ActiveTurn 락 안에서: 이 태스크를 tasks 에서 제거. 마지막 태스크였다면
    ///    ActiveTurn 자체를 None 으로 비움 (`should_clear_active_turn`).
    /// 3. turn_state 에서 pending_input / tool_calls / token_usage_at_turn_start 추출.
    /// 4. pending_input 각각에 대해 `inspect_pending_input` 훅 실행. 결과가
    ///    Accepted 면 record_pending_input, Blocked 면 추가 컨텍스트만 기록.
    /// 5. token usage delta 계산 (현재 - 시작) → histogram 메트릭 발사.
    /// 6. `TurnComplete` 이벤트 (last_agent_message + duration 포함) 클라이언트에게.
    /// 7. ActiveTurn 비웠으면 `spawn_blocking + block_on` 으로 다음 턴 시도. 💡 왜
    ///    spawn_blocking 인가? — 현재 호출 스택이 ActiveTurn 락 해제 직후라
    ///    재진입을 피하려고 별도 OS 스레드로 던진다.
    ///
    /// ### Layer 4 — Why
    /// 한 함수에서 종료 ceremony 를 모두 처리해야 메트릭/이벤트/큐 처리 순서가 안정.
    /// 분산되면 metrics 누락 / TurnComplete 누락이 자주 발생.
    ///
    /// ### Layer 5 — Why Not
    /// - **`Drop` 으로 이걸 처리?** → async 처리 불가 + ordering 보장 불가.
    /// - **각 단계 독립 함수로?** → 호출자가 7단계 외워야 함.
    ///
    /// ### Layer 6 — Lesson
    /// 📌 *"비동기 워커의 종료 ceremony 는 *완료 시점에* 호출되는 한 함수에 모두
    /// 모아라"* — Drop 가 아니라 명시적 호출이 async 세계의 안전한 cleanup.
    pub async fn on_task_finished(
        self: &Arc<Self>,
        turn_context: Arc<TurnContext>,
        last_agent_message: Option<String>,
    ) {
        turn_context
            .turn_metadata_state
            .cancel_git_enrichment_task();

        let mut pending_input = Vec::<ResponseInputItem>::new();
        let mut should_clear_active_turn = false;
        let mut token_usage_at_turn_start = None;
        let mut turn_tool_calls = 0_u64;
        let turn_state = {
            let mut active = self.active_turn.lock().await;
            if let Some(at) = active.as_mut()
                && at.remove_task(&turn_context.sub_id)
            {
                should_clear_active_turn = true;
                let turn_state = Arc::clone(&at.turn_state);
                if should_clear_active_turn {
                    *active = None;
                }
                Some(turn_state)
            } else {
                None
            }
        };
        if let Some(turn_state) = turn_state {
            let mut ts = turn_state.lock().await;
            pending_input = ts.take_pending_input();
            turn_tool_calls = ts.tool_calls;
            token_usage_at_turn_start = Some(ts.token_usage_at_turn_start.clone());
        }
        if !pending_input.is_empty() {
            for pending_input_item in pending_input {
                match inspect_pending_input(self, &turn_context, pending_input_item).await {
                    PendingInputHookDisposition::Accepted(pending_input) => {
                        record_pending_input(self, &turn_context, *pending_input).await;
                    }
                    PendingInputHookDisposition::Blocked {
                        additional_contexts,
                    } => {
                        record_additional_contexts(self, &turn_context, additional_contexts).await;
                    }
                }
            }
        }
        // Emit token usage metrics.
        if let Some(token_usage_at_turn_start) = token_usage_at_turn_start {
            // TODO(jif): drop this
            let tmp_mem = (
                "tmp_mem_enabled",
                if self.enabled(Feature::MemoryTool) {
                    "true"
                } else {
                    "false"
                },
            );
            let network_proxy_active = match self.services.network_proxy.as_ref() {
                Some(started_network_proxy) => {
                    match started_network_proxy.proxy().current_cfg().await {
                        Ok(config) => config.network.enabled,
                        Err(err) => {
                            warn!(
                                "failed to read managed network proxy state for turn metrics: {err:#}"
                            );
                            false
                        }
                    }
                }
                None => false,
            };
            emit_turn_network_proxy_metric(
                &self.services.session_telemetry,
                network_proxy_active,
                tmp_mem,
            );
            self.services.session_telemetry.histogram(
                TURN_TOOL_CALL_METRIC,
                i64::try_from(turn_tool_calls).unwrap_or(i64::MAX),
                &[tmp_mem],
            );
            let total_token_usage = self.total_token_usage().await.unwrap_or_default();
            let turn_token_usage = TokenUsage {
                input_tokens: (total_token_usage.input_tokens
                    - token_usage_at_turn_start.input_tokens)
                    .max(0),
                cached_input_tokens: (total_token_usage.cached_input_tokens
                    - token_usage_at_turn_start.cached_input_tokens)
                    .max(0),
                output_tokens: (total_token_usage.output_tokens
                    - token_usage_at_turn_start.output_tokens)
                    .max(0),
                reasoning_output_tokens: (total_token_usage.reasoning_output_tokens
                    - token_usage_at_turn_start.reasoning_output_tokens)
                    .max(0),
                total_tokens: (total_token_usage.total_tokens
                    - token_usage_at_turn_start.total_tokens)
                    .max(0),
            };
            self.services.session_telemetry.histogram(
                TURN_TOKEN_USAGE_METRIC,
                turn_token_usage.total_tokens,
                &[("token_type", "total"), tmp_mem],
            );
            self.services.session_telemetry.histogram(
                TURN_TOKEN_USAGE_METRIC,
                turn_token_usage.input_tokens,
                &[("token_type", "input"), tmp_mem],
            );
            self.services.session_telemetry.histogram(
                TURN_TOKEN_USAGE_METRIC,
                turn_token_usage.cached_input(),
                &[("token_type", "cached_input"), tmp_mem],
            );
            self.services.session_telemetry.histogram(
                TURN_TOKEN_USAGE_METRIC,
                turn_token_usage.output_tokens,
                &[("token_type", "output"), tmp_mem],
            );
            self.services.session_telemetry.histogram(
                TURN_TOKEN_USAGE_METRIC,
                turn_token_usage.reasoning_output_tokens,
                &[("token_type", "reasoning_output"), tmp_mem],
            );
        }
        let (completed_at, duration_ms) = turn_context
            .turn_timing_state
            .completed_at_and_duration_ms()
            .await;
        let event = EventMsg::TurnComplete(TurnCompleteEvent {
            turn_id: turn_context.sub_id.clone(),
            last_agent_message,
            completed_at,
            duration_ms,
        });
        self.send_event(turn_context.as_ref(), event).await;

        if should_clear_active_turn {
            let session = Arc::clone(self);
            let _scheduler = tokio::task::spawn_blocking(move || {
                tokio::runtime::Handle::current().block_on(async move {
                    session.maybe_start_turn_for_pending_work().await;
                });
            });
        }
    }

    /// `Option::take` 위임 — ActiveTurn 슬롯을 한 번에 빼낸다(원자적).
    async fn take_active_turn(&self) -> Option<ActiveTurn> {
        let mut active = self.active_turn.lock().await;
        active.take()
    }

    /// 진행 중인 unified exec 프로세스를 모두 죽인다 — 위임 헬퍼.
    pub(crate) async fn close_unified_exec_processes(&self) {
        self.services
            .unified_exec_manager
            .terminate_all_processes()
            .await;
    }

    /// 인터럽트 후 정리 — 현재는 js_repl 커널 인터럽트만 처리.
    /// js_repl 매니저가 초기화돼 있을 때만 동작 (lazy init 일 수 있음).
    pub(crate) async fn cleanup_after_interrupt(&self, turn_context: &Arc<TurnContext>) {
        if let Some(manager) = turn_context.js_repl.manager_if_initialized()
            && let Err(err) = manager.interrupt_turn_exec(&turn_context.sub_id).await
        {
            warn!("failed to interrupt js_repl kernel: {err}");
        }
    }

    /// 한 RunningTask 를 협조적-그리고-필요시-강제로 종료.
    ///
    /// ### Layer 1 — What
    /// 1) 취소 토큰 켜기, 2) 100ms 동안 graceful 종료 대기, 3) 안 끝나면 강제 abort,
    /// 4) 인터럽트 사유면 history marker + rollout flush, 5) `TurnAborted` 이벤트.
    ///
    /// ### Layer 2 — How
    /// 1. 이미 cancelled 면 즉시 return — 중복 abort 방지.
    /// 2. `cancel_git_enrichment_task` — git 쪽도 함께 정리.
    /// 3. `tokio::select!` 로 두 갈래 경쟁: `done.notified()` (정상 종료 신호) vs
    ///    100ms 타이머. 타이머 이기면 경고 로그.
    /// 4. `handle.abort()` — 그래도 안 끝났으면 강제 종료.
    /// 5. `session_task.abort()` 호출 — 태스크별 cleanup hook.
    /// 6. Interrupted 면:
    ///    - cleanup_after_interrupt (js_repl 등)
    ///    - history 에 interrupted marker 한 줄 기록
    ///    - rollout 에 영구 저장
    ///    - **flush 대기** — 어떤 클라이언트는 TurnAborted 받자마자 rollout 을
    ///      재읽기 때문에, marker 가 디스크에 안착 후 이벤트를 보내야 일관성 유지.
    /// 7. duration 계산 후 `TurnAborted` 이벤트 발사.
    ///
    /// ### Layer 4 — Why
    /// "협조적 → 강제" 의 2단계는 **데이터 손상 방지** 차이. 협조적 cancel 은
    /// 태스크가 자기 critical section 을 빠져나올 시간을 준다. 강제 abort 는 critical
    /// section 한복판이라도 끊는다. 100ms 가 절충점.
    ///
    /// ### Layer 5 — Why Not
    /// - **즉시 abort?** → critical section 중간 종료 시 부분 쓰기/락 보유 leak 위험.
    /// - **무한 graceful 대기?** → 응답 안 하는 태스크가 사용자 인터럽트를 막음.
    /// - **history flush 안 기다림?** → 일부 클라이언트가 stale rollout 을 읽음.
    ///
    /// ### Layer 6 — Lesson
    /// 📌 *"외부에 보내는 종료 이벤트보다 disk 영속화가 먼저"* — 외부 시스템이
    /// 이벤트를 받자마자 의존하는 데이터를 다시 읽을 수 있다.
    /// 📌 *"강제 종료는 협조적 종료 후의 fallback"* — graceful → forced 의 2단계가
    /// 비동기 시스템의 표준. 교과서: *Two-Phase Termination Pattern*.
    async fn handle_task_abort(self: &Arc<Self>, task: RunningTask, reason: TurnAbortReason) {
        let sub_id = task.turn_context.sub_id.clone();
        if task.cancellation_token.is_cancelled() {
            return;
        }

        trace!(task_kind = ?task.kind, sub_id, "aborting running task");
        task.cancellation_token.cancel();
        task.turn_context
            .turn_metadata_state
            .cancel_git_enrichment_task();
        let session_task = task.task;

        select! {
            _ = task.done.notified() => {
            },
            _ = tokio::time::sleep(Duration::from_millis(GRACEFULL_INTERRUPTION_TIMEOUT_MS)) => {
                warn!("task {sub_id} didn't complete gracefully after {}ms", GRACEFULL_INTERRUPTION_TIMEOUT_MS);
            }
        }

        task.handle.abort();

        let session_ctx = Arc::new(SessionTaskContext::new(Arc::clone(self)));
        session_task
            .abort(session_ctx, Arc::clone(&task.turn_context))
            .await;

        if reason == TurnAbortReason::Interrupted {
            self.cleanup_after_interrupt(&task.turn_context).await;

            let marker = interrupted_turn_history_marker();
            self.record_into_history(std::slice::from_ref(&marker), task.turn_context.as_ref())
                .await;
            self.persist_rollout_items(&[RolloutItem::ResponseItem(marker)])
                .await;
            // Ensure the marker is durably visible before emitting TurnAborted: some clients
            // synchronously re-read the rollout on receipt of the abort event.
            if let Err(err) = self.flush_rollout().await {
                warn!("failed to flush interrupted-turn marker before emitting TurnAborted: {err}");
            }
        }

        let (completed_at, duration_ms) = task
            .turn_context
            .turn_timing_state
            .completed_at_and_duration_ms()
            .await;
        let event = EventMsg::TurnAborted(TurnAbortedEvent {
            turn_id: Some(task.turn_context.sub_id.clone()),
            reason,
            completed_at,
            duration_ms,
        });
        self.send_event(task.turn_context.as_ref(), event).await;
    }
}

// ---------------------------------------------------------------------------
// ## 🎓 What to Steal for Your Own Projects
//
// 1. **Strategy 트레잇 + dyn 래퍼 듀오** — 호출자에게 친화적인 `SessionTask` 와
//    object-safe 한 `AnySessionTask` 를 분리하고 blanket impl 로 잇는 패턴.
//    제로 비용 + 동적 디스패치 둘 다 지원.
// 2. **`SessionTaskContext` facade** — 큰 구조체(Session) 의 일부만 노출하는 작은
//    인터페이스로 결합도 ↓. 테스트 모킹/리팩터링이 쉬워진다.
// 3. **Two-Phase Termination** — `cancel_token.cancel()` (협조) → 100ms 대기 →
//    `handle.abort()` (강제). 데이터 손상 없이 빠른 종료를 보장.
// 4. **외부 이벤트 전송 전 disk flush** — `TurnAborted` 같은 이벤트를 보내기 전에
//    rollout 을 flush 해서 외부 시스템이 받자마자 stale 한 데이터를 보지 않도록.
// 5. **Idle 자동 와인딩 (`maybe_start_turn_for_pending_work`)** — 사용자 입력 없이도
//    큐에 쌓인 일감을 자동 처리. UX/시스템 효율 모두 개선.
// 6. **종료 ceremony 를 한 함수에 모으기 (`on_task_finished`)** — 메트릭/이벤트/큐
//    처리 순서가 자주 깨지는 영역. 한 곳에 가두면 새 메트릭 추가도 안전.
// 7. **`ActiveTurn::take()` 로 슬롯 통째 빼내기** — 락 안에서 한 번에 owned value
//    를 얻고 락을 풀어 처리. 락 holding 시간 최소화 + 처리 중 재진입 방지.
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
