//! 📄 이 모듈이 하는 일:
//!   exec 세션에서 받은 알림을 기계가 읽기 쉬운 JSONL 이벤트 줄로 바꿔 내보낸다.
//!   비유로 말하면 경기 기록원이 장면마다 표준 양식 카드로 적어서 방송국에 넘기는 역할이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/exec/src/lib.rs`
//!   - `codex-rs/exec/src/exec_events.rs`
//!
//! 🧩 핵심 개념:
//!   - JSONL = 한 줄에 사건 하나씩 적는 사건 일지 형식
//!   - raw item id ↔ exec item id = 원본 앱 서버 카드 번호를 exec 전용 번호표로 바꿔 붙이는 표

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use codex_app_server_protocol::CollabAgentTool;
use codex_app_server_protocol::CollabAgentToolCallStatus;
use codex_app_server_protocol::CommandExecutionStatus;
use codex_app_server_protocol::McpToolCallStatus;
use codex_app_server_protocol::PatchApplyStatus;
use codex_app_server_protocol::PatchChangeKind;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadTokenUsage;
use codex_app_server_protocol::TurnStatus;
use codex_core::config::Config;
use codex_protocol::models::WebSearchAction;
use codex_protocol::protocol::SessionConfiguredEvent;
use serde_json::json;

pub use crate::event_processor::CodexStatus;
use crate::event_processor::EventProcessor;
use crate::event_processor::handle_last_message;
use crate::exec_events::AgentMessageItem;
use crate::exec_events::CollabAgentState;
use crate::exec_events::CollabAgentStatus;
use crate::exec_events::CollabTool;
use crate::exec_events::CollabToolCallItem;
use crate::exec_events::CollabToolCallStatus;
use crate::exec_events::CommandExecutionItem;
use crate::exec_events::CommandExecutionStatus as ExecCommandExecutionStatus;
use crate::exec_events::ErrorItem;
use crate::exec_events::FileChangeItem;
use crate::exec_events::FileUpdateChange;
use crate::exec_events::ItemCompletedEvent;
use crate::exec_events::ItemStartedEvent;
use crate::exec_events::ItemUpdatedEvent;
use crate::exec_events::McpToolCallItem;
use crate::exec_events::McpToolCallItemError;
use crate::exec_events::McpToolCallItemResult;
use crate::exec_events::McpToolCallStatus as ExecMcpToolCallStatus;
use crate::exec_events::PatchApplyStatus as ExecPatchApplyStatus;
use crate::exec_events::PatchChangeKind as ExecPatchChangeKind;
use crate::exec_events::ReasoningItem;
use crate::exec_events::ThreadErrorEvent;
use crate::exec_events::ThreadEvent;
use crate::exec_events::ThreadItem as ExecThreadItem;
use crate::exec_events::ThreadItemDetails;
use crate::exec_events::ThreadStartedEvent;
use crate::exec_events::TodoItem;
use crate::exec_events::TodoListItem;
use crate::exec_events::TurnCompletedEvent;
use crate::exec_events::TurnFailedEvent;
use crate::exec_events::TurnStartedEvent;
use crate::exec_events::Usage;
use crate::exec_events::WebSearchItem;

/// 🍳 이 구조체는 서버 알림을 JSONL 이벤트로 갈아 끼우는 기록원이다.
///   `ServerNotification` → `ThreadEvent` 줄들 + 종료 상태
pub struct EventProcessorWithJsonOutput {
    last_message_path: Option<PathBuf>,
    next_item_id: AtomicU64,
    raw_to_exec_item_id: HashMap<String, String>,
    running_todo_list: Option<RunningTodoList>,
    last_total_token_usage: Option<ThreadTokenUsage>,
    last_critical_error: Option<ThreadErrorEvent>,
    final_message: Option<String>,
    emit_final_message_on_shutdown: bool,
}

/// 🍳 이 구조체는 아직 끝나지 않은 todo 목록 진행판을 잠깐 들고 있는 메모장이다.
///   진행 중인 todo item id + 단계 목록 → 다음 알림까지 보관
#[derive(Debug, Clone)]
struct RunningTodoList {
    item_id: String,
    items: Vec<TodoItem>,
}

