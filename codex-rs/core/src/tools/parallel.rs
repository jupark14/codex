//! ## 📐 Architecture Overview
//!
//! 이 파일은 **"한 턴에서 모델이 여러 툴을 동시에 호출할 때, 어떤 건 같이 돌리고
//! 어떤 건 줄 세울지"** 를 결정하는 작은 디스패처를 담고 있다.
//!
//! 비유하면 **공유 주방의 조리대 1개**를 떠올리면 쉽다:
//! - 어떤 요리(`supports_parallel == true`)는 옆에서 동시에 해도 된다 → 여러 명이
//!   조리대에서 같이 작업 (RwLock 의 `read` 락 = 다중 동시 접근 허용).
//! - 어떤 요리(`supports_parallel == false`)는 조리대를 *통째로* 잡고 해야 한다
//!   (예: 파일시스템을 변경하는 `apply_patch` 같은 부작용 도구) → 한 명만
//!   (RwLock 의 `write` 락 = 단독 점유).
//!
//! ```text
//!   handle_output_item_done (stream_events_utils.rs)
//!         │
//!         ▼  ToolCall 만들어서
//!   ToolCallRuntime::handle_tool_call ★ 이 파일
//!         │
//!         ▼  parallel-safe ?
//!     ┌───┴────┐
//!     │  YES   │  NO
//!     │ read   │  write
//!     │ lock   │  lock
//!     ▼        ▼
//!   ToolRouter::dispatch_tool_call_with_code_mode_result
//!         │
//!         ▼
//!   AnyToolResult  →  ResponseInputItem
//! ```
//!
//! 데이터 플로우:
//! - 호출자: [[codex-rs/core/src/stream_events_utils.rs::handle_output_item_done]]
//! - 위임 대상: [[codex-rs/core/src/tools/router.rs::ToolRouter::dispatch_tool_call_with_code_mode_result]]
//! - 결과 소비자: 같은 호출자의 OutputItemResult.tool_future

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tokio_util::either::Either;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;
use tracing::instrument;
use tracing::trace_span;

use crate::codex::Session;
use crate::codex::TurnContext;
use crate::function_tool::FunctionCallError;
use crate::tools::context::AbortedToolOutput;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::context::ToolPayload;
use crate::tools::registry::AnyToolResult;
use crate::tools::router::ToolCall;
use crate::tools::router::ToolCallSource;
use crate::tools::router::ToolRouter;
use codex_protocol::error::CodexErr;
use codex_protocol::models::ResponseInputItem;
use codex_tools::ToolSpec;

