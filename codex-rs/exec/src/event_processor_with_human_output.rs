//! 📄 이 모듈이 하는 일:
//!   exec 세션에서 들어오는 이벤트를 사람이 읽기 쉬운 문장과 색으로 바꿔서 보여 준다.
//!   비유로 말하면 경기 해설자가 현장 신호를 받아 관중이 이해하기 쉬운 말로 풀어주는 자리다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/exec/src/lib.rs`
//!   - 내부 전용 구현체 (`EventProcessor` 약속을 채우는 사람용 출력 담당)
//!
//! 🧩 핵심 개념:
//!   - `ThreadItem` = 한 턴 안에서 실제로 일어난 작은 사건 카드 묶음
//!   - `Style` = 같은 내용도 중요도에 따라 색연필을 다르게 칠하는 규칙

use std::io::IsTerminal;
use std::path::PathBuf;

use codex_app_server_protocol::CommandExecutionStatus;
use codex_app_server_protocol::McpToolCallStatus;
use codex_app_server_protocol::PatchApplyStatus;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadTokenUsage;
use codex_app_server_protocol::TurnStatus;
use codex_core::config::Config;
use codex_model_provider_info::WireApi;
use codex_protocol::num_format::format_with_separators;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionConfiguredEvent;
use owo_colors::OwoColorize;
use owo_colors::Style;

use crate::event_processor::CodexStatus;
use crate::event_processor::EventProcessor;
use crate::event_processor::handle_last_message;

/// 🍳 이 구조체는 이벤트 카드 꾸러미를 사람용 출력으로 번역하는 해설판이다.
///   서버 알림/실행 결과 → 색 있는 stderr 출력 + 마지막 메시지 저장 상태
pub(crate) struct EventProcessorWithHumanOutput {
    bold: Style,
    cyan: Style,
    dimmed: Style,
    green: Style,
    italic: Style,
    magenta: Style,
    red: Style,
    yellow: Style,
    show_agent_reasoning: bool,
    show_raw_agent_reasoning: bool,
    last_message_path: Option<PathBuf>,
    final_message: Option<String>,
    final_message_rendered: bool,
    emit_final_message_on_shutdown: bool,
    last_total_token_usage: Option<ThreadTokenUsage>,
}

impl EventProcessorWithHumanOutput {
    /// 🍳 이 함수는 색연필 세트를 챙겨서 사람용 해설기를 조립한다.
    ///   ANSI 사용 여부 + config + 마지막 메시지 경로 → 출력용 processor
    pub(crate) fn create_with_ansi(
        with_ansi: bool,
        config: &Config,
        last_message_path: Option<PathBuf>,
    ) -> Self {
        // 🎨 ANSI를 끄면 같은 정보라도 무채색으로 맞춰서,
        //    로그 파일이나 색 없는 터미널에서도 글자가 깨끗하게 보이게 한다.
        let style = |styled: Style, plain: Style| if with_ansi { styled } else { plain };
        Self {
            bold: style(Style::new().bold(), Style::new()),
            cyan: style(Style::new().cyan(), Style::new()),
            dimmed: style(Style::new().dimmed(), Style::new()),
            green: style(Style::new().green(), Style::new()),
            italic: style(Style::new().italic(), Style::new()),
            magenta: style(Style::new().magenta(), Style::new()),
            red: style(Style::new().red(), Style::new()),
            yellow: style(Style::new().yellow(), Style::new()),
            show_agent_reasoning: !config.hide_agent_reasoning,
            show_raw_agent_reasoning: config.show_raw_agent_reasoning,
            last_message_path,
            final_message: None,
            final_message_rendered: false,
            emit_final_message_on_shutdown: false,
            last_total_token_usage: None,
        }
    }

    /// 🍳 이 함수는 막 시작한 작업을 전광판 첫 줄처럼 짧게 알려 준다.
    ///   시작된 `ThreadItem` → 유형별 시작 메시지
    fn render_item_started(&self, item: &ThreadItem) {
        match item {
            ThreadItem::CommandExecution { command, cwd, .. } => {
                eprintln!(
                    "{}\n{} in {}",
                    "exec".style(self.italic).style(self.magenta),
                    command.style(self.bold),
                    cwd.display()
                );
            }
            ThreadItem::McpToolCall { server, tool, .. } => {
                eprintln!(
                    "{} {} {}",
                    "mcp:".style(self.bold),
                    format!("{server}/{tool}").style(self.cyan),
                    "started".style(self.dimmed)
                );
            }
            ThreadItem::WebSearch { query, .. } => {
                eprintln!("{} {}", "web search:".style(self.bold), query);
            }
            ThreadItem::FileChange { .. } => {
                eprintln!("{}", "apply patch".style(self.bold));
            }
            ThreadItem::CollabAgentToolCall { tool, .. } => {
                eprintln!("{} {:?}", "collab:".style(self.bold), tool);
            }
            _ => {}
        }
    }