/// 🍳 이 구조체는 한 알림을 처리하고 모은 결과 꾸러미다.
///   생성된 이벤트들 + 계속 실행할지 여부
#[derive(Debug, PartialEq)]
pub struct CollectedThreadEvents {
    pub events: Vec<ThreadEvent>,
    pub status: CodexStatus,
}

impl EventProcessorWithJsonOutput {
    /// 🍳 이 함수는 빈 사건 일지와 번호표 기계를 준비해 JSON 출력기를 만든다.
    ///   마지막 메시지 저장 경로 → 초기화된 processor
    pub fn new(last_message_path: Option<PathBuf>) -> Self {
        Self {
            last_message_path,
            next_item_id: AtomicU64::new(0),
            raw_to_exec_item_id: HashMap::new(),
            running_todo_list: None,
            last_total_token_usage: None,
            last_critical_error: None,
            final_message: None,
            emit_final_message_on_shutdown: false,
        }
    }

    /// 🍳 이 함수는 지금까지 모아 둔 마지막 agent 멘트를 꺼내 보는 창이다.
    ///   내부 `final_message` → 문자열 참조
    pub fn final_message(&self) -> Option<&str> {
        self.final_message.as_deref()
    }

    /// 🍳 이 함수는 새 사건 카드에 붙일 연속 번호표를 만든다.
    ///   내부 카운터 → `item_N` 문자열
    fn next_item_id(&self) -> String {
        format!("item_{}", self.next_item_id.fetch_add(1, Ordering::SeqCst))
    }

    #[allow(clippy::print_stdout)]
    /// 🍳 이 함수는 `ThreadEvent` 한 장을 JSON 문자열 한 줄로 바꿔 stdout에 뿌린다.
    ///   이벤트 객체 → JSONL 1줄
    fn emit(&self, event: ThreadEvent) {
        println!(
            "{}",
            serde_json::to_string(&event).unwrap_or_else(|err| {
                // 🛟 직렬화가 실패해도 stdout 형식이 완전히 깨지지 않게
                //    최소한의 에러 이벤트 한 줄로 대신 보낸다.
                json!({
                    "type": "error",
                    "message": format!("failed to serialize exec json event: {err}"),
                })
                .to_string()
            })
        );
    }

    /// 🍳 이 함수는 최신 누적 토큰 정보에서 turn 요약표를 꺼낸다.
    ///   마지막 total token usage → exec event용 `Usage`
    fn usage_from_last_total(&self) -> Usage {
        let Some(usage) = self.last_total_token_usage.as_ref() else {
            return Usage::default();
        };
        Usage {
            input_tokens: usage.total.input_tokens,
            cached_input_tokens: usage.total.cached_input_tokens,
            output_tokens: usage.total.output_tokens,
        }
    }

    /// 🍳 이 함수는 계획 단계 목록을 todo 체크리스트로 바꾼다.
    ///   turn plan step들 → `TodoItem` 목록
    pub fn map_todo_items(plan: &[codex_app_server_protocol::TurnPlanStep]) -> Vec<TodoItem> {
        plan.iter()
            .map(|step| TodoItem {
                text: step.step.clone(),
                completed: matches!(
                    step.status,
                    codex_app_server_protocol::TurnPlanStepStatus::Completed
                ),
            })
            .collect()
    }

