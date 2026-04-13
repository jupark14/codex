//! 📄 이 파일이 하는 일:
//!   skill 목록을 popup에 띄우고, mention/앱 연결과 관련된 helper를 제공한다.
//!   비유로 말하면 도구 보관함에서 어떤 skill을 보여 줄지, 어떤 이름표가 어떤 실물 도구를 가리키는지 정리해 주는 안내 데스크다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/chatwidget.rs`
//!   - skill popup / skill 토글 / mention 해석 흐름
//!
//! 🧩 핵심 개념:
//!   - skill list = 현재 cwd에서 사용 가능한 skill 카드 목록
//!   - tool mention = 텍스트 안 `$name` 또는 링크형 표기를 실제 skill/app 대상으로 다시 연결하는 과정

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use super::ChatWidget;
use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::SkillsToggleItem;
use crate::bottom_pane::SkillsToggleView;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::legacy_core::TOOL_MENTION_SIGIL;
use crate::legacy_core::connectors::connector_mention_slug;
use crate::legacy_core::skills::model::SkillDependencies;
use crate::legacy_core::skills::model::SkillInterface;
use crate::legacy_core::skills::model::SkillMetadata;
use crate::legacy_core::skills::model::SkillToolDependency;
use crate::skills_helpers::skill_description;
use crate::skills_helpers::skill_display_name;
use codex_chatgpt::connectors::AppInfo;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::ListSkillsResponseEvent;
use codex_protocol::protocol::SkillMetadata as ProtocolSkillMetadata;
use codex_protocol::protocol::SkillsListEntry;

impl ChatWidget {
    /// 🍳 composer에 `$`를 넣어 skill 목록 popup을 바로 열도록 유도한다.
    pub(crate) fn open_skills_list(&mut self) {
        self.insert_str("$");
    }