/// 한 턴이 사용하는 "툴 호출 발사대".
///
/// ### Layer 1 — What
/// 모델이 내뱉는 툴 호출 하나를 받아, **다른 진행 중 툴 호출과 충돌하지 않도록**
/// 락으로 게이팅해서 실제 디스패처([`ToolRouter`])에게 넘겨주는 얇은 코디네이터.
///
/// ### Layer 2 — How
/// 1. `Clone` 가능 — 내부 필드가 모두 `Arc` 라 cheap copy. 동시 호출들이 같은
///    runtime 인스턴스를 공유한다.
/// 2. `parallel_execution: Arc<RwLock<()>>` — 💡 **값이 unit `()` 인 RwLock!** 보호
///    대상 데이터가 없는 *순수 게이트* 다. 락의 read/write 권한 그 자체만 사용.
///    - `read().await` = "나는 다른 parallel-safe 툴들과 같이 돌아도 됨"
///    - `write().await` = "나는 단독으로 돌아야 함"
///
/// ### Layer 3 — Macro Role
/// [[codex-rs/core/src/codex.rs]] 가 턴 시작 시 만들어 `ToolCallRuntime::new` 로
/// 부트하고, [[codex-rs/core/src/stream_events_utils.rs::handle_output_item_done]]
/// 가 모델의 tool_call 이벤트마다 `handle_tool_call` 을 호출한다.
///
/// ### Layer 4 — Why
/// 툴마다 동시 실행 가능 여부가 다른데, 그걸 매번 호출자가 신경 쓰면 코드 폭발.
/// 이 객체가 단일 진입점에서 룰을 강제 → 호출자는 그냥 `handle_tool_call` 만 부르면
/// 안전성이 자동 보장된다.
///
/// ### Layer 5 — Why Not
/// - **`Mutex<()>` 만으로?** → 모든 툴이 직렬화되어 병렬성 손실. RwLock 의
///   "다중 reader" 특성이 핵심.
/// - **툴별 분리된 락?** → 어떤 툴 조합이 안전한지 정의해야 함 → 폭발적 복잡도.
///   "전체적으로 한 명만 또는 여러 명 OK" 의 단순 모델이 유지보수 비용이 낮다.
/// - **세마포어?** → 카운팅 한도가 필요 없는 경우라 RwLock 이 더 자연스럽다.
///
/// ### Layer 6 — Lesson
/// 📌 *"`RwLock<()>` 패턴 — 락을 동시성 게이트로 쓰기"* — 보호할 데이터가 없어도
/// "공존 OK / 단독 점유" 라는 두 가지 모드의 직관적 표현이 필요할 때 RwLock 의
/// 상태 전이를 그대로 차용하는 트릭. 교과서: *Reader-Writer Lock as a Synchronization
/// Primitive* (Tanenbaum, *Modern Operating Systems*).
#[derive(Clone)]
pub(crate) struct ToolCallRuntime {
    router: Arc<ToolRouter>,
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    tracker: SharedTurnDiffTracker,
    parallel_execution: Arc<RwLock<()>>,
}

impl ToolCallRuntime {
    /// 생성자 — 라우터/세션/턴/diff tracker 를 받아 런타임을 만든다.
    /// 락은 새로 만들어 이 런타임 인스턴스 단위로 격리.
    pub(crate) fn new(
        router: Arc<ToolRouter>,
        session: Arc<Session>,
        turn_context: Arc<TurnContext>,
        tracker: SharedTurnDiffTracker,
    ) -> Self {
        Self {
            router,
            session,
            turn_context,
            tracker,
            parallel_execution: Arc::new(RwLock::new(())),
        }
    }

    /// 단순 위임 — 툴 이름으로 ToolSpec 조회.
    pub(crate) fn find_spec(&self, tool_name: &str) -> Option<ToolSpec> {
        self.router.find_spec(tool_name)
    }

    /// 외부에서 보는 진입점 — 툴 호출 1건을 실행하고 모델로 돌려보낼
    /// `ResponseInputItem` 을 반환한다.
    ///
    /// ### Layer 1 — What
    /// "이 툴 호출을 안전하게 돌려서 결과 메시지로 만들어줘" 의 표준 진입점.
    ///
    /// ### Layer 2 — How
    /// 1. 호출 정보를 미리 한 번 clone — 실패 시 에러 응답을 빚어낼 백업으로 사용.
    /// 2. 내부 헬퍼 `handle_tool_call_with_source` 에 위임 (소스를 `Direct` 로 표기).
    /// 3. 결과 매칭:
    ///    - `Ok(response)` → `into_response()` 로 모델용 형태로 변환
    ///    - `Err(Fatal)` → 그대로 위로 전파 (턴 종료 사유)
    ///    - 그 외 에러 → 실패 응답으로 변환해 모델에게 돌려줌 (턴은 계속)
    ///
    /// ### Layer 3 — Macro Role
    /// `stream_events_utils.rs::handle_output_item_done` 안에서 `Box::pin` 으로
    /// 감싸져 `OutputItemResult.tool_future` 에 들어간다 — 호출자는 모델 스트림을
    /// 계속 처리하면서 백그라운드에서 툴을 실행시킨다.
    ///
    /// ### Layer 4 — Why
    /// Fatal 에러와 일반 에러를 구분 처리. Fatal 은 턴을 죽일 수 있어야 하지만, 일반
    /// 에러(권한 거부 등)는 모델에게 알려주고 턴은 살려야 한다 → 두 결과 타입을
    /// 한 진입점에서 흡수해 호출자 코드를 단순화.
    ///
    /// ### Layer 5 — Why Not
    /// - **에러 변환을 호출자에게 맡기기?** → 모든 호출 지점에 동일 패턴 복제 →
    ///   누락 시 잘못된 메시지 형태로 모델에 전송 → 모델 혼란.
    /// - **`?` 연산자로 단순화?** → Fatal 만 propagate 하고 나머지는 흡수해야 해서
    ///   match 가 더 명확하다.
    ///
    /// ### Layer 6 — Lesson
    /// 📌 *"두 종류 에러(치명/회복가능)는 진입점에서 한 번에 정렬하라"* — 위로 올릴 것/
    /// 결과로 변환할 것을 분리해 호출자 시그니처를 단순하게 유지.
    /// 교과서: *Errors as Values* (Go), *Two-Track Programming* (F# Railway-Oriented).
    #[instrument(level = "trace", skip_all)]
    pub(crate) fn handle_tool_call(
        self,
        call: ToolCall,
        cancellation_token: CancellationToken,
    ) -> impl std::future::Future<Output = Result<ResponseInputItem, CodexErr>> {
        let error_call = call.clone();
        let future =
            self.handle_tool_call_with_source(call, ToolCallSource::Direct, cancellation_token);
        async move {
            match future.await {
                Ok(response) => Ok(response.into_response()),
                Err(FunctionCallError::Fatal(message)) => Err(CodexErr::Fatal(message)),
                Err(other) => Ok(Self::failure_response(error_call, other)),
            }
        }
        .in_current_span()
    }

