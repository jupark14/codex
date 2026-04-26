//! ## 📐 Architecture Overview
//!
//! 이 파일은 **모델이 흘려보낸 출력 한 조각(`ResponseItem`)을 해석해서, 그게
//! 툴 호출이면 실행 future 를 만들고 / 메시지·추론·이미지생성이면 화면에 보여줄
//! `TurnItem` 으로 변환·기록하는** 변환기 모음이다.
//!
//! 비유하면 라이브 방송의 **오디오 믹서**다 — 들어오는 신호가 음성/효과음/광고
//! 큐인지를 가려 각각 다른 채널로 라우팅한다.
//!
//! ```text
//!   model stream
//!         │  ResponseItem ───────────────────────────┐
//!         ▼                                          │
//!   handle_output_item_done ★ 이 파일                │
//!     ├── ToolRouter::build_tool_call ──┐            │
//!     │      Some(call) → tool_runtime.handle_tool_call → tool_future
//!     │      None       → handle_non_tool_response_item → TurnItem (UI 표시)
//!     │      Err(...)   → 에러 응답 입력 큐로 push                │
//!     └── record_completed_response_item (모든 경로 공통: 히스토리/메모리 기록)
//! ```
//!
//! 데이터 플로우상 위치:
//! - 호출자: 모델 응답 스트림 루프 (codex.rs 내부)
//! - 의존: [[codex-rs/core/src/tools/parallel.rs::ToolCallRuntime]] 로 툴 실행 위임,
//!   [[codex-rs/core/src/tools/router.rs::ToolRouter::build_tool_call]] 로 분류.
//! - 결과: `OutputItemResult` — 호출자는 needs_follow_up / tool_future /
//!   last_agent_message 를 보고 다음 동작을 결정.

use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_protocol::config_types::ModeKind;
use codex_protocol::items::TurnItem;
use codex_utils_stream_parser::strip_citations;
use tokio_util::sync::CancellationToken;

use crate::codex::Session;
use crate::codex::TurnContext;
use crate::function_tool::FunctionCallError;
use crate::memories::citations::get_thread_id_from_citations;
use crate::memories::citations::parse_memory_citation;
use crate::parse_turn_item;
use crate::tools::parallel::ToolCallRuntime;
use crate::tools::router::ToolRouter;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::DeveloperInstructions;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_rollout::state_db;
use codex_utils_stream_parser::strip_proposed_plan_blocks;
use futures::Future;
use tracing::debug;
use tracing::instrument;

const GENERATED_IMAGE_ARTIFACTS_DIR: &str = "generated_images";