    /// 🍳 이 함수는 끝난 작업 카드를 펼쳐 보고,
    ///   사람에게 보여 줄 요약과 마지막 메시지 후보를 함께 정리한다.
    ///   완료된 `ThreadItem` → 결과 출력 + 상태 갱신
    fn render_item_completed(&mut self, item: ThreadItem) {
        match item {
            ThreadItem::AgentMessage { text, .. } => {
                eprintln!(
                    "{}\n{}",
                    "codex".style(self.italic).style(self.magenta),
                    text
                );
                self.final_message = Some(text);
                self.final_message_rendered = true;
            }
            ThreadItem::Reasoning {
                summary, content, ..
            } => {
                // 🤔 reasoning 전문은 길 수 있어서 설정에 따라
                //    요약(summary)만 볼지, 원문(content)까지 볼지 갈라서 고른다.
                if self.show_agent_reasoning
                    && let Some(text) =
                        reasoning_text(&summary, &content, self.show_raw_agent_reasoning)
                    && !text.trim().is_empty()
                {
                    eprintln!("{}", text.style(self.dimmed));
                }
            }
            ThreadItem::CommandExecution {
                command: _,
                aggregated_output,
                exit_code,
                status,
                duration_ms,
                ..
            } => {
                let duration_suffix = duration_ms
                    .map(|duration_ms| format!(" in {duration_ms}ms"))
                    .unwrap_or_default();
                match status {
                    CommandExecutionStatus::Completed => {
                        eprintln!(
                            "{}",
                            format!(" succeeded{duration_suffix}:").style(self.green)
                        );
                    }
                    CommandExecutionStatus::Failed => {
                        let exit_code = exit_code.unwrap_or(1);
                        eprintln!(
                            "{}",
                            format!(" exited {exit_code}{duration_suffix}:").style(self.red)
                        );
                    }
                    CommandExecutionStatus::Declined => {
                        eprintln!(
                            "{}",
                            format!(" declined{duration_suffix}:").style(self.yellow)
                        );
                    }
                    CommandExecutionStatus::InProgress => {
                        eprintln!(
                            "{}",
                            format!(" in progress{duration_suffix}:").style(self.dimmed)
                        );
                    }
                }
                if let Some(output) = aggregated_output
                    && !output.trim().is_empty()
                {
                    // 📦 출력이 비어 있지 않을 때만 본문을 보여 줘서
                    //    "성공했지만 할 말은 없음"인 경우 잡음을 줄인다.
                    eprintln!("{output}");
                }
            }
            ThreadItem::FileChange {
                changes, status, ..
            } => {
                let status_text = match status {
                    PatchApplyStatus::Completed => "completed",
                    PatchApplyStatus::Failed => "failed",
                    PatchApplyStatus::Declined => "declined",
                    PatchApplyStatus::InProgress => "in_progress",
                };
                eprintln!("{} {}", "patch:".style(self.bold), status_text);
                for change in changes {
                    eprintln!("{}", change.path.style(self.dimmed));
                }
            }
            ThreadItem::McpToolCall {
                server,
                tool,
                status,
                error,
                ..
            } => {
                let status_text = match status {
                    McpToolCallStatus::Completed => "completed".style(self.green),
                    McpToolCallStatus::Failed => "failed".style(self.red),
                    McpToolCallStatus::InProgress => "in_progress".style(self.dimmed),
                };
                eprintln!(
                    "{} {} {}",
                    "mcp:".style(self.bold),
                    format!("{server}/{tool}").style(self.cyan),
                    format!("({status_text})").style(self.dimmed)
                );
                if let Some(error) = error {
                    eprintln!("{}", error.message.style(self.red));
                }
            }
            ThreadItem::WebSearch { query, .. } => {
                eprintln!("{} {}", "web search:".style(self.bold), query);
            }
            ThreadItem::ContextCompaction { .. } => {
                eprintln!("{}", "context compacted".style(self.dimmed));
            }
            _ => {}
        }
    }
}

