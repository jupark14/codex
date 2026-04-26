//! Turn-scoped state and active turn metadata scaffolding.
//!
//! ## 📐 Architecture Overview
//!
//! 이 파일은 **"지금 이 턴(turn)이 머리에 들고 있는 메모지"** 를 정의한다.
//! 비유하면 식당 주방의 "현재 주문 보드" — 어떤 요리가 진행 중이고
//! (`RunningTask`), 손님이 추가로 부탁한 게 있는지(`pending_input`),
//! 결제는 어떻게 되는지(`pending_approvals`)를 한 보드에 모아둔다.
//!
//! ```text
//!  Session (영구 상태)
//!     └── ActiveTurn (한 턴 동안만 사는 메모지) ★ 이 파일
//!           ├── tasks: IndexMap<sub_id, RunningTask>   ← 백그라운드에서 도는 태스크들
//!           └── turn_state: Mutex<TurnState>           ← 승인 대기/입력 대기/usage 카운터
//! ```
//!
//! 데이터 플로우상 위치:
//! - 누가 만드는가: [[codex-rs/core/src/tasks/mod.rs::start_task]] 가 새 턴을 시작할 때.
//! - 누가 읽는가: 툴 디스패처([[codex-rs/core/src/tools/parallel.rs]]),
//!   스트림 핸들러([[codex-rs/core/src/stream_events_utils.rs]]) 가 사용자/모델
//!   응답을 기다릴 때 여기에 oneshot sender를 꽂아둔다.
//! - 누가 비우는가: 턴이 끝나거나(`on_task_finished`) 인터럽트되면
//!   `clear_pending` 으로 보드를 통째로 지운다.

use codex_sandboxing::policy_transforms::merge_permission_profiles;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use codex_protocol::dynamic_tools::DynamicToolResponse;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::request_permissions::RequestPermissionsResponse;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_rmcp_client::ElicitationResponse;
use rmcp::model::RequestId;
use tokio::sync::oneshot;

use crate::codex::TurnContext;
use crate::tasks::AnySessionTask;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::TokenUsage;

/// Metadata about the currently running turn.
///
/// ### Layer 1 — What
/// 한 턴 동안만 살아 있는 "주방 주문 보드". 진행 중 태스크 목록과
/// 그 턴에서 누적되는 모든 가변 상태(`TurnState`)를 한 묶음으로 들고 있다.
///
/// ### Layer 2 — How
/// 1. `tasks` 는 sub_id → `RunningTask` 매핑. 일반적으로 동시에 1개만 들어있지만
///    구조상 여러 개를 허용한다(legacy 호환).
/// 2. `turn_state` 는 `Arc<Mutex<...>>` 라 여러 비동기 태스크가 같은 보드에 동시
///    접근해도 안전하다. 💡 락은 `tokio::sync::Mutex` — 비동기 await 가능한 락이다.
///
/// ### Layer 3 — Macro Role
/// `Session::active_turn: Mutex<Option<ActiveTurn>>` 로 보관된다. `None` 이면 idle
/// 상태(턴 없음), `Some(...)` 이면 진행 중. 새 턴이 시작될 때
/// [[codex-rs/core/src/tasks/mod.rs::start_task]] 가 `default()` 로 만들어 꽂는다.
///
/// ### Layer 4 — Why
/// 턴 단위로 격리된 mutable state 가 필요했다. 세션 전체에 펼쳐 두면 이전 턴의
/// 잔여 상태가 새 턴에 새는 위험이 있고, 매번 함수 인자로 들고 다니면 시그니처가
/// 폭발한다 — 그래서 "턴 컨테이너" 한 개를 두고 거기에 모은다.
///
/// ### Layer 5 — Why Not
/// - **그냥 `TurnState` 만 두고 tasks 는 Session 에 두기?** → 턴이 끝났는데 태스크
///   핸들이 살아있는 어색한 윈도우가 생긴다. 함께 묶어두면 `take()` 한 번으로
///   "현재 턴 통째로 종료" 가 가능해진다.
/// - **`RwLock`?** → 거의 항상 write 라 RwLock 의 이점이 없다. Mutex 로 충분.
///
/// ### Layer 6 — Lesson
/// 📌 *"수명이 같은 가변 상태는 한 컨테이너에 묶고, 그 컨테이너를 `Option` 으로
/// 들고 있어라."* — 시작/종료가 한 줄(`*active = Some(..)` / `*active = None`) 로
/// 표현되어 race condition 없는 깔끔한 라이프사이클이 만들어진다.
/// 교과서: *Object Lifetime Pattern* 또는 *Resource Acquisition Is Initialization*.
pub(crate) struct ActiveTurn {
    pub(crate) tasks: IndexMap<String, RunningTask>,
    pub(crate) turn_state: Arc<Mutex<TurnState>>,
}

