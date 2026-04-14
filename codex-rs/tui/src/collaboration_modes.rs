//! 📄 이 파일이 하는 일:
//!   TUI에서 보여 줄 collaboration mode preset을 걸러내고 기본/다음 모드를 고르는 helper를 제공한다.
//!   비유로 말하면 여러 수업 모드 중에서 학생용 화면에 보여 줄 모드만 추려서 기본 모드와 다음 모드를 정해 주는 모드 선택표다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui`
//!   - collaboration mode picker / slash command 흐름
//!
//! 🧩 핵심 개념:
//!   - filtered preset = TUI에 보일 수 있는 모드만 남긴 목록
//!   - next cycle = 현재 모드 기준 다음 preset으로 순환하는 규칙

use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::config_types::ModeKind;

use crate::model_catalog::ModelCatalog;

/// 🍳 TUI에 보여 줄 수 있는 mode preset만 남겨 돌려준다.
fn filtered_presets(model_catalog: &ModelCatalog) -> Vec<CollaborationModeMask> {
    model_catalog
        .list_collaboration_modes()
        .into_iter()
        .filter(|mask| mask.mode.is_some_and(ModeKind::is_tui_visible))
        .collect()
}

/// 🍳 TUI용 전체 preset 목록 입구다.
pub(crate) fn presets_for_tui(model_catalog: &ModelCatalog) -> Vec<CollaborationModeMask> {
    filtered_presets(model_catalog)
}

/// 🍳 기본 모드를 찾되 없으면 첫 번째 보이는 preset으로 fallback한다.
pub(crate) fn default_mask(model_catalog: &ModelCatalog) -> Option<CollaborationModeMask> {
    let presets = filtered_presets(model_catalog);
    presets
        .iter()
        .find(|mask| mask.mode == Some(ModeKind::Default))
        .cloned()
        .or_else(|| presets.into_iter().next())
}

/// 🍳 특정 `ModeKind`에 해당하는 preset을 찾는다.
pub(crate) fn mask_for_kind(
    model_catalog: &ModelCatalog,
    kind: ModeKind,
) -> Option<CollaborationModeMask> {
    if !kind.is_tui_visible() {
        return None;
    }
    filtered_presets(model_catalog)
        .into_iter()
        .find(|mask| mask.mode == Some(kind))
}

/// Cycle to the next collaboration mode preset in list order.
/// 🍳 현재 모드 다음 순서의 preset을 순환 방식으로 골라 준다.
pub(crate) fn next_mask(
    model_catalog: &ModelCatalog,
    current: Option<&CollaborationModeMask>,
) -> Option<CollaborationModeMask> {
    let presets = filtered_presets(model_catalog);
    if presets.is_empty() {
        return None;
    }
    let current_kind = current.and_then(|mask| mask.mode);
    let next_index = presets
        .iter()
        .position(|mask| mask.mode == current_kind)
        .map_or(0, |idx| (idx + 1) % presets.len());
    presets.get(next_index).cloned()
}

/// 🍳 기본(Default) 모드 preset helper다.
pub(crate) fn default_mode_mask(model_catalog: &ModelCatalog) -> Option<CollaborationModeMask> {
    mask_for_kind(model_catalog, ModeKind::Default)
}

/// 🍳 Plan 모드 preset helper다.
pub(crate) fn plan_mask(model_catalog: &ModelCatalog) -> Option<CollaborationModeMask> {
    mask_for_kind(model_catalog, ModeKind::Plan)
}