impl EventProcessor for EventProcessorWithHumanOutput {
    fn print_config_summary(
        &mut self,
        config: &Config,
        prompt: &str,
        session_configured_event: &SessionConfiguredEvent,
    ) {
        const VERSION: &str = env!("CARGO_PKG_VERSION");
        eprintln!("OpenAI Codex v{VERSION} (research preview)\n--------");
        // 🧾 시작 전에 설정 요약을 먼저 보여 줘야
        //    "어느 모델/샌드박스/승인 정책으로 돌았는지"를 바로 되짚을 수 있다.
        for (key, value) in config_summary_entries(config, session_configured_event) {
            eprintln!("{} {}", format!("{key}:").style(self.bold), value);
        }
        eprintln!("--------");
        eprintln!("{}\n{}", "user".style(self.cyan), prompt);
    }

    fn process_server_notification(&mut self, notification: ServerNotification) -> CodexStatus {
        match notification {
            ServerNotification::ConfigWarning(notification) => {
                let details = notification
                    .details
                    .map(|details| format!(" ({details})"))
                    .unwrap_or_default();
                eprintln!(
                    "{} {}{}",
                    "warning:".style(self.yellow).style(self.bold),
                    notification.summary,
                    details
                );
                CodexStatus::Running
            }
            ServerNotification::Error(notification) => {
                eprintln!(
                    "{} {}",
                    "ERROR:".style(self.red).style(self.bold),
                    notification.error
                );
                CodexStatus::Running
            }
            ServerNotification::DeprecationNotice(notification) => {
                eprintln!(
                    "{} {}",
                    "deprecated:".style(self.yellow).style(self.bold),
                    notification.summary
                );
                if let Some(details) = notification.details {
                    eprintln!("{}", details.style(self.dimmed));
                }
                CodexStatus::Running
            }
            ServerNotification::HookStarted(notification) => {
                eprintln!(
                    "{} {}",
                    "hook:".style(self.bold),
                    format!("{:?}", notification.run.event_name).style(self.dimmed)
                );
                CodexStatus::Running
            }
            ServerNotification::HookCompleted(notification) => {
                eprintln!(
                    "{} {} {:?}",
                    "hook:".style(self.bold),
                    format!("{:?}", notification.run.event_name).style(self.dimmed),
                    notification.run.status
                );
                CodexStatus::Running
            }
            ServerNotification::ItemStarted(notification) => {
                self.render_item_started(&notification.item);
                CodexStatus::Running
            }
            ServerNotification::ItemCompleted(notification) => {
                self.render_item_completed(notification.item);
                CodexStatus::Running
            }
            ServerNotification::ModelRerouted(notification) => {
                eprintln!(
                    "{} {} -> {}",
                    "model rerouted:".style(self.yellow).style(self.bold),
                    notification.from_model,
                    notification.to_model
                );
                CodexStatus::Running
            }
            ServerNotification::ThreadTokenUsageUpdated(notification) => {
                // 🧮 토큰 사용량은 마지막에 한꺼번에 보여 주기 위해
                //    알림이 올 때마다 최신 합계를 덮어쓴다.
                self.last_total_token_usage = Some(notification.token_usage);
                CodexStatus::Running
            }
            ServerNotification::TurnCompleted(notification) => match notification.turn.status {
                TurnStatus::Completed => {
                    let rendered_message = self
                        .final_message_rendered
                        .then(|| self.final_message.clone())
                        .flatten();
                    if let Some(final_message) =
                        final_message_from_turn_items(notification.turn.items.as_slice())
                    {
                        // 🏁 턴 전체 카드에서 마지막 최종 멘트를 다시 찾는 이유는,
                        //    중간에 이미 화면에 찍었더라도 종료 시 저장용 기준을 하나로 맞추기 위해서다.
                        self.final_message_rendered =
                            rendered_message.as_deref() == Some(final_message.as_str());
                        self.final_message = Some(final_message);
                    }
                    self.emit_final_message_on_shutdown = true;
                    CodexStatus::InitiateShutdown
                }
                TurnStatus::Failed => {
                    self.final_message = None;
                    self.final_message_rendered = false;
                    self.emit_final_message_on_shutdown = false;
                    if let Some(error) = notification.turn.error {
                        eprintln!("{} {}", "ERROR:".style(self.red).style(self.bold), error);
                    }
                    CodexStatus::InitiateShutdown
                }
                TurnStatus::Interrupted => {
                    self.final_message = None;
                    self.final_message_rendered = false;
                    self.emit_final_message_on_shutdown = false;
                    eprintln!("{}", "turn interrupted".style(self.dimmed));
                    CodexStatus::InitiateShutdown
                }
                TurnStatus::InProgress => CodexStatus::Running,
            },
            ServerNotification::TurnDiffUpdated(notification) => {
                if !notification.diff.trim().is_empty() {
                    eprintln!("{}", notification.diff);
                }
                CodexStatus::Running
            }
            ServerNotification::TurnPlanUpdated(notification) => {
                if let Some(explanation) = notification.explanation {
                    eprintln!("{}", explanation.style(self.italic));
                }
                for step in notification.plan {
                    match step.status {
                        codex_app_server_protocol::TurnPlanStepStatus::Completed => {
                            eprintln!("  {} {}", "✓".style(self.green), step.step);
                        }
                        codex_app_server_protocol::TurnPlanStepStatus::InProgress => {
                            eprintln!("  {} {}", "→".style(self.cyan), step.step);
                        }
                        codex_app_server_protocol::TurnPlanStepStatus::Pending => {
                            eprintln!(
                                "  {} {}",
                                "•".style(self.dimmed),
                                step.step.style(self.dimmed)
                            );
                        }
                    }
                }
                CodexStatus::Running
            }
            ServerNotification::TurnStarted(_) => CodexStatus::Running,
            _ => CodexStatus::Running,
        }
    }

