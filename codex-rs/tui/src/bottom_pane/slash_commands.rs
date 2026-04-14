//! Shared helpers for filtering and matching built-in slash commands.
//!
//! The same sandbox- and feature-gating rules are used by both the composer
//! and the command popup. Centralizing them here keeps those call sites small
//! and ensures they stay in sync.
//!
//! 📄 이 파일이 하는 일:
//!   현재 기능 플래그/샌드박스 상태에 맞춰 어떤 slash 명령이 보여야 하는지 골라 준다.
//!   비유로 말하면 놀이공원 입장권 종류를 보고 탈 수 있는 놀이기구 목록만 열어 주는 게이트 검사표다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/bottom_pane/chat_composer.rs`
//!   - `codex-rs/tui/src/bottom_pane/command_popup.rs`
//!
//! 🧩 핵심 개념:
//!   - gating = 지금 상태에서 허용된 명령만 남기는 필터
//!   - builtin command = 미리 정의된 slash 명령
use std::str::FromStr;

use codex_utils_fuzzy_match::fuzzy_match;

use crate::slash_command::SlashCommand;
use crate::slash_command::built_in_slash_commands;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuiltinCommandFlags {
    pub(crate) collaboration_modes_enabled: bool,
    pub(crate) connectors_enabled: bool,
    pub(crate) plugins_command_enabled: bool,
    pub(crate) fast_command_enabled: bool,
    pub(crate) personality_command_enabled: bool,
    pub(crate) realtime_conversation_enabled: bool,
    pub(crate) audio_device_selection_enabled: bool,
    pub(crate) allow_elevate_sandbox: bool,
}

/// Return the built-ins that should be visible/usable for the current input.
/// 🍳 이 함수는 현재 플래그에 맞는 builtin slash 명령만 골라 목록으로 돌려준다.
pub(crate) fn builtins_for_input(flags: BuiltinCommandFlags) -> Vec<(&'static str, SlashCommand)> {
    built_in_slash_commands()
        .into_iter()
        .filter(|(_, cmd)| flags.allow_elevate_sandbox || *cmd != SlashCommand::ElevateSandbox)
        .filter(|(_, cmd)| {
            flags.collaboration_modes_enabled
                || !matches!(*cmd, SlashCommand::Collab | SlashCommand::Plan)
        })
        .filter(|(_, cmd)| flags.connectors_enabled || *cmd != SlashCommand::Apps)
        .filter(|(_, cmd)| flags.plugins_command_enabled || *cmd != SlashCommand::Plugins)
        .filter(|(_, cmd)| flags.fast_command_enabled || *cmd != SlashCommand::Fast)
        .filter(|(_, cmd)| flags.personality_command_enabled || *cmd != SlashCommand::Personality)
        .filter(|(_, cmd)| flags.realtime_conversation_enabled || *cmd != SlashCommand::Realtime)
        .filter(|(_, cmd)| flags.audio_device_selection_enabled || *cmd != SlashCommand::Settings)
        .collect()
}

/// Find a single built-in command by exact name, after applying the gating rules.
/// 🍳 이 함수는 이름이 정확히 맞는 명령이 지금 허용되는지 확인한다.
pub(crate) fn find_builtin_command(name: &str, flags: BuiltinCommandFlags) -> Option<SlashCommand> {
    let cmd = SlashCommand::from_str(name).ok()?;
    builtins_for_input(flags)
        .into_iter()
        .any(|(_, visible_cmd)| visible_cmd == cmd)
        .then_some(cmd)
}

/// Whether any visible built-in fuzzily matches the provided prefix.
/// 🍳 이 함수는 사용자가 친 접두어와 비슷한 명령이 하나라도 보이는지 확인한다.
pub(crate) fn has_builtin_prefix(name: &str, flags: BuiltinCommandFlags) -> bool {
    builtins_for_input(flags)
        .into_iter()
        .any(|(command_name, _)| fuzzy_match(command_name, name).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn all_enabled_flags() -> BuiltinCommandFlags {
        BuiltinCommandFlags {
            collaboration_modes_enabled: true,
            connectors_enabled: true,
            plugins_command_enabled: true,
            fast_command_enabled: true,
            personality_command_enabled: true,
            realtime_conversation_enabled: true,
            audio_device_selection_enabled: true,
            allow_elevate_sandbox: true,
        }
    }

    #[test]
    fn debug_command_still_resolves_for_dispatch() {
        let cmd = find_builtin_command("debug-config", all_enabled_flags());
        assert_eq!(cmd, Some(SlashCommand::DebugConfig));
    }

    #[test]
    fn clear_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("clear", all_enabled_flags()),
            Some(SlashCommand::Clear)
        );
    }

    #[test]
    fn stop_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("stop", all_enabled_flags()),
            Some(SlashCommand::Stop)
        );
    }

    #[test]
    fn clean_command_alias_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("clean", all_enabled_flags()),
            Some(SlashCommand::Stop)
        );
    }

    #[test]
    fn fast_command_is_hidden_when_disabled() {
        let mut flags = all_enabled_flags();
        flags.fast_command_enabled = false;
        assert_eq!(find_builtin_command("fast", flags), None);
    }

    #[test]
    fn realtime_command_is_hidden_when_realtime_is_disabled() {
        let mut flags = all_enabled_flags();
        flags.realtime_conversation_enabled = false;
        assert_eq!(find_builtin_command("realtime", flags), None);
    }

    #[test]
    fn settings_command_is_hidden_when_realtime_is_disabled() {
        let mut flags = all_enabled_flags();
        flags.realtime_conversation_enabled = false;
        flags.audio_device_selection_enabled = false;
        assert_eq!(find_builtin_command("settings", flags), None);
    }

    #[test]
    fn settings_command_is_hidden_when_audio_device_selection_is_disabled() {
        let mut flags = all_enabled_flags();
        flags.audio_device_selection_enabled = false;
        assert_eq!(find_builtin_command("settings", flags), None);
    }
}