    /// 진짜 알맹이 — 락 게이팅 + 디스패치 + 취소 처리를 한꺼번에 한다.
    ///
    /// ### Layer 1 — What
    /// 1) 툴이 parallel-safe 인지 보고 적절한 락(read/write)을 잡고,
    /// 2) `tokio::spawn` 으로 새 태스크에서 디스패처를 돌리고,
    /// 3) `cancellation_token` 이 켜지면 그 자리에서 abort-응답을 만들어 반환.
    ///
    /// ### Layer 2 — How
    /// 1. `supports_parallel` 플래그를 읽는다.
    /// 2. 모든 의존성을 `Arc` 로 clone — spawn 한 태스크가 `'static` 라이프타임을
    ///    요구하기 때문. 💡 self 의 필드를 빌리지 않고 모두 owned `Arc` 사본으로 옮긴다.
    /// 3. `dispatch_span` 으로 OTEL trace span 을 만들어 호출 메타(툴 이름/call_id)
    ///    를 기록.
    /// 4. `tokio::spawn` 안에서 `tokio::select!` 로 두 갈래를 경쟁:
    ///    - 취소 토큰 발사 → abort 응답 생성
    ///    - 정상 경로 → `Either::Left(read)` / `Either::Right(write)` 락 잡고
    ///      `dispatch_tool_call_with_code_mode_result` 호출.
    /// 5. 락은 `_guard` 로 잡아두고 디스패치가 끝날 때까지 유지 → drop 시 자동 해제.
    /// 6. spawn 핸들을 `AbortOnDropHandle` 로 감싸 누수 방지.
    ///
    /// ### Layer 3 — Macro Role
    /// 모델이 한 턴에 N개의 tool_call 이벤트를 흘리면, 이 메서드가 N번 spawn 되어
    /// 동시에 도는 워커 태스크들이 같은 RwLock 으로 정렬된다.
    ///
    /// ### Layer 4 — Why
    /// `tokio::select!` 에서 cancel 을 가장 위쪽 가지로 둔 이유는 — 디스패치가 락
    /// 대기 중이라도 즉시 빠져나올 수 있게 하기 위함. 사용자 인터럽트 응답 latency 를
    /// 락 경합 시간에 묶지 않는다.
    ///
    /// ### Layer 5 — Why Not
    /// - **`Either<RwLockReadGuard, RwLockWriteGuard>`?** → 두 타입은 같은 트레잇만
    ///   구현하면 충분하고, 호출 후 drop 만 하면 되므로 enum 으로 합쳐 보관 가능 →
    ///   바로 `Either` 사용. 별도 trait object (`Box<dyn Drop>`) 보다 stack-allocated
    ///   `Either` 가 alloc 없음.
    /// - **spawn 없이 await 직접?** → cancel 을 `select!` 로 받기 어렵고, 취소 시
    ///   디스패치 스택이 풀릴 때까지 기다려야 한다. spawn 으로 격리하면 abort 가능.
    ///
    /// ### Layer 6 — Lesson
    /// 📌 *"`select!` + `CancellationToken` + `AbortOnDropHandle` 3종 세트가 비동기
    /// 작업의 강제 취소를 안전하게 만든다"* — 신호 받기, 협조적 종료, 강제 종료의
    /// 세 단계를 분리해 두면 race-free cancellation 이 자연스럽게 따라온다.
    /// 교과서: *Structured Concurrency* (Nathaniel J. Smith, *Notes on structured
    /// concurrency*).
    #[instrument(level = "trace", skip_all)]
    pub(crate) fn handle_tool_call_with_source(
        self,
        call: ToolCall,
        source: ToolCallSource,
        cancellation_token: CancellationToken,
    ) -> impl std::future::Future<Output = Result<AnyToolResult, FunctionCallError>> {
        let supports_parallel = self.router.tool_supports_parallel(&call.tool_name);
        let router = Arc::clone(&self.router);
        let session = Arc::clone(&self.session);
        let turn = Arc::clone(&self.turn_context);
        let tracker = Arc::clone(&self.tracker);
        let lock = Arc::clone(&self.parallel_execution);
        let started = Instant::now();
        let display_name = call.tool_name.display();

        let dispatch_span = trace_span!(
            "dispatch_tool_call_with_code_mode_result",
            otel.name = display_name.as_str(),
            tool_name = display_name.as_str(),
            call_id = call.call_id.as_str(),
            aborted = false,
        );

        let handle: AbortOnDropHandle<Result<AnyToolResult, FunctionCallError>> =
            AbortOnDropHandle::new(tokio::spawn(async move {
                tokio::select! {
                    // 취소 신호가 먼저 오면 락도 안 잡고 abort 응답을 만든다.
                    // ⚠️ `started.elapsed()` 가 0초로 보고되면 클라이언트가 0/NaN
                    // 처리를 잘못할 위험이 있어 `.max(0.1)` 으로 floor.
                    _ = cancellation_token.cancelled() => {
                        let secs = started.elapsed().as_secs_f32().max(0.1);
                        dispatch_span.record("aborted", true);
                        Ok(Self::aborted_response(&call, secs))
                    },
                    res = async {
                        // 💡 핵심 트릭: `Either<read_guard, write_guard>` —
                        // 두 가드 타입을 한 변수에 담고 자연 drop 으로 락 해제.
                        let _guard = if supports_parallel {
                            Either::Left(lock.read().await)
                        } else {
                            Either::Right(lock.write().await)
                        };

                        router
                            .dispatch_tool_call_with_code_mode_result(
                                session,
                                turn,
                                tracker,
                                call.clone(),
                                source,
                            )
                            .instrument(dispatch_span.clone())
                            .await
                    } => res,
                }
            }));

        async move {
            handle.await.map_err(|err| {
                FunctionCallError::Fatal(format!("tool task failed to receive: {err:?}"))
            })?
        }
        .in_current_span()
    }
}