    fn process_warning(&mut self, message: String) -> CodexStatus {
        eprintln!(
            "{} {message}",
            "warning:".style(self.yellow).style(self.bold)
        );
        CodexStatus::Running
    }

    fn print_final_output(&mut self) {
        if self.emit_final_message_on_shutdown
            && let Some(path) = self.last_message_path.as_deref()
        {
            // 💾 종료 직전 한 번 더 파일로 남겨서,
            //    외부 스크립트가 "마지막 답변"만 따로 읽을 수 있게 한다.
            handle_last_message(self.final_message.as_deref(), path);
        }

        if let Some(usage) = &self.last_total_token_usage {
            eprintln!(
                "{}\n{}",
                "tokens used".style(self.dimmed),
                format_with_separators(blended_total(usage))
            );
        }

        #[allow(clippy::print_stdout)]
        if should_print_final_message_to_stdout(
            self.emit_final_message_on_shutdown
                .then_some(self.final_message.as_deref())
                .flatten(),
            std::io::stdout().is_terminal(),
            std::io::stderr().is_terminal(),
        ) && let Some(message) = self.final_message.as_deref()
        {
            // 📢 stdout이 파이프/파일로 이어진 상황에서는
            //    최종 답변을 stdout으로 보내야 다른 프로그램이 쉽게 받아 적을 수 있다.
            println!("{message}");
        } else if should_print_final_message_to_tty(
            self.emit_final_message_on_shutdown
                .then_some(self.final_message.as_deref())
                .flatten(),
            self.final_message_rendered,
            std::io::stdout().is_terminal(),
            std::io::stderr().is_terminal(),
        ) && let Some(message) = self.final_message.as_deref()
        {
            // 🖥️ 양쪽 다 터미널이면 사람 눈에 잘 띄게 stderr 쪽 해설 포맷으로 다시 찍는다.
            eprintln!(
                "{}\n{}",
                "codex".style(self.italic).style(self.magenta),
                message
            );
        }
    }
}

/// 🍳 이 함수는 설정 여러 개를 한 줄짜리 안내판 목록으로 바꾼다.
///   `Config` + 세션 정보 → 출력용 `(이름, 값)` 목록
fn config_summary_entries(
    config: &Config,
    session_configured_event: &SessionConfiguredEvent,
) -> Vec<(&'static str, String)> {
    let mut entries = vec![
        ("workdir", config.cwd.display().to_string()),
        ("model", session_configured_event.model.clone()),
        (
            "provider",
            session_configured_event.model_provider_id.clone(),
        ),
        (
            "approval",
            config.permissions.approval_policy.value().to_string(),
        ),
        (
            "sandbox",
            summarize_sandbox_policy(config.permissions.sandbox_policy.get()),
        ),
    ];
    if config.model_provider.wire_api == WireApi::Responses {
        // 🤖 Responses API를 쓸 때만 reasoning 관련 옵션이 의미가 있어서
        //    다른 provider에서는 불필요한 줄을 숨긴다.
        entries.push((
            "reasoning effort",
            config
                .model_reasoning_effort
                .map(|effort| effort.to_string())
                .unwrap_or_else(|| "none".to_string()),
        ));
        entries.push((
            "reasoning summaries",
            config
                .model_reasoning_summary
                .map(|summary| summary.to_string())
                .unwrap_or_else(|| "none".to_string()),
        ));
    }
    entries.push((
        "session id",
        session_configured_event.session_id.to_string(),
    ));
    entries
}