/// Whether mailbox deliveries should still be folded into the current turn.
///
/// State machine:
/// - A turn starts in `CurrentTurn`, so queued child mail can join the next
///   model request for that turn.
/// - After user-visible terminal output is recorded, we switch to `NextTurn`
///   to leave late child mail queued instead of extending an already shown
///   answer.
/// - If the same task later gets explicit same-turn work again (a steered user
///   prompt or a tool call after an untagged preamble), we reopen `CurrentTurn`
///   so that pending child mail is drained into that follow-up request.
///
/// ### Layer 1 — What
/// 자식 에이전트(child agent)들이 보내온 메일을 *지금 진행 중인* 모델 요청에
/// 끼워넣을지, 아니면 *다음* 턴까지 줄세워둘지를 가리는 작은 신호등.
///
/// ### Layer 2 — How
/// 비유하면 카페 주문 큐 옆의 **"지금 받는 중 / 그만 받음"** 표지판이다.
/// - 턴이 시작되면 표지판은 `CurrentTurn` 으로 켜진다 → 자식 메일이 지금 턴에 합류.
/// - 화면에 최종 답변이 나가는 순간 → `NextTurn` 으로 전환 (이미 보여준 답을
///   확장하지 않기 위해).
/// - 같은 태스크가 다시 명시적 후속 작업을 받으면 → 표지판을 다시 `CurrentTurn`
///   으로 켜서 그 사이 줄선 메일을 흡수한다.
///
/// ### Layer 3 — Macro Role
/// `TurnState::mailbox_delivery_phase` 한 필드로 들고 있다. 모델 호출을 만들기
/// 직전에 [[codex-rs/core/src/codex.rs::Session::accepts_mailbox_delivery_for_current_turn]]
/// 가 이 값을 보고 mailbox 큐를 비울지 말지 결정한다.
///
/// ### Layer 4 — Why
/// "최종 답변 출력 후 도착한 자식 메일" 이 갑자기 화면에 추가되는 UX 사고를 막기
/// 위해 만들어진 가드. 내부적으로는 race condition 이 아니라 *의도된 게이트* 다.
///
/// ### Layer 5 — Why Not
/// - **bool 플래그?** → 의미가 모호해진다. 두 상태에 명확한 이름(`CurrentTurn`/
///   `NextTurn`)을 붙여주는 enum 이 self-documenting.
/// - **시간 기반(타임스탬프)?** → "언제부터 다음 턴인가" 의 컷오프가 흐려진다.
///   상태 전환 이벤트로 끊는 편이 더 정확하다.
///
/// ### Layer 6 — Lesson
/// 📌 *"두 상태짜리 신호등도 bool 보다 enum 이 낫다"* — 변수명/필드명은 거짓말할
/// 수 있어도 enum variant 이름은 호출 지점에서 자동으로 의미를 드러낸다.
/// 교과서: *State Pattern (mini)* 또는 *Make Illegal States Unrepresentable*.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MailboxDeliveryPhase {
    /// Incoming mailbox messages can still be consumed by the current turn.
    #[default]
    CurrentTurn,
    /// The current turn already emitted visible final answer text; mailbox
    /// messages should remain queued for a later turn.
    NextTurn,
}