    /// 🍳 이 함수는 원본 `ThreadItem` 하나를 exec 전용 카드 모양으로 변환한다.
    ///   앱 서버 item + id 생성기 → exec item 또는 생략(None)
    fn map_item_with_id(
        item: ThreadItem,
        make_id: impl FnOnce() -> String,
    ) -> Option<ExecThreadItem> {
        match item {
            ThreadItem::AgentMessage { text, .. } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::AgentMessage(AgentMessageItem { text }),
            }),
            ThreadItem::Reasoning { summary, .. } => {
                let text = summary.join("\n");
                if text.trim().is_empty() {
                    return None;
                }
                Some(ExecThreadItem {
                    id: make_id(),
                    details: ThreadItemDetails::Reasoning(ReasoningItem { text }),
                })
            }
            ThreadItem::CommandExecution {
                command,
                aggregated_output,
                exit_code,
                status,
                ..
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::CommandExecution(CommandExecutionItem {
                    command,
                    aggregated_output: aggregated_output.unwrap_or_default(),
                    exit_code,
                    status: match status {
                        CommandExecutionStatus::InProgress => ExecCommandExecutionStatus::InProgress,
                        CommandExecutionStatus::Completed => ExecCommandExecutionStatus::Completed,
                        CommandExecutionStatus::Failed => ExecCommandExecutionStatus::Failed,
                        CommandExecutionStatus::Declined => ExecCommandExecutionStatus::Declined,
                    },
                }),
            }),
            ThreadItem::FileChange {
                changes, status, ..
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::FileChange(FileChangeItem {
                    changes: changes
                        .into_iter()
                        .map(|change| FileUpdateChange {
                            path: change.path,
                            kind: match change.kind {
                                PatchChangeKind::Add => ExecPatchChangeKind::Add,
                                PatchChangeKind::Delete => ExecPatchChangeKind::Delete,
                                PatchChangeKind::Update { .. } => ExecPatchChangeKind::Update,
                            },
                        })
                        .collect(),
                    status: match status {
                        PatchApplyStatus::InProgress => ExecPatchApplyStatus::InProgress,
                        PatchApplyStatus::Completed => ExecPatchApplyStatus::Completed,
                        PatchApplyStatus::Failed | PatchApplyStatus::Declined => {
                            ExecPatchApplyStatus::Failed
                        }
                    },
                }),
            }),
            ThreadItem::McpToolCall {
                server,
                tool,
                status,
                arguments,
                result,
                error,
                ..
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::McpToolCall(McpToolCallItem {
                    server,
                    tool,
                    status: match status {
                        McpToolCallStatus::InProgress => ExecMcpToolCallStatus::InProgress,
                        McpToolCallStatus::Completed => ExecMcpToolCallStatus::Completed,
                        McpToolCallStatus::Failed => ExecMcpToolCallStatus::Failed,
                    },
                    arguments,
                    result: result.map(|result| McpToolCallItemResult {
                        content: result.content,
                        structured_content: result.structured_content,
                    }),
                    error: error.map(|error| McpToolCallItemError {
                        message: error.message,
                    }),
                }),
            }),
            ThreadItem::CollabAgentToolCall {
                tool,
                sender_thread_id,
                receiver_thread_ids,
                prompt,
                agents_states,
                status,
                ..
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::CollabToolCall(CollabToolCallItem {
                    tool: match tool {
                        CollabAgentTool::SpawnAgent => CollabTool::SpawnAgent,
                        CollabAgentTool::SendInput => CollabTool::SendInput,
                        CollabAgentTool::ResumeAgent => CollabTool::Wait,
                        CollabAgentTool::Wait => CollabTool::Wait,
                        CollabAgentTool::CloseAgent => CollabTool::CloseAgent,
                    },
                    sender_thread_id,
                    receiver_thread_ids,
                    prompt,
                    agents_states: agents_states
                        .into_iter()
                        .map(|(thread_id, state)| {
                            (
                                thread_id,
                                CollabAgentState {
                                    status: match state.status {
                                        codex_app_server_protocol::CollabAgentStatus::PendingInit => {
                                            CollabAgentStatus::PendingInit
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Running => {
                                            CollabAgentStatus::Running
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Interrupted => {
                                            CollabAgentStatus::Interrupted
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Completed => {
                                            CollabAgentStatus::Completed
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Errored => {
                                            CollabAgentStatus::Errored
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::Shutdown => {
                                            CollabAgentStatus::Shutdown
                                        }
                                        codex_app_server_protocol::CollabAgentStatus::NotFound => {
                                            CollabAgentStatus::NotFound
                                        }
                                    },
                                    message: state.message,
                                },
                            )
                        })
                        .collect(),
                    status: match status {
                        CollabAgentToolCallStatus::InProgress => CollabToolCallStatus::InProgress,
                        CollabAgentToolCallStatus::Completed => CollabToolCallStatus::Completed,
                        CollabAgentToolCallStatus::Failed => CollabToolCallStatus::Failed,
                    },
                }),
            }),
            ThreadItem::WebSearch {
                id: raw_id,
                query,
                action,
            } => Some(ExecThreadItem {
                id: make_id(),
                details: ThreadItemDetails::WebSearch(WebSearchItem {
                    id: raw_id,
                    query,
                    action: match action {
                        // 🧭 web search action enum은 wire 형태가 조금 달라도
                        //    serde round-trip으로 안전하게 맞춰 보고, 실패하면 `Other`로 접는다.
                        Some(action) => serde_json::from_value(
                            serde_json::to_value(action).unwrap_or_else(|_| json!("other")),
                        )
                        .unwrap_or(WebSearchAction::Other),
                        None => WebSearchAction::Other,
                    },
                }),
            }),
            _ => None,
        }
    }

    /// 🍳 이 함수는 "시작됨" 이벤트용 번호표를 고정해 두는 보관함이다.
    ///   원본 raw id → exec item id
    fn started_item_id(&mut self, raw_id: &str) -> String {
        if let Some(existing) = self.raw_to_exec_item_id.get(raw_id) {
            return existing.clone();
        }
        let exec_id = self.next_item_id();
        self.raw_to_exec_item_id
            .insert(raw_id.to_string(), exec_id.clone());
        exec_id
    }

    /// 🍳 이 함수는 "완료됨" 이벤트에서 예전 번호표를 회수한다.
    ///   원본 raw id → 기존 exec item id 또는 새 fallback id
    fn completed_item_id(&mut self, raw_id: &str) -> String {
        self.raw_to_exec_item_id
            .remove(raw_id)
            .unwrap_or_else(|| self.next_item_id())
    }

    /// 🍳 이 함수는 시작 이벤트에서 보여 줄 만한 item만 골라 카드로 만든다.
    ///   시작된 item → exec 시작 카드 또는 생략
    fn map_started_item(&mut self, item: ThreadItem) -> Option<ExecThreadItem> {
        match item {
            ThreadItem::AgentMessage { .. } | ThreadItem::Reasoning { .. } => None,
            other => {
                let raw_id = other.id().to_string();
                Self::map_item_with_id(other, || self.started_item_id(&raw_id))
            }
        }
    }

    /// 🍳 이 함수는 완료 이벤트용 item을 만들면서 빈 reasoning은 잡음을 줄이려고 건너뛴다.
    ///   완료된 item → exec 완료 카드 또는 생략
    fn map_completed_item_mut(&mut self, item: ThreadItem) -> Option<ExecThreadItem> {
        if let ThreadItem::Reasoning { summary, .. } = &item
            && summary.join("\n").trim().is_empty()
        {
            return None;
        }
        match &item {
            ThreadItem::AgentMessage { .. } | ThreadItem::Reasoning { .. } => {
                Self::map_item_with_id(item, || self.next_item_id())
            }
            other => {
                let raw_id = other.id().to_string();
                Self::map_item_with_id(item, || self.completed_item_id(&raw_id))
            }
        }
    }

    /// 🍳 이 함수는 시작만 찍히고 완료 카드가 안 나온 item들을 턴 끝에서 정산한다.
    ///   turn 전체 item 목록 → 보정용 완료 이벤트들
    fn reconcile_unfinished_started_items(
        &mut self,
        turn_items: &[ThreadItem],
    ) -> Vec<ThreadEvent> {
        turn_items
            .iter()
            .filter_map(|item| {
                let raw_id = item.id().to_string();
                if !self.raw_to_exec_item_id.contains_key(&raw_id) {
                    return None;
                }
                self.map_completed_item_mut(item.clone())
                    .map(|item| ThreadEvent::ItemCompleted(ItemCompletedEvent { item }))
            })
            .collect()
    }

    /// 🍳 이 함수는 턴 끝 카드 더미에서 마지막 핵심 멘트를 찾는다.
    ///   `ThreadItem` 목록 → 최종 agent/plan 메시지
    fn final_message_from_turn_items(items: &[ThreadItem]) -> Option<String> {
        items
            .iter()
            .rev()
            .find_map(|item| match item {
                ThreadItem::AgentMessage { text, .. } => Some(text.clone()),
                _ => None,
            })
            .or_else(|| {
                items.iter().rev().find_map(|item| match item {
                    ThreadItem::Plan { text, .. } => Some(text.clone()),
                    _ => None,
                })
            })
    }

    /// 🍳 이 함수는 세션 시작 알림을 JSONL 첫 줄 카드로 만든다.
    ///   세션 설정 이벤트 → `thread.started`
    pub fn thread_started_event(session_configured: &SessionConfiguredEvent) -> ThreadEvent {
        ThreadEvent::ThreadStarted(ThreadStartedEvent {
            thread_id: session_configured.session_id.to_string(),
        })
    }

    /// 🍳 이 함수는 일반 warning을 JSONL 에러 아이템 한 장으로 바꾼다.
    ///   경고 문자열 → `CollectedThreadEvents`
    pub fn collect_warning(&mut self, message: String) -> CollectedThreadEvents {
        CollectedThreadEvents {
            events: vec![ThreadEvent::ItemCompleted(ItemCompletedEvent {
                item: ExecThreadItem {
                    id: self.next_item_id(),
                    details: ThreadItemDetails::Error(ErrorItem { message }),
                },
            })],
            status: CodexStatus::Running,
        }
    }

    /// 🍳 이 함수는 서버 알림 하나를 받아,
    ///   필요한 JSONL 이벤트 여러 장으로 펼치고 종료 상태까지 함께 정한다.
    ///   `ServerNotification` → 이벤트 묶음 + `CodexStatus`
    pub fn collect_thread_events(
        &mut self,
        notification: ServerNotification,
    ) -> CollectedThreadEvents {
        let mut events = Vec::new();
        let status = match notification {
            ServerNotification::ConfigWarning(notification) => {
                let message = match notification.details {
                    Some(details) if !details.is_empty() => {
                        format!("{} ({details})", notification.summary)
                    }
                    _ => notification.summary,
                };
                events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent {
                    item: ExecThreadItem {
                        id: self.next_item_id(),
                        details: ThreadItemDetails::Error(ErrorItem { message }),
                    },
                }));
                CodexStatus::Running
            }
            ServerNotification::Error(notification) => {
                let message = match notification.error.additional_details {
                    Some(details) if !details.is_empty() => {
                        format!("{} ({details})", notification.error.message)
                    }
                    _ => notification.error.message,
                };
                let error = ThreadErrorEvent { message };
                self.last_critical_error = Some(error.clone());
                events.push(ThreadEvent::Error(error));
                CodexStatus::Running
            }
            ServerNotification::DeprecationNotice(notification) => {
                let message = match notification.details {
                    Some(details) if !details.is_empty() => {
                        format!("{} ({details})", notification.summary)
                    }
                    _ => notification.summary,
                };
                events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent {
                    item: ExecThreadItem {
                        id: self.next_item_id(),
                        details: ThreadItemDetails::Error(ErrorItem { message }),
                    },
                }));
                CodexStatus::Running
            }
            ServerNotification::HookStarted(_) | ServerNotification::HookCompleted(_) => {
                CodexStatus::Running
            }
            ServerNotification::ItemStarted(notification) => {
                if let Some(item) = self.map_started_item(notification.item) {
                    events.push(ThreadEvent::ItemStarted(ItemStartedEvent { item }));
                }
                CodexStatus::Running
            }
            ServerNotification::ItemCompleted(notification) => {
                if let Some(item) = self.map_completed_item_mut(notification.item) {
                    if let ThreadItemDetails::AgentMessage(AgentMessageItem { text }) =
                        &item.details
                    {
                        self.final_message = Some(text.clone());
                    }
                    events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent { item }));
                }
                CodexStatus::Running
            }
            ServerNotification::ModelRerouted(notification) => {
                events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent {
                    item: ExecThreadItem {
                        id: self.next_item_id(),
                        details: ThreadItemDetails::Error(ErrorItem {
                            message: format!(
                                "model rerouted: {} -> {} ({:?})",
                                notification.from_model, notification.to_model, notification.reason
                            ),
                        }),
                    },
                }));
                CodexStatus::Running
            }
            ServerNotification::ThreadTokenUsageUpdated(notification) => {
                // 🧮 토큰 합계는 turn 종료 때 요약표를 만들 재료라서
                //    최신 값만 계속 기억해 둔다.
                self.last_total_token_usage = Some(notification.token_usage);
                CodexStatus::Running
            }
            ServerNotification::TurnCompleted(notification) => {
                if let Some(running) = self.running_todo_list.take() {
                    events.push(ThreadEvent::ItemCompleted(ItemCompletedEvent {
                        item: ExecThreadItem {
                            id: running.item_id,
                            details: ThreadItemDetails::TodoList(TodoListItem {
                                items: running.items,
                            }),
                        },
                    }));
                }
                events.extend(self.reconcile_unfinished_started_items(&notification.turn.items));
                match notification.turn.status {
                    TurnStatus::Completed => {
                        if let Some(final_message) =
                            Self::final_message_from_turn_items(notification.turn.items.as_slice())
                        {
                            self.final_message = Some(final_message);
                        }
                        self.emit_final_message_on_shutdown = true;
                        events.push(ThreadEvent::TurnCompleted(TurnCompletedEvent {
                            usage: self.usage_from_last_total(),
                        }));
                        CodexStatus::InitiateShutdown
                    }
                    TurnStatus::Failed => {
                        // 🛟 turn 자체 에러가 비어 있더라도,
                        //    직전에 본 치명적 에러나 기본 문구로 빈칸을 메워 JSONL을 완성한다.
                        self.final_message = None;
                        self.emit_final_message_on_shutdown = false;
                        let error = notification
                            .turn
                            .error
                            .map(|error| ThreadErrorEvent {
                                message: match error.additional_details {
                                    Some(details) if !details.is_empty() => {
                                        format!("{} ({details})", error.message)
                                    }
                                    _ => error.message,
                                },
                            })
                            .or_else(|| self.last_critical_error.clone())
                            .unwrap_or_else(|| ThreadErrorEvent {
                                message: "turn failed".to_string(),
                            });
                        events.push(ThreadEvent::TurnFailed(TurnFailedEvent { error }));
                        CodexStatus::InitiateShutdown
                    }
                    TurnStatus::Interrupted => {
                        self.final_message = None;
                        self.emit_final_message_on_shutdown = false;
                        CodexStatus::InitiateShutdown
                    }
                    TurnStatus::InProgress => CodexStatus::Running,
                }
            }
            ServerNotification::TurnDiffUpdated(_) => CodexStatus::Running,
            ServerNotification::TurnPlanUpdated(notification) => {
                let items = Self::map_todo_items(&notification.plan);
                if let Some(running) = self.running_todo_list.as_mut() {
                    running.items = items.clone();
                    let item_id = running.item_id.clone();
                    events.push(ThreadEvent::ItemUpdated(ItemUpdatedEvent {
                        item: ExecThreadItem {
                            id: item_id,
                            details: ThreadItemDetails::TodoList(TodoListItem { items }),
                        },
                    }));
                } else {
                    let item_id = self.next_item_id();
                    self.running_todo_list = Some(RunningTodoList {
                        item_id: item_id.clone(),
                        items: items.clone(),
                    });
                    events.push(ThreadEvent::ItemStarted(ItemStartedEvent {
                        item: ExecThreadItem {
                            id: item_id,
                            details: ThreadItemDetails::TodoList(TodoListItem { items }),
                        },
                    }));
                }
                CodexStatus::Running
            }
            ServerNotification::TurnStarted(_) => {
                events.push(ThreadEvent::TurnStarted(TurnStartedEvent {}));
                CodexStatus::Running
            }
            _ => CodexStatus::Running,
        };

        CollectedThreadEvents { events, status }
    }
}

impl EventProcessor for EventProcessorWithJsonOutput {
    fn print_config_summary(
        &mut self,
        _: &Config,
        _: &str,
        session_configured: &SessionConfiguredEvent,
    ) {
        self.emit(Self::thread_started_event(session_configured));
    }

    fn process_server_notification(&mut self, notification: ServerNotification) -> CodexStatus {
        let collected = self.collect_thread_events(notification);
        for event in collected.events {
            self.emit(event);
        }
        collected.status
    }

    fn process_warning(&mut self, message: String) -> CodexStatus {
        let collected = self.collect_warning(message);
        for event in collected.events {
            self.emit(event);
        }
        collected.status
    }

    fn print_final_output(&mut self) {
        if self.emit_final_message_on_shutdown
            && let Some(path) = self.last_message_path.as_deref()
        {
            // 💾 JSONL 모드여도 마지막 자연어 답변만 따로 뽑아 쓸 수 있게 파일 저장은 동일하게 유지한다.
            handle_last_message(self.final_message.as_deref(), path);
        }
    }
}

#[cfg(test)]
#[path = "event_processor_with_jsonl_output_tests.rs"]
mod tests;
