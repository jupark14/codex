//! 📄 이 모듈이 하는 일:
//!   추가 권한 요청 도구가 주고받는 인자/응답/이벤트 타입을 정의한다.
//!   비유로 말하면 "이번 숙제에서 인터넷 써도 돼요?" 같은 허가서를 신청서, 답변서, 기록지로 나눠 만든 양식함이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/protocol.rs`
//!   - 권한 요청 도구 및 승인 UI 코드
//!
//! 🧩 핵심 개념:
//!   - `RequestPermissionProfile` = 이번에 부탁하는 권한 묶음
//!   - `PermissionGrantScope` = 한 턴만 허용할지, 세션 전체에 열어 둘지 정하는 범위 스위치

use crate::models::FileSystemPermissions;
use crate::models::NetworkPermissions;
use crate::models::PermissionProfile;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// 🍳 이 enum은 승인된 권한을 이번 턴에만 쓸지, 세션 전체에 유지할지 고르는 범위표다.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PermissionGrantScope {
    #[default]
    Turn,
    Session,
}

/// 🍳 이 구조체는 요청/응답에서 쓰는 권한 묶음 카드다.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct RequestPermissionProfile {
    pub network: Option<NetworkPermissions>,
    pub file_system: Option<FileSystemPermissions>,
}

impl RequestPermissionProfile {
    /// 🍳 이 함수는 카드 안에 실제로 요청한 권한이 하나라도 있는지 본다.
    pub fn is_empty(&self) -> bool {
        self.network.is_none() && self.file_system.is_none()
    }
}

impl From<RequestPermissionProfile> for PermissionProfile {
    fn from(value: RequestPermissionProfile) -> Self {
        Self {
            network: value.network,
            file_system: value.file_system,
        }
    }
}

impl From<PermissionProfile> for RequestPermissionProfile {
    fn from(value: PermissionProfile) -> Self {
        Self {
            network: value.network,
            file_system: value.file_system,
        }
    }
}

/// 🍳 이 구조체는 권한 요청 도구에 보내는 신청서다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestPermissionsArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub permissions: RequestPermissionProfile,
}

/// 🍳 이 구조체는 권한 요청이 승인된 뒤 돌아오는 답변서다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestPermissionsResponse {
    pub permissions: RequestPermissionProfile,
    #[serde(default)]
    pub scope: PermissionGrantScope,
}

/// 🍳 이 구조체는 클라이언트에 보여 줄 "권한 요청 발생" 이벤트 카드다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestPermissionsEvent {
    /// Responses API call id for the associated tool call, if available.
    pub call_id: String,
    /// Turn ID that this request belongs to.
    /// Uses `#[serde(default)]` for backwards compatibility.
    #[serde(default)]
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub permissions: RequestPermissionProfile,
}