impl Default for ActiveTurn {
    fn default() -> Self {
        Self {
            tasks: IndexMap::new(),
            turn_state: Arc::new(Mutex::new(TurnState::default())),
        }
    }
}

/// What kind of work the task represents — 텔레메트리/UI 라벨 용도.
/// `Regular` 는 일반 채팅, `Review` 는 review 워크플로우, `Compact` 는 컨텍스트
/// 압축. 라우팅에 영향을 주진 않고, 분류용 태그.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TaskKind {
    Regular,
    Review,
    Compact,
}

/// 백그라운드에서 실제로 돌고 있는 한 태스크의 핸들.
///
/// ### Layer 1 — What
/// "현재 진행 중인 한 건의 일감" 을 추적하는 묶음. 끝났음을 알리는 종소리(`done`),
/// 중간 취소 버튼(`cancellation_token`), 강제 종료 핸들(`handle`), 그리고 그 일감이
/// 사용 중인 컨텍스트(`turn_context`) 가 포함된다.
///
/// ### Layer 2 — How
/// 1. `tokio::spawn` 으로 띄운 future 의 `JoinHandle` 을 `AbortOnDropHandle` 로
///    감싸 **이 구조체가 drop 되면 태스크도 자동으로 abort** 되게 한다. 💡 RAII.
/// 2. `done: Arc<Notify>` — 다른 코드가 "끝나면 깨워줘" 하고 await 할 수 있는
///    원샷(?) 신호. 비유하자면 호출벨.
/// 3. `cancellation_token` 은 협조적 종료 신호. abort 와 달리 태스크가 직접
///    "취소됐는지" 폴링해서 깔끔히 마무리할 기회를 준다.
/// 4. `_timer` 는 `Drop` 시 OTEL 메트릭을 기록하기 위한 가드. 변수명이 `_` 로
///    시작 → ⚠️ "안 쓰지만 라이프타임만 잡아두는 가드" 임을 표시.
///
/// ### Layer 3 — Macro Role
/// [[codex-rs/core/src/tasks/mod.rs::start_task]] 가 만들어 `ActiveTurn::tasks`
/// 에 꽂는다. abort 시 [[codex-rs/core/src/tasks/mod.rs::handle_task_abort]] 가
/// 꺼내서 graceful shutdown → handle.abort() 의 2단계 종료를 진행한다.
///
/// ### Layer 4 — Why
/// 비동기 태스크 라이프사이클의 4가지 관심사(완료 알림 / 협조적 취소 / 강제 중단 /
/// 컨텍스트 보관)를 한 객체에 모았다. 이게 흩어져 있으면 abort 경로가 4곳을 따로
/// 건드려야 해서 누락 위험이 크다.
///
/// ### Layer 5 — Why Not
/// - **`JoinHandle` 직접 보관?** → drop 해도 태스크가 안 멈춘다. 누수 위험.
///   `AbortOnDropHandle` 로 감싸야 안전.
/// - **`done` 을 `oneshot::channel` 로?** → oneshot 은 한 번만 받을 수 있다.
///   `Notify` 는 여러 waiter 가 동시에 깨어날 수 있어 더 유연.
///
/// ### Layer 6 — Lesson
/// 📌 *"비동기 태스크의 모든 끝맺음 수단을 한 구조체에 모아라"* — 종료 경로가
/// 분산되면 반드시 어딘가에서 leak 난다. 교과서적으로는 *"Cancellation 은
/// 자료구조의 일부다"* — Tokio 권장 패턴.
pub(crate) struct RunningTask {
    pub(crate) done: Arc<Notify>,
    pub(crate) kind: TaskKind,
    pub(crate) task: Arc<dyn AnySessionTask>,
    pub(crate) cancellation_token: CancellationToken,
    pub(crate) handle: Arc<AbortOnDropHandle<()>>,
    pub(crate) turn_context: Arc<TurnContext>,
    // Timer recorded when the task drops to capture the full turn duration.
    pub(crate) _timer: Option<codex_otel::Timer>,
}