impl ToolCallRuntime {
    /// 일반(비-Fatal) 에러를 모델용 응답 메시지로 변환한다.
    /// ### How
    /// 페이로드 타입(ToolSearch / Custom / 그 외)별로 응답 enum 의 다른 variant 를
    /// 골라야 모델 SDK 가 제대로 해석한다. ToolSearch 만은 빈 결과로 "completed"
    /// 처리하는 게 관례 — 검색이 0건이라는 의미.
    /// ### Why
    /// 모델 입장에선 "툴이 실패했다" 도 일종의 응답이다. 그냥 throw 하면 모델은
    /// 자기가 무엇을 호출했는지조차 모르고 멈춰버린다 → 친절한 실패 메시지로
    /// 돌려줘야 다음 턴에서 회복 가능.
    fn failure_response(call: ToolCall, err: FunctionCallError) -> ResponseInputItem {
        let message = err.to_string();
        match call.payload {
            ToolPayload::ToolSearch { .. } => ResponseInputItem::ToolSearchOutput {
                call_id: call.call_id,
                status: "completed".to_string(),
                execution: "client".to_string(),
                tools: Vec::new(),
            },
            ToolPayload::Custom { .. } => ResponseInputItem::CustomToolCallOutput {
                call_id: call.call_id,
                name: None,
                output: codex_protocol::models::FunctionCallOutputPayload {
                    body: codex_protocol::models::FunctionCallOutputBody::Text(message),
                    success: Some(false),
                },
            },
            _ => ResponseInputItem::FunctionCallOutput {
                call_id: call.call_id,
                output: codex_protocol::models::FunctionCallOutputPayload {
                    body: codex_protocol::models::FunctionCallOutputBody::Text(message),
                    success: Some(false),
                },
            },
        }
    }

