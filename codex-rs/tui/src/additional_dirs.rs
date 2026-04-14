//! 📄 이 모듈이 하는 일:
//!   `--add-dir`로 받은 추가 쓰기 폴더가 현재 샌드박스 정책에서 무시되는지 확인하고 경고 문구를 만든다.
//!   비유로 말하면 "이 열쇠는 오늘 잠금 규칙 때문에 못 써요"라고 안내문을 붙이는 경비실 메모 생성기다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui`
//!   - CLI/TUI에서 add-dir 경고를 표시하는 코드
//!
//! 🧩 핵심 개념:
//!   - read-only sandbox = 추가 쓰기 루트를 허용할 수 없는 잠금 모드
//!   - warning message = 실제 제한 이유를 사람에게 설명하는 안내문

use codex_protocol::protocol::SandboxPolicy;
use std::path::PathBuf;

/// Returns a warning describing why `--add-dir` entries will be ignored for the
/// resolved sandbox policy. The caller is responsible for presenting the
/// warning to the user (for example, printing to stderr).
/// 🍳 이 함수는 추가 폴더 요청이 현재 샌드박스에서 무시되는 경우에만 경고 문장을 만든다.
pub fn add_dir_warning_message(
    additional_dirs: &[PathBuf],
    sandbox_policy: &SandboxPolicy,
) -> Option<String> {
    if additional_dirs.is_empty() {
        return None;
    }

    match sandbox_policy {
        SandboxPolicy::WorkspaceWrite { .. }
        | SandboxPolicy::DangerFullAccess
        | SandboxPolicy::ExternalSandbox { .. } => None,
        SandboxPolicy::ReadOnly { .. } => Some(format_warning(additional_dirs)),
    }
}

/// 🍳 이 함수는 여러 경로를 한 줄 경고 문장으로 이어 붙여 보여 준다.
fn format_warning(additional_dirs: &[PathBuf]) -> String {
    let joined_paths = additional_dirs
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Ignoring --add-dir ({joined_paths}) because the effective sandbox mode is read-only. Switch to workspace-write or danger-full-access to allow additional writable roots."
    )
}

#[cfg(test)]
mod tests {
    use super::add_dir_warning_message;
    use codex_protocol::protocol::NetworkAccess;
    use codex_protocol::protocol::SandboxPolicy;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    #[test]
    fn returns_none_for_workspace_write() {
        let sandbox = SandboxPolicy::new_workspace_write_policy();
        let dirs = vec![PathBuf::from("/tmp/example")];
        assert_eq!(add_dir_warning_message(&dirs, &sandbox), None);
    }

    #[test]
    fn returns_none_for_danger_full_access() {
        let sandbox = SandboxPolicy::DangerFullAccess;
        let dirs = vec![PathBuf::from("/tmp/example")];
        assert_eq!(add_dir_warning_message(&dirs, &sandbox), None);
    }

    #[test]
    fn returns_none_for_external_sandbox() {
        let sandbox = SandboxPolicy::ExternalSandbox {
            network_access: NetworkAccess::Enabled,
        };
        let dirs = vec![PathBuf::from("/tmp/example")];
        assert_eq!(add_dir_warning_message(&dirs, &sandbox), None);
    }

    #[test]
    fn warns_for_read_only() {
        let sandbox = SandboxPolicy::new_read_only_policy();
        let dirs = vec![PathBuf::from("relative"), PathBuf::from("/abs")];
        let message = add_dir_warning_message(&dirs, &sandbox)
            .expect("expected warning for read-only sandbox");
        assert_eq!(
            message,
            "Ignoring --add-dir (relative, /abs) because the effective sandbox mode is read-only. Switch to workspace-write or danger-full-access to allow additional writable roots."
        );
    }

    #[test]
    fn returns_none_when_no_additional_dirs() {
        let sandbox = SandboxPolicy::new_read_only_policy();
        let dirs: Vec<PathBuf> = Vec::new();
        assert_eq!(add_dir_warning_message(&dirs, &sandbox), None);
    }
}