impl ActiveTurn {
    /// 새 태스크를 보드에 등록한다 — sub_id 가 키. 단순 삽입.
    pub(crate) fn add_task(&mut self, task: RunningTask) {
        let sub_id = task.turn_context.sub_id.clone();
        self.tasks.insert(sub_id, task);
    }

    /// 끝난 태스크를 보드에서 제거하고, "이제 보드가 비었나?" 를 반환.
    /// ⚠️ `swap_remove` 는 순서를 안 보존한다 — IndexMap 의 O(1) 삭제 트릭.
    /// 호출자는 반환값이 true 면 `ActiveTurn` 자체를 `None` 으로 비울 수 있다.
    pub(crate) fn remove_task(&mut self, sub_id: &str) -> bool {
        self.tasks.swap_remove(sub_id);
        self.tasks.is_empty()
    }

    /// 보드의 모든 태스크를 빼서 Vec 으로 돌려준다 — abort 경로에서 사용.
    /// 💡 핵심 트릭: `RunningTask` 는 drop 시 자동 abort 되므로, 호출자가 Vec 을
    /// 그냥 떨어뜨려도(=drop) 태스크들이 정리된다.
    pub(crate) fn drain_tasks(&mut self) -> Vec<RunningTask> {
        self.tasks.drain(..).map(|(_, task)| task).collect()
    }
}

/// Mutable state for a single turn.
///
/// ### Layer 1 — What
/// 한 턴이 모델/사용자/MCP 응답을 *기다리는 동안* 들고 있어야 할 모든 가변 상태.
/// 5종류의 "응답 대기 중" 박스 + 입력 큐 + 메일박스 신호등 + 권한 누적 + 카운터.
///
/// ### Layer 2 — How
/// 비유하면 콜센터 상담원의 **데스크 오거나이저** 같다:
/// 1. `pending_approvals` — *"사용자, 이 명령 실행해도 되나요?"* 답을 기다리는 슬롯.
///    key: 호출 ID, value: 답이 오면 깨워주는 oneshot 송신부.
/// 2. `pending_request_permissions` / `pending_user_input` — 같은 패턴, 다른 응답 타입.
/// 3. `pending_elicitations` — MCP 서버 elicitation. 키가 (server, RequestId) 튜플인 게
///    포인트 — 같은 RequestId 가 서로 다른 서버에서 충돌할 수 있다.
/// 4. `pending_dynamic_tools` — 동적 툴 정의 응답.
/// 5. `pending_input` — 다음 모델 호출에 합칠 입력 큐.
/// 6. `mailbox_delivery_phase` — 자식 메일을 지금 턴에 줄지 말지(앞에서 본 신호등).
/// 7. `granted_permissions` — 이 턴에서 누적 승인된 권한.
/// 8. `tool_calls` / `token_usage_at_turn_start` — 텔레메트리 카운터/스냅샷.
///
/// ### Layer 3 — Macro Role
/// `ActiveTurn::turn_state` 안에 `Arc<Mutex<TurnState>>` 로 들어 있어, 여러 비동기
/// 갈래가 같은 보드에 동시에 끼어들 수 있다. 모든 `pending_X` 슬롯의 수신부는
/// 호출자의 await 지점에서 잠을 잔다 — 답이 도착하면 sender 가 발사되어 깨운다.
///
/// ### Layer 4 — Why
/// 비동기 요청–응답 매칭 패턴. 호출자가 *"답이 올 때까지 await"* 하려면 그 답을
/// 다시 찾아갈 수 있어야 하고, 그 라우팅 테이블이 바로 이 HashMap 들이다.
/// 비유: 콜센터에서 손님 번호표 ↔ 통화 슬롯.
///
/// ### Layer 5 — Why Not
/// - **글로벌 채널 1개 + 라우팅?** → 응답 타입이 5가지로 다르다. 타입별 채널을 두면
///   타입 안전성이 자동으로 따라온다(잘못된 응답 타입을 못 꽂는다).
/// - **`Mutex<HashMap>` 각각 따로?** → 락이 5개로 늘어 데드락 가능성과 holding
///   비용이 커진다. 한 Mutex 로 묶어두는 편이 단순.
///
/// ### Layer 6 — Lesson
/// 📌 *"비동기 요청-응답 매칭은 `HashMap<key, oneshot::Sender<Resp>>` 패턴으로
/// 풀 수 있다"* — 호출자는 sender 를 등록하고 receiver 를 await, 응답 디스패처는
/// 키로 sender 를 꺼내 `send(resp)` 만 하면 끝. 교과서: *Reactor / Continuation
/// Passing for async I/O*.
#[derive(Default)]
pub(crate) struct TurnState {
    pending_approvals: HashMap<String, oneshot::Sender<ReviewDecision>>,
    pending_request_permissions: HashMap<String, oneshot::Sender<RequestPermissionsResponse>>,
    pending_user_input: HashMap<String, oneshot::Sender<RequestUserInputResponse>>,
    pending_elicitations: HashMap<(String, RequestId), oneshot::Sender<ElicitationResponse>>,
    pending_dynamic_tools: HashMap<String, oneshot::Sender<DynamicToolResponse>>,
    pending_input: Vec<ResponseInputItem>,
    mailbox_delivery_phase: MailboxDeliveryPhase,
    granted_permissions: Option<PermissionProfile>,
    pub(crate) tool_calls: u64,
    pub(crate) token_usage_at_turn_start: TokenUsage,
}