    /// 취소된 툴 호출에 대한 표준 응답 객체를 빚는다 — call 의 메타를 그대로
    /// 복사하고 결과만 `AbortedToolOutput` 으로 채운다.
    fn aborted_response(call: &ToolCall, secs: f32) -> AnyToolResult {
        AnyToolResult {
            call_id: call.call_id.clone(),
            payload: call.payload.clone(),
            result: Box::new(AbortedToolOutput {
                message: Self::abort_message(call, secs),
            }),
        }
    }

    /// 취소 메시지 문자열을 만든다.
    /// ### Why
    /// 셸 계열(shell/container.exec/local_shell/...) 툴은 모델이 "Wall time: X"
    /// 헤더를 기대한다 — 정상 종료 응답과 형식을 맞춰주지 않으면 모델이 응답을
    /// 잘못 해석할 수 있다. 그 외 일반 툴은 인간 친화적인 한 줄로 충분.
    /// ⚠️ namespace 가 `None` 인 경우만 셸 계열로 인정 — 같은 이름의 MCP 툴은
    /// 셸 출력 형식을 기대하지 않는다.
    fn abort_message(call: &ToolCall, secs: f32) -> String {
        if call.tool_name.namespace.is_none()
            && matches!(
                call.tool_name.name.as_str(),
                "shell" | "container.exec" | "local_shell" | "shell_command" | "unified_exec"
            )
        {
            format!("Wall time: {secs:.1} seconds\naborted by user")
        } else {
            format!("aborted by user after {secs:.1}s")
        }
    }
}

// ---------------------------------------------------------------------------
// ## 🎓 What to Steal for Your Own Projects
//
// 1. **`RwLock<()>` 게이트 패턴** — 보호할 데이터가 없어도 "병렬 OK / 단독 점유"
//    의 두 모드를 RwLock 의 read/write 시멘틱에 그대로 매핑. 직관적이고 빠르다.
// 2. **`Either<read_guard, write_guard>`** — 한 변수에 두 가드 타입을 담아 자동
//    drop. trait object 알로케이션 없이 zero-cost 다형성.
// 3. **`tokio::select!` 에서 cancel 가지를 위쪽에** — 락 대기 중이라도 먼저
//    빠져나올 수 있어 사용자 인터럽트 latency 가 락 경합 시간에 묶이지 않는다.
// 4. **에러 라우팅 진입점 1개** — Fatal vs 일반 에러를 한 곳에서 분기해서 위로 올릴
//    것/응답으로 변환할 것을 정렬. 호출자 코드가 단순해진다.
// 5. **모델용 실패 응답 형식 맞추기** — 외부 API/모델이 정상 응답과 똑같은 형식의
//    실패 응답을 받아야 다음 step 에서 회복 가능. throw 만 하면 통신 끊긴다.
// ---------------------------------------------------------------------------