/// 🍳 이 함수는 샌드박스 정책을 사람이 읽는 안내 문구로 접는다.
///   내부 정책 enum → 요약 문자열
fn summarize_sandbox_policy(sandbox_policy: &SandboxPolicy) -> String {
    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => "danger-full-access".to_string(),
        SandboxPolicy::ReadOnly { network_access, .. } => {
            let mut summary = "read-only".to_string();
            if *network_access {
                summary.push_str(" (network access enabled)");
            }
            summary
        }
        SandboxPolicy::ExternalSandbox { network_access } => {
            let mut summary = "external-sandbox".to_string();
            if matches!(
                network_access,
                codex_protocol::protocol::NetworkAccess::Enabled
            ) {
                summary.push_str(" (network access enabled)");
            }
            summary
        }
        SandboxPolicy::WorkspaceWrite {
            writable_roots,
            network_access,
            exclude_tmpdir_env_var,
            exclude_slash_tmp,
            read_only_access: _,
        } => {
            let mut summary = "workspace-write".to_string();
            let mut writable_entries = vec!["workdir".to_string()];
            // 🧰 `/tmp`와 `$TMPDIR`는 임시 작업대라서,
            //    제외되지 않았다면 요약에 넣어 "어디까지 쓸 수 있는지"를 분명히 보여 준다.
            if !*exclude_slash_tmp {
                writable_entries.push("/tmp".to_string());
            }
            if !*exclude_tmpdir_env_var {
                writable_entries.push("$TMPDIR".to_string());
            }
            writable_entries.extend(
                writable_roots
                    .iter()
                    .map(|path| path.to_string_lossy().to_string()),
            );
            summary.push_str(&format!(" [{}]", writable_entries.join(", ")));
            if *network_access {
                summary.push_str(" (network access enabled)");
            }
            summary
        }
    }
}

/// 🍳 이 함수는 reasoning 요약본과 원문 중 어느 메모장을 펼칠지 고른다.
///   summary/content + raw 표시 여부 → 출력할 reasoning 문자열
fn reasoning_text(
    summary: &[String],
    content: &[String],
    show_raw_agent_reasoning: bool,
) -> Option<String> {
    let entries = if show_raw_agent_reasoning && !content.is_empty() {
        content
    } else {
        summary
    };
    if entries.is_empty() {
        None
    } else {
        Some(entries.join("\n"))
    }
}

/// 🍳 이 함수는 턴 카드 더미 맨 뒤에서 마지막 핵심 멘트를 찾는다.
///   `ThreadItem` 목록 → 최종 agent 메시지 또는 plan 문장
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

/// 🍳 이 함수는 캐시 덕분에 공짜처럼 다시 쓴 입력 토큰을 빼고,
///   이번 턴에서 실제로 체감한 토큰 총량만 계산한다.
///   전체 token usage → 체감 총합
fn blended_total(usage: &ThreadTokenUsage) -> i64 {
    let cached_input = usage.total.cached_input_tokens.max(0);
    let non_cached_input = (usage.total.input_tokens - cached_input).max(0);
    (non_cached_input + usage.total.output_tokens.max(0)).max(0)
}

/// 🍳 이 함수는 마지막 멘트를 stdout으로 내보낼지 정하는 스위치다.
///   최종 멘트 존재 여부 + stdout/stderr 터미널 상태 → stdout 출력 여부
fn should_print_final_message_to_stdout(
    final_message: Option<&str>,
    stdout_is_terminal: bool,
    stderr_is_terminal: bool,
) -> bool {
    final_message.is_some() && !(stdout_is_terminal && stderr_is_terminal)
}

/// 🍳 이 함수는 마지막 멘트를 터미널용 장식 포맷으로 다시 보여 줄지 판단한다.
///   최종 멘트 + 이미 렌더링했는지 + 터미널 상태 → TTY 재출력 여부
fn should_print_final_message_to_tty(
    final_message: Option<&str>,
    final_message_rendered: bool,
    stdout_is_terminal: bool,
    stderr_is_terminal: bool,
) -> bool {
    final_message.is_some() && !final_message_rendered && stdout_is_terminal && stderr_is_terminal
}

#[cfg(test)]
#[path = "event_processor_with_human_output_tests.rs"]
mod tests;