impl TurnState {
    // 아래 `insert_pending_*` / `remove_pending_*` 들은 모두 동일 패턴의
    // 박스/꺼냄 헬퍼다. 각각의 응답 타입(ReviewDecision, RequestPermissionsResponse,
    // RequestUserInputResponse, ElicitationResponse, DynamicToolResponse) 별로 분리되어
    // 타입 안전성을 보장. 본질은 `HashMap::insert` / `HashMap::remove` 의 얇은 래퍼.

    /// 단순 래퍼 — 승인 요청 슬롯에 sender 를 꽂는다. 같은 키가 있으면 이전 sender
    /// 를 반환(호출자가 처리해야 함, 보통 drop = receiver 쪽이 RecvError 받음).
    pub(crate) fn insert_pending_approval(
        &mut self,
        key: String,
        tx: oneshot::Sender<ReviewDecision>,
    ) -> Option<oneshot::Sender<ReviewDecision>> {
        self.pending_approvals.insert(key, tx)
    }

    /// 단순 래퍼 — 키로 승인 sender 를 꺼낸다. 응답 디스패처가 호출.
    pub(crate) fn remove_pending_approval(
        &mut self,
        key: &str,
    ) -> Option<oneshot::Sender<ReviewDecision>> {
        self.pending_approvals.remove(key)
    }

    /// 턴 종료/abort 시 모든 대기 슬롯과 입력 큐를 한 번에 비운다.
    /// ⚠️ 비운 sender 들이 drop 되면서 receiver 들은 자동으로 RecvError 를 받는다 —
    /// 호출자(await 중인 쪽)가 그걸로 "취소됐구나" 를 감지한다. 별도 신호 안 보내도 됨.
    pub(crate) fn clear_pending(&mut self) {
        self.pending_approvals.clear();
        self.pending_request_permissions.clear();
        self.pending_user_input.clear();
        self.pending_elicitations.clear();
        self.pending_dynamic_tools.clear();
        self.pending_input.clear();
    }

    // `insert_pending_request_permissions` / `remove_*` ─ 권한 요청 응답 슬롯 헬퍼.
    pub(crate) fn insert_pending_request_permissions(
        &mut self,
        key: String,
        tx: oneshot::Sender<RequestPermissionsResponse>,
    ) -> Option<oneshot::Sender<RequestPermissionsResponse>> {
        self.pending_request_permissions.insert(key, tx)
    }

    pub(crate) fn remove_pending_request_permissions(
        &mut self,
        key: &str,
    ) -> Option<oneshot::Sender<RequestPermissionsResponse>> {
        self.pending_request_permissions.remove(key)
    }