/// 모델이 만든 이미지를 저장할 파일 경로를 만든다.
/// ### How
/// `codex_home/generated_images/<session_id>/<call_id>.png` 형태. session/call 이름이
/// 그대로 파일시스템에 들어가면 위험하므로(`/`, `..`, 콜론 등) 알파벳/숫자/`-`/`_`
/// 만 남기고 나머지는 `_` 로 치환. 빈 문자열이면 `generated_image` 로 폴백.
/// ### Why
/// 💡 **Path Traversal 방지** — sanitize 없이 사용자/모델 제공 ID 를 경로에 붙이면
/// `../../etc/passwd` 같은 공격 또는 OS-별 invalid 문자(예: 윈도우의 `<`, `>`, `?`)
/// 로 쓰기 실패가 발생할 수 있다. 화이트리스트 방식이 가장 안전.
pub(crate) fn image_generation_artifact_path(
    codex_home: &Path,
    session_id: &str,
    call_id: &str,
) -> PathBuf {
    let sanitize = |value: &str| {
        let mut sanitized: String = value
            .chars()
            .map(|ch| {
                // 화이트리스트: ASCII 영숫자 + `-` + `_` 만 통과.
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        if sanitized.is_empty() {
            sanitized = "generated_image".to_string();
        }
        sanitized
    };

    codex_home
        .join(GENERATED_IMAGE_ARTIFACTS_DIR)
        .join(sanitize(session_id))
        .join(format!("{}.png", sanitize(call_id)))
}

/// 모델 출력에서 사용자에게 보이지 말아야 할 마크업을 제거한 *디스플레이용* 텍스트.
/// ### Why
/// 모델은 출력에 `<citation>...</citation>` 같은 메타 태그를 섞어 보낸다 — 그건
/// 시스템용이지 화면용이 아니다. plan_mode 면 추가로 `<proposed_plan>` 블록도 제거.
fn strip_hidden_assistant_markup(text: &str, plan_mode: bool) -> String {
    let (without_citations, _) = strip_citations(text);
    if plan_mode {
        strip_proposed_plan_blocks(&without_citations)
    } else {
        without_citations
    }
}

/// `strip_hidden_assistant_markup` 의 사촌 — 마크업을 떼면서 *떼낸 citation 도*
/// 함께 돌려준다. 화면에는 안 보이지만 메모리 인용 추적에는 써야 하기 때문.
fn strip_hidden_assistant_markup_and_parse_memory_citation(
    text: &str,
    plan_mode: bool,
) -> (
    String,
    Option<codex_protocol::memory_citation::MemoryCitation>,
) {
    let (without_citations, citations) = strip_citations(text);
    let visible_text = if plan_mode {
        strip_proposed_plan_blocks(&without_citations)
    } else {
        without_citations
    };
    (visible_text, parse_memory_citation(citations))
}

/// `ResponseItem::Message`(role=assistant) 의 OutputText 들을 이어붙여 *raw* 문자열을
/// 돌려준다. 마크업 제거 전 단계라서 citation 같은 메타가 그대로 남아있다.
/// 호출자가 용도에 맞춰(검사 / 디스플레이) 다음 단계 가공을 한다.
pub(crate) fn raw_assistant_output_text_from_item(item: &ResponseItem) -> Option<String> {
    if let ResponseItem::Message { role, content, .. } = item
        && role == "assistant"
    {
        let combined = content
            .iter()
            .filter_map(|ci| match ci {
                codex_protocol::models::ContentItem::OutputText { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        return Some(combined);
    }
    None
}

/// Base64 이미지 페이로드를 디코드해서 디스크에 PNG 로 저장하고 그 경로를 돌려준다.
/// ### How
/// 1. 입력 문자열을 trim → base64 디코드. 잘못된 base64 면 `InvalidRequest` 로 변환.
/// 2. `image_generation_artifact_path` 로 안전한 저장 경로 계산.
/// 3. 부모 디렉토리가 없으면 `create_dir_all` 로 생성 (멱등).
/// 4. `tokio::fs::write` 로 비동기 쓰기.
/// ### Why
/// `Vec<u8>` 인 이미지를 conversation history 에 그대로 넣지 않고 외부 파일로 빼는
/// 이유는 — 모델 컨텍스트와 rollout 의 크기/속도를 보호하기 위함. 본문에는 경로만
/// 남기고 실제 바이너리는 별도 파일.
async fn save_image_generation_result(
    codex_home: &std::path::Path,
    session_id: &str,
    call_id: &str,
    result: &str,
) -> Result<PathBuf> {
    let bytes = BASE64_STANDARD
        .decode(result.trim().as_bytes())
        .map_err(|err| {
            CodexErr::InvalidRequest(format!("invalid image generation payload: {err}"))
        })?;
    let path = image_generation_artifact_path(codex_home, session_id, call_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, bytes).await?;
    Ok(path)
}

/// Persist a completed model response item and record any cited memory usage.
///
/// ### Layer 1 — What
/// 모델이 막 끝낸 응답 한 조각을 *대화 히스토리에 기록*하고, 그 부수효과(메일박스
/// 게이팅, 메모리 모드 pollution, stage1 토큰 usage 누적)를 처리한다.
///
/// ### Layer 2 — How
/// 1. `record_conversation_items` — 가장 먼저 히스토리/롤아웃에 영구 저장.
///    💡 이게 먼저여야 다음 단계에서 turn 이 인터럽트되어도 발화는 남는다.
/// 2. 메일박스 신호등 체크 — 최종 답변이 화면에 나간 시점이면 다음 자식 메일을
///    `NextTurn` 에 줄 세운다 (`completed_item_defers_mailbox_delivery_to_next_turn`).
/// 3. 웹 검색 호출이라면 thread 의 메모리 모드를 polluted 로 마크.
/// 4. citation 으로 인용된 메모리 thread_id 들에 stage1 출력 usage 를 기록.
///
/// ### Layer 3 — Macro Role
/// `handle_output_item_done` 의 모든 분기(tool call / 일반 / 에러)에서 공통 호출되는
/// "기록 단계". 호출자에게 "어디서 호출하든 똑같이 기록한다" 보장을 준다.
///
/// ### Layer 4 — Why
/// 기록·메일박스·메모리 추적을 한 함수에 모은 이유는 — 호출 지점마다 따로 호출하면
/// 일부만 누락될 위험이 크다(특히 새 응답 타입을 추가했을 때). 한 함수에 모아두면
/// 잊어버릴 일이 없다.
///
/// ### Layer 5 — Why Not
/// - **호출 지점에서 각각 직접 처리?** → 분기 N개에 4단계씩 펼쳐 두면 차이가 생긴다.
/// - **`Drop` 으로 자동화?** → side effect 가 async 라 Drop 에서 실행 불가.
/// - **이벤트 버스 패턴?** → 단일 콜체인 안에서의 순서 보장이 약해진다.
///
/// ### Layer 6 — Lesson
/// 📌 *"동일 라이프사이클의 부수효과들은 한 함수에 묶어 진입점을 강제하라"* —
/// 호출자에게 4단계를 외우게 하지 말고 1줄 호출로 끝내게 만들라.
/// 교과서: *Tell, Don't Ask*, *Single Source of Truth*.
pub(crate) async fn record_completed_response_item(
    sess: &Session,
    turn_context: &TurnContext,
    item: &ResponseItem,
) {
    sess.record_conversation_items(turn_context, std::slice::from_ref(item))
        .await;
    if completed_item_defers_mailbox_delivery_to_next_turn(
        item,
        turn_context.collaboration_mode.mode == ModeKind::Plan,
    ) {
        sess.defer_mailbox_delivery_to_next_turn(&turn_context.sub_id)
            .await;
    }
    maybe_mark_thread_memory_mode_polluted_from_web_search(sess, turn_context, item).await;
    record_stage1_output_usage_for_completed_item(turn_context, item).await;
}

/// 웹 검색이 일어난 thread 의 메모리 모드를 polluted 로 표시한다 — 단, 설정
/// (`no_memories_if_mcp_or_web_search`) 이 켜져 있고 응답이 실제 `WebSearchCall` 일 때만.
/// ### Why
/// 웹 검색 결과가 컨텍스트에 들어가면 모델 메모리(이전 대화 기반 추론)와 웹 정보가
/// 섞여 hallucination 가능성이 커진다 → 그 thread 는 "이미 외부 정보로 오염됨"
/// 마크를 달아 메모리 기반 추론을 보수적으로 하게 만든다.
async fn maybe_mark_thread_memory_mode_polluted_from_web_search(
    sess: &Session,
    turn_context: &TurnContext,
    item: &ResponseItem,
) {
    if !turn_context
        .config
        .memories
        .no_memories_if_mcp_or_web_search
        || !matches!(item, ResponseItem::WebSearchCall { .. })
    {
        return;
    }
    state_db::mark_thread_memory_mode_polluted(
        sess.services.state_db.as_deref(),
        sess.conversation_id,
        "record_completed_response_item",
    )
    .await;
}

/// 응답에 인용된 메모리 thread_id 들에 "stage1 출력 usage" 카운트를 누적한다.
/// ### How
/// 1. assistant 메시지가 아니면(citation 자체가 없으면) early-return.
/// 2. citation 들에서 thread_id 만 추출.
/// 3. state DB 가 있으면 비동기로 카운트++ — 실패해도 무시(`let _ =`) — 텔레메트리는
///    실패해도 turn 진행은 계속해야 한다.
/// ### Why
/// 어떤 메모리가 모델 응답에 *실제로 사용*됐는지를 추적해야 메모리 우선순위/수명
/// 정책에 피드백할 수 있다. "쓰인 메모리는 신선도 ↑, 안 쓰이는 메모리는 LRU 로 정리".
async fn record_stage1_output_usage_for_completed_item(
    turn_context: &TurnContext,
    item: &ResponseItem,
) {
    let Some(raw_text) = raw_assistant_output_text_from_item(item) else {
        return;
    };

    let (_, citations) = strip_citations(&raw_text);
    let thread_ids = get_thread_id_from_citations(citations);
    if thread_ids.is_empty() {
        return;
    }

    if let Some(db) = state_db::get_state_db(turn_context.config.as_ref()).await {
        let _ = db.record_stage1_output_usage(&thread_ids).await;
    }
}

/// Handle a completed output item from the model stream, recording it and
/// queuing any tool execution futures. This records items immediately so
/// history and rollout stay in sync even if the turn is later cancelled.
///
/// 백그라운드에서 도는 툴 호출 future 의 표준 타입.
/// 비유하면 "주문서 한 장에 적힌 약속" — 결과(`ResponseInputItem`)를 언젠가 갖다
/// 줄 거라는 약속을 모델 스트림 루프가 들고 다닌다.
pub(crate) type InFlightFuture<'f> =
    Pin<Box<dyn Future<Output = Result<ResponseInputItem>> + Send + 'f>>;

/// `handle_output_item_done` 의 반환 묶음 — 호출자가 다음 동작을 결정하기 위해
/// 필요한 3가지를 묶어둔다.
/// ### Why
/// - `last_agent_message`: 화면에 보일 마지막 에이전트 메시지 (텔레메트리/UI).
/// - `needs_follow_up`: 한 번 더 모델 호출이 필요한가? (툴 호출/에러 후 재호출).
/// - `tool_future`: 백그라운드에서 돌고 있는 툴 future. 호출자가 await 한다.
/// 셋을 한 구조체로 묶지 않으면 호출자가 5-tuple 을 받게 되어 가독성이 깨진다.
#[derive(Default)]
pub(crate) struct OutputItemResult {
    pub last_agent_message: Option<String>,
    pub needs_follow_up: bool,
    pub tool_future: Option<InFlightFuture<'static>>,
}

/// `handle_output_item_done` 가 일하는 데 필요한 컨텍스트 묶음.
/// 함수 시그니처에 인자 4개를 매번 늘어놓지 않으려고 묶어둔 *Parameter Object*.
/// 비유: "장 보러 가기 전에 챙기는 가방" — 지갑/장보기 리스트/장바구니/우산을 한
/// 가방에 넣어 들고 다닌다.
pub(crate) struct HandleOutputCtx {
    pub sess: Arc<Session>,
    pub turn_context: Arc<TurnContext>,
    pub tool_runtime: ToolCallRuntime,
    pub cancellation_token: CancellationToken,
}

/// 모델이 흘려보낸 출력 한 조각(`ResponseItem`)이 *완료*됐을 때 그 조각을 처리한다.
///
/// ### Layer 1 — What
/// 들어온 한 조각을 ① 툴 호출인지 분류하고 ② 분류 결과대로 라우팅하고
/// ③ 결과를 `OutputItemResult` 로 묶어 돌려준다. 한 조각당 정확히 한 번 호출됨.
///
/// ### Layer 2 — How
/// 1. `ToolRouter::build_tool_call(item)` 으로 분류 시도. 결과는 4갈래:
///    - `Ok(Some(call))` — 툴 호출이다 → 메일박스 신호등 `CurrentTurn`,
///      tracing 로그, 히스토리 기록, **백그라운드 future 생성**, `needs_follow_up=true`.
///    - `Ok(None)` — 메시지/추론/이미지/검색 → `handle_non_tool_response_item` 으로
///      TurnItem 으로 변환, started 이벤트 + completed 이벤트 발사, 마지막 메시지 추출.
///    - `Err(MissingLocalShellCallId)` — 모델이 잘못된 LocalShellCall 보냄 → 에러
///      메시지를 입력 큐에 넣고 follow-up 요청.
///    - `Err(RespondToModel)` — 라우터가 직접 답할 수 있음(거부/즉시 응답) → 그
///      메시지를 응답으로 push, follow-up.
///    - `Err(Fatal)` — 턴 자체를 죽인다. `Err(CodexErr::Fatal)` 로 propagate.
///
/// ### Layer 3 — Macro Role
/// 모델 응답 스트림 루프(codex.rs)에서 매 `OutputItemDone` 이벤트마다 호출되는
/// 단일 진입점. 이 함수가 모델 출력을 **시스템 동작(툴 실행 / UI 이벤트 발사 /
/// 히스토리 기록)으로 번역**하는 핵심 변환기.
///
/// ### Layer 4 — Why
/// 4갈래 분기를 한 함수에 모아둔 이유는 — 모든 경로가 `record_completed_response_item`
/// 을 공통 호출해야 하고, 각 경로의 사후 처리(`needs_follow_up` 설정 등)가 미묘하게
/// 다르다. 분리하면 새 분기를 추가할 때 어디를 빼먹을지 추적하기 어려워진다.
///
/// ### Layer 5 — Why Not
/// - **`Result<Outcome, FatalErr>` 같은 enum 반환?** → 호출자가 enum 매칭을 또
///   해야 한다. follow-up flag + future 옵션 + 메시지 옵션의 3-필드 struct 가
///   호출자에게 더 친화적.
/// - **toolCall vs nonTool 두 함수로 분리?** → 라우터의 분류 결과가 4갈래라 호출자가
///   라우팅을 다시 짜야 한다. 라우팅을 여기 한 곳에 가둬야 *호출자는 Just-do-it*.
///
/// ### Layer 6 — Lesson
/// 📌 *"분기 N개 + 공통 사후처리 패턴은 단일 진입 함수가 맡고, 결과는 묶음 struct
/// 로"* — 호출자에게 enum 매칭 부담을 떠넘기지 말고 사용 친화적 형태로 가공.
/// 교과서: *Pipes and Filters*, *Tagged Union return + Boundary Object*.
#[instrument(level = "trace", skip_all)]
pub(crate) async fn handle_output_item_done(
    ctx: &mut HandleOutputCtx,
    item: ResponseItem,
    previously_active_item: Option<TurnItem>,
) -> Result<OutputItemResult> {
    let mut output = OutputItemResult::default();
    let plan_mode = ctx.turn_context.collaboration_mode.mode == ModeKind::Plan;

    match ToolRouter::build_tool_call(ctx.sess.as_ref(), item.clone()).await {
        // The model emitted a tool call; log it, persist the item immediately, and queue the tool execution.
        Ok(Some(call)) => {
            ctx.sess
                .accept_mailbox_delivery_for_current_turn(&ctx.turn_context.sub_id)
                .await;

            let payload_preview = call.payload.log_payload().into_owned();
            tracing::info!(
                thread_id = %ctx.sess.conversation_id,
                "ToolCall: {} {}",
                call.tool_name.display(),
                payload_preview
            );

            record_completed_response_item(ctx.sess.as_ref(), ctx.turn_context.as_ref(), &item)
                .await;

            let cancellation_token = ctx.cancellation_token.child_token();
            let tool_future: InFlightFuture<'static> = Box::pin(
                ctx.tool_runtime
                    .clone()
                    .handle_tool_call(call, cancellation_token),
            );

            output.needs_follow_up = true;
            output.tool_future = Some(tool_future);
        }
        // No tool call: convert messages/reasoning into turn items and mark them as complete.
        Ok(None) => {
            if let Some(turn_item) = handle_non_tool_response_item(
                ctx.sess.as_ref(),
                ctx.turn_context.as_ref(),
                &item,
                plan_mode,
            )
            .await
            {
                if previously_active_item.is_none() {
                    let mut started_item = turn_item.clone();
                    if let TurnItem::ImageGeneration(item) = &mut started_item {
                        item.status = "in_progress".to_string();
                        item.revised_prompt = None;
                        item.result.clear();
                        item.saved_path = None;
                    }
                    ctx.sess
                        .emit_turn_item_started(&ctx.turn_context, &started_item)
                        .await;
                }

                ctx.sess
                    .emit_turn_item_completed(&ctx.turn_context, turn_item)
                    .await;
            }
            record_completed_response_item(ctx.sess.as_ref(), ctx.turn_context.as_ref(), &item)
                .await;
            let last_agent_message = last_assistant_message_from_item(&item, plan_mode);

            output.last_agent_message = last_agent_message;
        }
        // Guardrail: the model issued a LocalShellCall without an id; surface the error back into history.
        Err(FunctionCallError::MissingLocalShellCallId) => {
            let msg = "LocalShellCall without call_id or id";
            ctx.turn_context
                .session_telemetry
                .log_tool_failed("local_shell", msg);
            tracing::error!(msg);

            let response = ResponseInputItem::FunctionCallOutput {
                call_id: String::new(),
                output: FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::Text(msg.to_string()),
                    ..Default::default()
                },
            };
            record_completed_response_item(ctx.sess.as_ref(), ctx.turn_context.as_ref(), &item)
                .await;
            if let Some(response_item) = response_input_to_response_item(&response) {
                ctx.sess
                    .record_conversation_items(
                        &ctx.turn_context,
                        std::slice::from_ref(&response_item),
                    )
                    .await;
            }

            output.needs_follow_up = true;
        }
        // The tool request should be answered directly (or was denied); push that response into the transcript.
        Err(FunctionCallError::RespondToModel(message)) => {
            let response = ResponseInputItem::FunctionCallOutput {
                call_id: String::new(),
                output: FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::Text(message),
                    ..Default::default()
                },
            };
            record_completed_response_item(ctx.sess.as_ref(), ctx.turn_context.as_ref(), &item)
                .await;
            if let Some(response_item) = response_input_to_response_item(&response) {
                ctx.sess
                    .record_conversation_items(
                        &ctx.turn_context,
                        std::slice::from_ref(&response_item),
                    )
                    .await;
            }

            output.needs_follow_up = true;
        }
        // A fatal error occurred; surface it back into history.
        Err(FunctionCallError::Fatal(message)) => {
            return Err(CodexErr::Fatal(message));
        }
    }

    Ok(output)
}

/// 툴 호출이 아닌 모델 출력(메시지/추론/웹검색/이미지생성)을 화면용 `TurnItem` 으로
/// 가공한다.
///
/// ### Layer 1 — What
/// 들어온 `ResponseItem` 의 종류에 따라:
/// - 메시지/추론/웹검색/이미지생성 → `TurnItem` 으로 변환 + 부수 처리(citation strip,
///   이미지 디스크 저장, developer 메시지 추가).
/// - FunctionCallOutput / CustomToolCallOutput / ToolSearchOutput → 모델이 직접
///   보낼 게 아닌데 흘러나왔으므로 무시(debug 로그만 남김).
///
/// ### Layer 2 — How
/// 1. 일반 케이스: `parse_turn_item(item)` 로 변환.
/// 2. `AgentMessage` 면: 텍스트들을 합쳐서 citation strip + memory citation 파싱 →
///    가공된 텍스트로 `content` 교체, 추출한 memory_citation 을 슬롯에 삽입.
/// 3. `ImageGeneration` 이면: base64 → 디스크 저장 시도.
///    - 성공: `saved_path` 채우고 모델에게 *"이미지가 X 에 저장됐다"* developer
///      메시지를 히스토리에 추가 → 다음 턴에서 모델이 그 경로를 알 수 있게.
///    - 실패: 경고 로그만 남기고 진행 (이미지 손실은 fatal 아님).
///
/// ### Layer 3 — Macro Role
/// `handle_output_item_done` 의 "툴 호출 아님" 갈래에서만 호출된다. 반환된 `TurnItem`
/// 은 호출자가 `emit_turn_item_started` / `emit_turn_item_completed` 로 UI 에 흘려
/// 보낸다.
///
/// ### Layer 4 — Why
/// 메시지 가공(citation strip, memory citation 추출)을 여기 한 곳에 모은 이유는 —
/// `record_completed_response_item` 은 *원본*을 저장해야 하고(나중에 재처리 가능),
/// UI 이벤트는 *가공본*을 받아야 한다. 두 길의 갈림길이 바로 이 함수.
///
/// ### Layer 5 — Why Not
/// - **이미지 저장 실패 시 에러 propagate?** → 이미지가 안 저장돼도 모델 응답은
///   이미 받았다. 턴을 죽이는 건 사용자에게 손해. 경고 로그가 안전한 절충.
/// - **`AgentMessage` / `ImageGeneration` 별도 함수로 분리?** → 같은 라이프사이클
///   (parse → 가공 → return)이라 한 함수 안에 둬도 가독성에 무리 없음.
///
/// ### Layer 6 — Lesson
/// 📌 *"보낼 응답이 본질적으로 두 종류 (시스템 기록용 raw + 사용자용 가공본)면
/// 각각의 길을 별도 함수로 가르라"* — 한쪽 가공이 다른 쪽에 새지 않게 격리.
pub(crate) async fn handle_non_tool_response_item(
    sess: &Session,
    turn_context: &TurnContext,
    item: &ResponseItem,
    plan_mode: bool,
) -> Option<TurnItem> {
    debug!(?item, "Output item");

    match item {
        ResponseItem::Message { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. } => {
            let mut turn_item = parse_turn_item(item)?;
            if let TurnItem::AgentMessage(agent_message) = &mut turn_item {
                let combined = agent_message
                    .content
                    .iter()
                    .map(|entry| match entry {
                        codex_protocol::items::AgentMessageContent::Text { text } => text.as_str(),
                    })
                    .collect::<String>();
                let (stripped, memory_citation) =
                    strip_hidden_assistant_markup_and_parse_memory_citation(&combined, plan_mode);
                agent_message.content =
                    vec![codex_protocol::items::AgentMessageContent::Text { text: stripped }];
                agent_message.memory_citation = memory_citation;
            }
            if let TurnItem::ImageGeneration(image_item) = &mut turn_item {
                let session_id = sess.conversation_id.to_string();
                match save_image_generation_result(
                    turn_context.config.codex_home.as_path(),
                    &session_id,
                    &image_item.id,
                    &image_item.result,
                )
                .await
                {
                    Ok(path) => {
                        image_item.saved_path = Some(path.to_string_lossy().into_owned());
                        let image_output_path = image_generation_artifact_path(
                            turn_context.config.codex_home.as_path(),
                            &session_id,
                            "<image_id>",
                        );
                        let image_output_dir = image_output_path
                            .parent()
                            .unwrap_or(turn_context.config.codex_home.as_path());
                        let message: ResponseItem = DeveloperInstructions::new(format!(
                            "Generated images are saved to {} as {} by default.\nIf you need to use a generated image at another path, copy it and leave the original in place unless the user explicitly asks you to delete it.",
                            image_output_dir.display(),
                            image_output_path.display(),
                        ))
                        .into();
                        sess.record_conversation_items(turn_context, &[message])
                            .await;
                    }
                    Err(err) => {
                        let output_path = image_generation_artifact_path(
                            turn_context.config.codex_home.as_path(),
                            &session_id,
                            &image_item.id,
                        );
                        let output_dir = output_path
                            .parent()
                            .unwrap_or(turn_context.config.codex_home.as_path());
                        tracing::warn!(
                            call_id = %image_item.id,
                            output_dir = %output_dir.display(),
                            "failed to save generated image: {err}"
                        );
                    }
                }
            }
            Some(turn_item)
        }
        ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. } => {
            debug!("unexpected tool output from stream");
            None
        }
        _ => None,
    }
}

/// 응답 조각에서 *사용자에게 보일 마지막 에이전트 메시지*를 뽑아낸다.
/// ### How
/// raw 텍스트가 비어있거나, 마크업 strip 후 trim 결과가 빈 문자열이면 `None`.
/// 의미 있는 화면 표시 텍스트가 있을 때만 `Some(stripped)`.
/// ### Why
/// "마지막 에이전트 메시지" 는 텔레메트리/요약 파이프라인이 쓰는 의미 있는 값이라
/// citation 같은 메타로만 채워진 빈 메시지는 제외해야 false-positive 가 안 생긴다.
pub(crate) fn last_assistant_message_from_item(
    item: &ResponseItem,
    plan_mode: bool,
) -> Option<String> {
    if let Some(combined) = raw_assistant_output_text_from_item(item) {
        if combined.is_empty() {
            return None;
        }
        let stripped = strip_hidden_assistant_markup(&combined, plan_mode);
        if stripped.trim().is_empty() {
            return None;
        }
        return Some(stripped);
    }
    None
}

/// "이 완료된 조각 이후로는 자식 메일을 *다음 턴*에 미뤄야 하나?" 의 정책 함수.
/// ### How
/// - assistant 의 최종 답변(`Commentary` 가 아닌 메시지) 이고 가공 후 표시 텍스트가
///   있으면 → true (이미 답변 보여줬으니 메일 미뤄야 함).
/// - 이미지 생성 완료 → true (이미지도 사용자가 보는 산출물).
/// - 그 외 (commentary, 추론, 툴 출력 등) → false (메일 흡수해도 자연스러움).
/// ### Why
/// 메일박스 신호등(`MailboxDeliveryPhase`) 의 `NextTurn` 전이 트리거를 판단하는 정책.
/// 이 정책을 한 함수로 모아두면, 새 응답 타입을 추가했을 때 한 곳만 보면 된다.
fn completed_item_defers_mailbox_delivery_to_next_turn(
    item: &ResponseItem,
    plan_mode: bool,
) -> bool {
    match item {
        ResponseItem::Message { role, phase, .. } => {
            if role != "assistant" || matches!(phase, Some(MessagePhase::Commentary)) {
                return false;
            }
            // Treat `None` like final-answer text so untagged providers default
            // to the safer "defer mailbox mail" behavior.
            last_assistant_message_from_item(item, plan_mode).is_some()
        }
        ResponseItem::ImageGenerationCall { .. } => true,
        _ => false,
    }
}

/// 입력용 enum(`ResponseInputItem`) 을 출력용 enum(`ResponseItem`) 으로 호환 변환.
/// ### Why
/// 두 타입은 거의 같은 모양이지만 protocol 분리로 별도 enum 이다. 변환 가능한 것만
/// `Some` 으로 돌려주고, 그 외(예: 사용자 텍스트 같은 input-only variant)는 `None`.
/// 호출자(주로 에러 응답을 히스토리에 다시 기록할 때)가 이 함수로 다리를 놓는다.
/// ⚠️ McpToolCallOutput 은 변환 시 payload 모양을 `as_function_call_output_payload()`
/// 로 단순화 — protocol level 에서 두 타입의 모양이 다른 부분을 흡수.
pub(crate) fn response_input_to_response_item(input: &ResponseInputItem) -> Option<ResponseItem> {
    match input {
        ResponseInputItem::FunctionCallOutput { call_id, output } => {
            Some(ResponseItem::FunctionCallOutput {
                call_id: call_id.clone(),
                output: output.clone(),
            })
        }
        ResponseInputItem::CustomToolCallOutput {
            call_id,
            name,
            output,
        } => Some(ResponseItem::CustomToolCallOutput {
            call_id: call_id.clone(),
            name: name.clone(),
            output: output.clone(),
        }),
        ResponseInputItem::McpToolCallOutput { call_id, output } => {
            let output = output.as_function_call_output_payload();
            Some(ResponseItem::FunctionCallOutput {
                call_id: call_id.clone(),
                output,
            })
        }
        ResponseInputItem::ToolSearchOutput {
            call_id,
            status,
            execution,
            tools,
        } => Some(ResponseItem::ToolSearchOutput {
            call_id: Some(call_id.clone()),
            status: status.clone(),
            execution: execution.clone(),
            tools: tools.clone(),
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ## 🎓 What to Steal for Your Own Projects
//
// 1. **단일 진입점에서 분기 라우팅** — 모델/외부 응답을 처리할 땐 분류 → 라우팅을
//    한 함수에 가두면, 호출자가 분류 결과 enum 매칭을 안 해도 됨. 새 분기 추가도
//    한 곳에서.
// 2. **공통 사후처리 함수** — `record_completed_response_item` 처럼 모든 분기에서
//    호출되어야 할 부수효과(저장/메트릭/플래그)를 한 함수로 묶으면 누락 위험 ↓.
// 3. **raw 와 가공본의 분리** — 시스템 기록(history) 은 원본을, UI 이벤트는
//    가공본(citation strip 후) 을 받게 분리. 한쪽 가공이 다른 쪽에 새지 않는다.
// 4. **사용자 입력 ID sanitize** — 외부 ID 를 파일 경로에 쓰기 전에 화이트리스트
//    문자만 통과시켜라. Path Traversal/OS-별 invalid 문자 동시 해결.
// 5. **`Parameter Object` 패턴** — 4개 이상의 인자를 매번 함께 들고 다닌다면
//    `HandleOutputCtx` 처럼 묶음 struct 로. 시그니처가 안정되고 새 컨텍스트 추가가
//    O(1) (struct 필드 하나).
// 6. **`telemetry / 비주류 부수효과는 best-effort`** — `let _ = ...` 패턴으로
//    실패를 흡수해 본업(턴 처리) 이 죽지 않게. 단, 본업 실패는 절대 흡수 금지.
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "stream_events_utils_tests.rs"]
mod tests;
