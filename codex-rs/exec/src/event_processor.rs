//! 📄 이 모듈이 하는 일:
//!   실행 중 들어오는 알림을 받아서 "계속 달릴지, 정리하고 멈출지"를 정한다.
//!   비유로 말하면 경기장 전광판 담당이 들어오는 신호를 보고 화면을 바꾸는 역할이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/exec/src/event_processor_with_human_output.rs`
//!   - `codex-rs/exec/src/event_processor_with_jsonl_output.rs`
//!
//! 🧩 핵심 개념:
//!   - `ServerNotification` = 앱 서버가 보내는 상황 보고서
//!   - `CodexStatus` = 다음 차례에 계속 진행할지, 종료 준비할지 알려 주는 손팻말

use std::path::Path;

use codex_app_server_protocol::ServerNotification;
use codex_core::config::Config;
use codex_protocol::protocol::SessionConfiguredEvent;

/// 🍳 이 enum은 초록불/정리벨처럼 exec 루프의 다음 행동을 알려 준다.
///   알림 처리 결과 → 계속 실행 또는 종료 준비
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexStatus {
    Running,
    InitiateShutdown,
}

/// 🍳 이 trait는 "알림을 받으면 어떻게 보여 줄지"를 정하는 공통 약속이다.
///   서버 알림/경고 → 사람용 출력 또는 JSON 출력 처리
pub(crate) trait EventProcessor {
    /// Print summary of effective configuration and user prompt.
    fn print_config_summary(
        &mut self,
        config: &Config,
        prompt: &str,
        session_configured: &SessionConfiguredEvent,
    );

    /// Handle a single typed app-server notification emitted by the agent.
    fn process_server_notification(&mut self, notification: ServerNotification) -> CodexStatus;

    /// Handle a local exec warning that is not represented as an app-server notification.
    fn process_warning(&mut self, message: String) -> CodexStatus;

    fn print_final_output(&mut self) {}
}

/// 🍳 이 함수는 마지막 방송 멘트를 봉투에 담아 저장하는 우체통 역할을 한다.
///   마지막 agent 메시지 옵션 + 출력 파일 경로 → 파일 저장 시도
pub(crate) fn handle_last_message(last_agent_message: Option<&str>, output_file: &Path) {
    // 📭 마지막 멘트가 없으면 빈 종이라도 먼저 넣어 두고,
    //    나중 단계가 "파일이 아예 없다" 때문에 헷갈리지 않게 한다.
    let message = last_agent_message.unwrap_or_default();
    write_last_message_file(message, Some(output_file));
    if last_agent_message.is_none() {
        // 🚨 실제 멘트가 비어 있었다는 사실은 stderr로 따로 알려서
        //    "정상 저장"과 "내용 없음"을 구분할 수 있게 한다.
        eprintln!(
            "Warning: no last agent message; wrote empty content to {}",
            output_file.display()
        );
    }
}

/// 🍳 이 함수는 지정된 경로가 있으면 마지막 메시지를 조용히 써 보는 집배원이다.
///   문자열 내용 + 선택적 파일 경로 → 파일 쓰기 시도
fn write_last_message_file(contents: &str, last_message_path: Option<&Path>) {
    if let Some(path) = last_message_path
        && let Err(e) = std::fs::write(path, contents)
    {
        // 🛟 저장 실패는 프로그램 전체를 멈출 정도 사고는 아니어서,
        //    여기서는 경고만 남기고 본 흐름은 계속 보낸다.
        eprintln!("Failed to write last message file {path:?}: {e}");
    }
}