    // `insert_pending_user_input` / `remove_*` ─ 사용자 추가 입력 응답 슬롯 헬퍼.
    pub(crate) fn insert_pending_user_input(
        &mut self,
        key: String,
        tx: oneshot::Sender<RequestUserInputResponse>,
    ) -> Option<oneshot::Sender<RequestUserInputResponse>> {
        self.pending_user_input.insert(key, tx)
    }

    pub(crate) fn remove_pending_user_input(
        &mut self,
        key: &str,
    ) -> Option<oneshot::Sender<RequestUserInputResponse>> {
        self.pending_user_input.remove(key)
    }

    /// MCP elicitation 슬롯 — 키가 (server_name, RequestId) 튜플인 게 포인트.
    /// ⚠️ 같은 RequestId 가 여러 MCP 서버에서 동시에 떠다닐 수 있으므로 server_name
    /// 까지 합쳐야 충돌 없는 키가 된다.
    pub(crate) fn insert_pending_elicitation(
        &mut self,
        server_name: String,
        request_id: RequestId,
        tx: oneshot::Sender<ElicitationResponse>,
    ) -> Option<oneshot::Sender<ElicitationResponse>> {
        self.pending_elicitations
            .insert((server_name, request_id), tx)
    }

    pub(crate) fn remove_pending_elicitation(
        &mut self,
        server_name: &str,
        request_id: &RequestId,
    ) -> Option<oneshot::Sender<ElicitationResponse>> {
        self.pending_elicitations
            .remove(&(server_name.to_string(), request_id.clone()))
    }

    // `insert_pending_dynamic_tool` / `remove_*` ─ 동적 툴 정의 응답 슬롯 헬퍼.
    pub(crate) fn insert_pending_dynamic_tool(
        &mut self,
        key: String,
        tx: oneshot::Sender<DynamicToolResponse>,
    ) -> Option<oneshot::Sender<DynamicToolResponse>> {
        self.pending_dynamic_tools.insert(key, tx)
    }

    pub(crate) fn remove_pending_dynamic_tool(
        &mut self,
        key: &str,
    ) -> Option<oneshot::Sender<DynamicToolResponse>> {
        self.pending_dynamic_tools.remove(key)
    }

    /// 다음 모델 호출에 합칠 입력 한 건을 큐 끝에 붙인다.
    pub(crate) fn push_pending_input(&mut self, input: ResponseInputItem) {
        self.pending_input.push(input);
    }

    /// 새 입력 묶음을 *큐 앞쪽*에 끼워넣는다. 일반 push 와 달리 우선순위가 높은
    /// 입력(예: 인터럽트 후 재개되는 메시지)을 다음 모델 호출에 먼저 보내야 할 때 사용.
    /// ### Why
    /// `Vec::splice` 대신 `append + 교체` 트릭을 쓰는 이유는, 새 입력이 비어있을 때
    /// 의미 없는 reallocation 을 피하기 위함이다. 💡 빈 벡터 가드(`is_empty` 검사)
    /// 가 이를 보장.
    pub(crate) fn prepend_pending_input(&mut self, mut input: Vec<ResponseInputItem>) {
        if input.is_empty() {
            return;
        }

        input.append(&mut self.pending_input);
        self.pending_input = input;
    }

    /// 큐를 통째로 비우고 그 내용을 반환한다.
    /// ### Why
    /// `std::mem::swap` 으로 자리만 바꾸면 `clone` 없이 owned Vec 을 빼올 수 있다.
    /// 빈 큐일 땐 `Vec::with_capacity(0)` 으로 바로 빈 Vec 반환 — 미세한 alloc 절약.
    pub(crate) fn take_pending_input(&mut self) -> Vec<ResponseInputItem> {
        if self.pending_input.is_empty() {
            Vec::with_capacity(0)
        } else {
            let mut ret = Vec::new();
            std::mem::swap(&mut ret, &mut self.pending_input);
            ret
        }
    }