    /// 🍳 skill 관련 행동 메뉴(목록 보기 / on-off 관리)를 띄운다.
    pub(crate) fn open_skills_menu(&mut self) {
        let items = vec![
            SelectionItem {
                name: "List skills".to_string(),
                description: Some("Tip: press $ to open this list directly.".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenSkillsList);
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Enable/Disable Skills".to_string(),
                description: Some("Enable or disable skills.".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenManageSkillsPopup);
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Skills".to_string()),
            subtitle: Some("Choose an action".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    /// 🍳 현재 skill 상태를 토글할 수 있는 popup을 만든다.
    pub(crate) fn open_manage_skills_popup(&mut self) {
        if self.skills_all.is_empty() {
            self.add_info_message("No skills available.".to_string(), /*hint*/ None);
            return;
        }

        let mut initial_state = HashMap::new();
        for skill in &self.skills_all {
            initial_state.insert(normalize_skill_config_path(&skill.path), skill.enabled);
        }
        self.skills_initial_state = Some(initial_state);

        let items: Vec<SkillsToggleItem> = self
            .skills_all
            .iter()
            .map(|skill| {
                let core_skill = protocol_skill_to_core(skill);
                let display_name = skill_display_name(&core_skill).to_string();
                let description = skill_description(&core_skill).to_string();
                let name = core_skill.name.clone();
                let path = core_skill.path_to_skills_md;
                SkillsToggleItem {
                    name: display_name,
                    skill_name: name,
                    description,
                    enabled: skill.enabled,
                    path,
                }
            })
            .collect();

        let view = SkillsToggleView::new(items, self.app_event_tx.clone());
        self.bottom_pane.show_view(Box::new(view));
    }

    /// 🍳 특정 skill path의 enabled 상태를 바꾸고 mention용 skill 집합도 갱신한다.
    pub(crate) fn update_skill_enabled(&mut self, path: PathBuf, enabled: bool) {
        let target = normalize_skill_config_path(&path);
        for skill in &mut self.skills_all {
            if normalize_skill_config_path(&skill.path) == target {
                skill.enabled = enabled;
            }
        }
        self.set_skills(Some(enabled_skills_for_mentions(&self.skills_all)));
    }

    /// 🍳 skill 관리 popup이 닫힌 뒤 실제 변경된 개수를 계산해 요약 메시지를 보여 준다.
    pub(crate) fn handle_manage_skills_closed(&mut self) {
        let Some(initial_state) = self.skills_initial_state.take() else {
            return;
        };
        let mut current_state = HashMap::new();
        for skill in &self.skills_all {
            current_state.insert(normalize_skill_config_path(&skill.path), skill.enabled);
        }

        let mut enabled_count = 0;
        let mut disabled_count = 0;
        for (path, was_enabled) in initial_state {
            let Some(is_enabled) = current_state.get(&path) else {
                continue;
            };
            if was_enabled != *is_enabled {
                if *is_enabled {
                    enabled_count += 1;
                } else {
                    disabled_count += 1;
                }
            }
        }

        if enabled_count == 0 && disabled_count == 0 {
            return;
        }
        self.add_info_message(
            format!("{enabled_count} skills enabled, {disabled_count} skills disabled"),
            /*hint*/ None,
        );
    }

    /// 🍳 app-server 응답에서 현재 cwd용 skill 목록만 꺼내 ChatWidget 상태에 넣는다.
    pub(crate) fn set_skills_from_response(&mut self, response: &ListSkillsResponseEvent) {
        let skills = skills_for_cwd(&self.config.cwd, &response.skills);
        self.skills_all = skills;
        self.set_skills(Some(enabled_skills_for_mentions(&self.skills_all)));
    }

    /// 🍳 parsed command 중 SKILL.md 읽기를 실제 skill 이름이 붙은 라벨로 보강한다.
    pub(crate) fn annotate_skill_reads_in_parsed_cmd(
        &self,
        mut parsed_cmd: Vec<ParsedCommand>,
    ) -> Vec<ParsedCommand> {
        if self.skills_all.is_empty() {
            return parsed_cmd;
        }

        for parsed in &mut parsed_cmd {
            let ParsedCommand::Read { name, path, .. } = parsed else {
                continue;
            };
            if name != "SKILL.md" {
                continue;
            }

            // Best effort only: annotate exact SKILL.md path matches from the loaded skills list.
            if let Some(skill) = self.skills_all.iter().find(|skill| skill.path == *path) {
                *name = format!("{name} ({} skill)", skill.name);
            }
        }

        parsed_cmd
    }
}

/// 🍳 응답 전체에서 현재 작업 폴더(cwd)에 해당하는 skill 목록만 고른다.
fn skills_for_cwd(cwd: &Path, skills_entries: &[SkillsListEntry]) -> Vec<ProtocolSkillMetadata> {
    skills_entries
        .iter()
        .find(|entry| entry.cwd.as_path() == cwd)
        .map(|entry| entry.skills.clone())
        .unwrap_or_default()
}

/// 🍳 enabled=true인 skill만 mention 후보용 core 타입으로 바꿔 돌려준다.
fn enabled_skills_for_mentions(skills: &[ProtocolSkillMetadata]) -> Vec<SkillMetadata> {
    skills
        .iter()
        .filter(|skill| skill.enabled)
        .map(protocol_skill_to_core)
        .collect()
}

/// 🍳 프로토콜용 skill metadata를 core 쪽 `SkillMetadata`로 변환한다.
fn protocol_skill_to_core(skill: &ProtocolSkillMetadata) -> SkillMetadata {
    SkillMetadata {
        name: skill.name.clone(),
        description: skill.description.clone(),
        short_description: skill.short_description.clone(),
        interface: skill.interface.clone().map(|interface| SkillInterface {
            display_name: interface.display_name,
            short_description: interface.short_description,
            icon_small: interface.icon_small,
            icon_large: interface.icon_large,
            brand_color: interface.brand_color,
            default_prompt: interface.default_prompt,
        }),
        dependencies: skill
            .dependencies
            .clone()
            .map(|dependencies| SkillDependencies {
                tools: dependencies
                    .tools
                    .into_iter()
                    .map(|tool| SkillToolDependency {
                        r#type: tool.r#type,
                        value: tool.value,
                        description: tool.description,
                        transport: tool.transport,
                        command: tool.command,
                        url: tool.url,
                    })
                    .collect(),
            }),
        policy: None,
        path_to_skills_md: skill.path.clone(),
        scope: skill.scope,
    }
}

/// 🍳 skill 설정 경로를 canonicalize해서 같은 skill을 같은 경로로 비교하게 만든다.
fn normalize_skill_config_path(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 🍳 텍스트 안 tool mention과 이미 연결된 경로 정보를 합쳐 최종 tool mention 집합을 만든다.
pub(crate) fn collect_tool_mentions(
    text: &str,
    mention_paths: &HashMap<String, String>,
) -> ToolMentions {
    let mut mentions = extract_tool_mentions_from_text(text);
    for (name, path) in mention_paths {
        if mentions.names.contains(name) {
            mentions.linked_paths.insert(name.clone(), path.clone());
        }
    }
    mentions
}

/// 🍳 mention이 가리키는 skill 경로나 skill 이름을 기준으로 관련 skill 카드들을 찾아낸다.
pub(crate) fn find_skill_mentions_with_tool_mentions(
    mentions: &ToolMentions,
    skills: &[SkillMetadata],
) -> Vec<SkillMetadata> {
    let mention_skill_paths: HashSet<&str> = mentions
        .linked_paths
        .values()
        .filter(|path| is_skill_path(path))
        .map(|path| normalize_skill_path(path))
        .collect();

    let mut seen_names = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut matches: Vec<SkillMetadata> = Vec::new();

    for skill in skills {
        if seen_paths.contains(&skill.path_to_skills_md) {
            continue;
        }
        let path_str = skill.path_to_skills_md.to_string_lossy();
        if mention_skill_paths.contains(path_str.as_ref()) {
            seen_paths.insert(skill.path_to_skills_md.clone());
            seen_names.insert(skill.name.clone());
            matches.push(skill.clone());
        }
    }

    for skill in skills {
        if seen_paths.contains(&skill.path_to_skills_md) {
            continue;
        }
        if mentions.names.contains(&skill.name) && seen_names.insert(skill.name.clone()) {
            seen_paths.insert(skill.path_to_skills_md.clone());
            matches.push(skill.clone());
        }
    }

    matches
}

/// 🍳 mention과 앱 목록을 대조해서 실제로 참조된 app만 골라낸다.
pub(crate) fn find_app_mentions(
    mentions: &ToolMentions,
    apps: &[AppInfo],
    skill_names_lower: &HashSet<String>,
) -> Vec<AppInfo> {
    let mut explicit_names = HashSet::new();
    let mut selected_ids = HashSet::new();
    for (name, path) in &mentions.linked_paths {
        if let Some(connector_id) = app_id_from_path(path) {
            explicit_names.insert(name.clone());
            selected_ids.insert(connector_id.to_string());
        }
    }

    let mut slug_counts: HashMap<String, usize> = HashMap::new();
    for app in apps.iter().filter(|app| app.is_enabled) {
        let slug = connector_mention_slug(app);
        *slug_counts.entry(slug).or_insert(0) += 1;
    }

    for app in apps.iter().filter(|app| app.is_enabled) {
        let slug = connector_mention_slug(app);
        let slug_count = slug_counts.get(&slug).copied().unwrap_or(0);
        if mentions.names.contains(&slug)
            && !explicit_names.contains(&slug)
            && slug_count == 1
            && !skill_names_lower.contains(&slug)
        {
            selected_ids.insert(app.id.clone());
        }
    }

    apps.iter()
        .filter(|app| app.is_enabled && selected_ids.contains(&app.id))
        .cloned()
        .collect()
}

/// 🍳 텍스트에서 뽑아낸 tool mention 이름들과 링크 경로를 묶는 결과 상자다.
pub(crate) struct ToolMentions {
    names: HashSet<String>,
    linked_paths: HashMap<String, String>,
}

/// 🍳 일반 텍스트에서 tool mention을 추출하는 기본 입구다.
fn extract_tool_mentions_from_text(text: &str) -> ToolMentions {
    extract_tool_mentions_from_text_with_sigil(text, TOOL_MENTION_SIGIL)
}

/// 🍳 특정 sigil(`$` 등)을 기준으로 mention 이름과 링크형 표기를 찾아낸다.
fn extract_tool_mentions_from_text_with_sigil(text: &str, sigil: char) -> ToolMentions {
    let text_bytes = text.as_bytes();
    let mut names: HashSet<String> = HashSet::new();
    let mut linked_paths: HashMap<String, String> = HashMap::new();

    let mut index = 0;
    while index < text_bytes.len() {
        let byte = text_bytes[index];
        if byte == b'['
            && let Some((name, path, end_index)) =
                parse_linked_tool_mention(text, text_bytes, index, sigil)
        {
            if !is_common_env_var(name) {
                if is_skill_path(path) {
                    names.insert(name.to_string());
                }
                linked_paths
                    .entry(name.to_string())
                    .or_insert(path.to_string());
            }
            index = end_index;
            continue;
        }

        if byte != sigil as u8 {
            index += 1;
            continue;
        }

        let name_start = index + 1;
        let Some(first_name_byte) = text_bytes.get(name_start) else {
            index += 1;
            continue;
        };
        if !is_mention_name_char(*first_name_byte) {
            index += 1;
            continue;
        }

        let mut name_end = name_start + 1;
        while let Some(next_byte) = text_bytes.get(name_end)
            && is_mention_name_char(*next_byte)
        {
            name_end += 1;
        }

        let name = &text[name_start..name_end];
        if !is_common_env_var(name) {
            names.insert(name.to_string());
        }
        index = name_end;
    }

    ToolMentions {
        names,
        linked_paths,
    }
}

/// 🍳 `[$name](path)` 꼴의 링크형 tool mention을 파싱한다.
fn parse_linked_tool_mention<'a>(
    text: &'a str,
    text_bytes: &[u8],
    start: usize,
    sigil: char,
) -> Option<(&'a str, &'a str, usize)> {
    let sigil_index = start + 1;
    if text_bytes.get(sigil_index) != Some(&(sigil as u8)) {
        return None;
    }

    let name_start = sigil_index + 1;
    let first_name_byte = text_bytes.get(name_start)?;
    if !is_mention_name_char(*first_name_byte) {
        return None;
    }

    let mut name_end = name_start + 1;
    while let Some(next_byte) = text_bytes.get(name_end)
        && is_mention_name_char(*next_byte)
    {
        name_end += 1;
    }

    if text_bytes.get(name_end) != Some(&b']') {
        return None;
    }

    let mut path_start = name_end + 1;
    while let Some(next_byte) = text_bytes.get(path_start)
        && next_byte.is_ascii_whitespace()
    {
        path_start += 1;
    }
    if text_bytes.get(path_start) != Some(&b'(') {
        return None;
    }

    let mut path_end = path_start + 1;
    while let Some(next_byte) = text_bytes.get(path_end)
        && *next_byte != b')'
    {
        path_end += 1;
    }
    if text_bytes.get(path_end) != Some(&b')') {
        return None;
    }

    let path = text[path_start + 1..path_end].trim();
    if path.is_empty() {
        return None;
    }

    let name = &text[name_start..name_end];
    Some((name, path, path_end + 1))
}

/// 🍳 PATH/HOME 같은 흔한 환경변수 이름은 tool mention으로 오인하지 않게 걸러낸다.
fn is_common_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "PATH"
            | "HOME"
            | "USER"
            | "SHELL"
            | "PWD"
            | "TMPDIR"
            | "TEMP"
            | "TMP"
            | "LANG"
            | "TERM"
            | "XDG_CONFIG_HOME"
    )
}

/// 🍳 mention 이름에 들어갈 수 있는 문자인지 판별한다.
fn is_mention_name_char(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-')
}

/// 🍳 경로가 skill 계열인지 app/mcp/plugin 계열인지 구분한다.
fn is_skill_path(path: &str) -> bool {
    !path.starts_with("app://") && !path.starts_with("mcp://") && !path.starts_with("plugin://")
}

/// 🍳 `skill://` 접두사가 있으면 떼고 비교용 경로만 남긴다.
fn normalize_skill_path(path: &str) -> &str {
    path.strip_prefix("skill://").unwrap_or(path)
}

/// 🍳 `app://...` 경로에서 실제 app id만 꺼낸다.
fn app_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("app://")
        .filter(|value| !value.is_empty())
}