    /// 단순 조회 — 입력 큐에 뭐라도 있나?
    pub(crate) fn has_pending_input(&self) -> bool {
        !self.pending_input.is_empty()
    }

    /// 신호등을 `CurrentTurn` 으로 켠다 — 다음 자식 메일을 지금 턴에 흡수하라.
    pub(crate) fn accept_mailbox_delivery_for_current_turn(&mut self) {
        self.set_mailbox_delivery_phase(MailboxDeliveryPhase::CurrentTurn);
    }

    /// 신호등이 `CurrentTurn` 인지 확인.
    pub(crate) fn accepts_mailbox_delivery_for_current_turn(&self) -> bool {
        self.mailbox_delivery_phase == MailboxDeliveryPhase::CurrentTurn
    }

    /// 신호등을 직접 설정 — `NextTurn` 으로 끄는 경로에서 호출.
    pub(crate) fn set_mailbox_delivery_phase(&mut self, phase: MailboxDeliveryPhase) {
        self.mailbox_delivery_phase = phase;
    }

    /// 사용자가 새로 승인해준 권한을 기존 누적 권한과 합쳐 저장한다.
    /// ### Why
    /// 단순 덮어쓰기가 아니라 `merge_permission_profiles` 로 합쳐야, 같은 턴 안에서
    /// 사용자가 "Read 만 OK" → "Write 도 OK" 로 점진 확장한 경우의 합집합이 유지된다.
    pub(crate) fn record_granted_permissions(&mut self, permissions: PermissionProfile) {
        self.granted_permissions =
            merge_permission_profiles(self.granted_permissions.as_ref(), Some(&permissions));
    }

    /// 누적된 승인 권한 스냅샷을 클론으로 반환 — 호출자가 자유롭게 들고 다닐 수 있게.
    pub(crate) fn granted_permissions(&self) -> Option<PermissionProfile> {
        self.granted_permissions.clone()
    }
}

impl ActiveTurn {
    /// Clear any pending approvals and input buffered for the current turn.
    ///
    /// ### Why
    /// `&self` 만 받는 비동기 메서드 — 내부 락(`Mutex<TurnState>`) 을 잡고 위임한다.
    /// 호출자는 `&mut ActiveTurn` 이 없어도 호출 가능하므로, abort 경로처럼 여러
    /// 곳에서 동시에 들어와도 안전. 비유하면 "공용 화이트보드를 잠시 잠가두고 지운다."
    pub(crate) async fn clear_pending(&self) {
        let mut ts = self.turn_state.lock().await;
        ts.clear_pending();
    }
}

// ---------------------------------------------------------------------------
// ## 🎓 What to Steal for Your Own Projects
//
// 1. **턴-스코프 컨테이너 패턴** — 수명이 같은 mutable 상태들은 한 구조체에 묶고,
//    그 구조체를 `Mutex<Option<...>>` 로 보관. 시작/종료가 한 줄로 깔끔해진다.
// 2. **`AbortOnDropHandle` + `RAII`** — 비동기 태스크 핸들을 그냥 들고 있으면
//    drop 해도 안 멈추는 leak 위험이 있다. drop 시 자동 abort 되는 래퍼로 감싸라.
// 3. **`HashMap<key, oneshot::Sender<Resp>>` 라우팅** — 비동기 요청-응답 매칭의
//    공식. 응답 디스패처는 키만 보고 sender 를 꺼내 발사하면 끝. 호출자는 await.
// 4. **`drop = cancel` 시멘틱** — `clear_pending` 에서 sender 들이 drop 되면 receiver
//    가 RecvError 를 받는다. 별도 "취소 신호" 채널을 만들 필요가 없다.
// 5. **`std::mem::swap` 으로 owned 값 추출** — `&mut self` 에서 내부 Vec 을 비우면서
//    내용을 owned 로 받아내고 싶을 때, `clone` 없이 swap 한 번이면 끝. 메모리/CPU 절약.
// ---------------------------------------------------------------------------
