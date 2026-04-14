//! 📄 이 모듈이 하는 일:
//!   명령 실행, 네트워크 접근, MCP 호출 같은 위험 행동을 승인 화면에 보여 줄 공통 타입을 정의한다.
//!   비유로 말하면 학교에서 외출증, 특별실 사용 신청서, 보호자 확인표를 같은 행정실 양식으로 묶어 둔 서류함이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/lib.rs`
//!   - 승인 UI/앱 서버/guardian 판단 코드
//!
//! 🧩 핵심 개념:
//!   - approval event = "이 행동을 해도 되는지" 묻는 신청서
//!   - amendment = 다음번엔 같은 종류를 덜 묻게 규칙표를 고쳐 두는 제안서

use crate::mcp::RequestId;
use crate::models::PermissionProfile;
use crate::parse_command::ParsedCommand;
use crate::permissions::FileSystemSandboxPolicy;
use crate::permissions::NetworkSandboxPolicy;
use crate::protocol::FileChange;
use crate::protocol::ReviewDecision;
use crate::protocol::SandboxPolicy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;
use ts_rs::TS;

/// 🍳 이 구조체는 실제 샌드박스/네트워크/파일 권한 세트를 한 번에 들고 다니는 권한 상자다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permissions {
    pub sandbox_policy: SandboxPolicy,
    pub file_system_sandbox_policy: FileSystemSandboxPolicy,
    pub network_sandbox_policy: NetworkSandboxPolicy,
}

/// 🍳 이 enum은 권한을 "프로필 이름표"로 줄지, "실제 권한 묶음"으로 줄지 고르는 포장 방식이다.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscalationPermissions {
    PermissionProfile(PermissionProfile),
    Permissions(Permissions),
}

/// Proposed execpolicy change to allow commands starting with this prefix.
///
/// The `command` tokens form the prefix that would be added as an execpolicy
/// `prefix_rule(..., decision="allow")`, letting the agent bypass approval for
/// commands that start with this token sequence.
/// 🍳 이 구조체는 "이 앞부분으로 시작하는 명령은 다음부터 그냥 통과시켜도 될까?"를 적는 제안서다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(transparent)]
#[ts(type = "Array<string>")]
pub struct ExecPolicyAmendment {
    pub command: Vec<String>,
}

impl ExecPolicyAmendment {
    /// 🍳 이 함수는 허용 prefix 토큰 묶음을 제안서 상자에 담는다.
    pub fn new(command: Vec<String>) -> Self {
        Self { command }
    }

    /// 🍳 이 함수는 제안서 안의 명령 prefix를 읽기 전용으로 꺼낸다.
    pub fn command(&self) -> &[String] {
        &self.command
    }
}

impl From<Vec<String>> for ExecPolicyAmendment {
    fn from(command: Vec<String>) -> Self {
        Self { command }
    }
}

/// 🍳 이 enum은 네트워크 요청이 어떤 통로를 쓰는지 적는 배송 수단 표다.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NetworkApprovalProtocol {
    // TODO(viyatb): Add websocket protocol variants when managed proxy policy
    // decisions expose websocket traffic as a distinct approval context.
    Http,
    #[serde(alias = "https_connect", alias = "http-connect")]
    Https,
    Socks5Tcp,
    Socks5Udp,
}

/// 🍳 이 구조체는 어떤 host에 어떤 프로토콜로 가려는지 적는 네트워크 요청 카드다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct NetworkApprovalContext {
    pub host: String,
    pub protocol: NetworkApprovalProtocol,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicyRuleAction {
    Allow,
    Deny,
}

/// 🍳 이 enum은 guardian이 본 위험도를 신호등처럼 적는 표다.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum GuardianRiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// 🍳 이 enum은 사용자가 이 행동을 얼마나 직접 허락했는지 체온계처럼 나눈다.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum GuardianUserAuthorization {
    Unknown,
    Low,
    Medium,
    High,
}

/// 🍳 이 enum은 guardian 심사표가 지금 어디 단계에 있는지 알려 준다.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum GuardianAssessmentStatus {
    InProgress,
    Approved,
    Denied,
    TimedOut,
    Aborted,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum GuardianAssessmentDecisionSource {
    Agent,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum GuardianCommandSource {
    Shell,
    UnifiedExec,
}

/// 🍳 이 enum은 guardian이 실제로 무엇을 심사했는지 행동 종류별로 나눈다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum GuardianAssessmentAction {
    Command {
        source: GuardianCommandSource,
        command: String,
        cwd: PathBuf,
    },
    Execve {
        source: GuardianCommandSource,
        program: String,
        argv: Vec<String>,
        cwd: PathBuf,
    },
    ApplyPatch {
        cwd: PathBuf,
        files: Vec<PathBuf>,
    },
    NetworkAccess {
        target: String,
        host: String,
        protocol: NetworkApprovalProtocol,
        port: u16,
    },
    McpToolCall {
        server: String,
        tool_name: String,
        connector_id: Option<String>,
        connector_name: Option<String>,
        tool_title: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct NetworkPolicyAmendment {
    pub host: String,
    pub action: NetworkPolicyRuleAction,
}

/// 🍳 이 구조체는 guardian 심사 한 건의 전 과정을 기록하는 생활기록부 한 장이다.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema, TS)]
pub struct GuardianAssessmentEvent {
    /// Stable identifier for this guardian review lifecycle.
    pub id: String,
    /// Thread item being reviewed, when the review maps to a concrete item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target_item_id: Option<String>,
    /// Turn ID that this assessment belongs to.
    /// Uses `#[serde(default)]` for backwards compatibility.
    #[serde(default)]
    pub turn_id: String,
    pub status: GuardianAssessmentStatus,
    /// Coarse risk label. Omitted while the assessment is in progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub risk_level: Option<GuardianRiskLevel>,
    /// How directly the transcript authorizes the reviewed action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub user_authorization: Option<GuardianUserAuthorization>,
    /// Human-readable explanation of the final assessment. Omitted while in progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rationale: Option<String>,
    /// Source that produced the terminal assessment decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub decision_source: Option<GuardianAssessmentDecisionSource>,
    /// Canonical action payload that was reviewed.
    pub action: GuardianAssessmentAction,
}

/// 🍳 이 구조체는 exec 명령 승인 요청서다.
///   어떤 명령을 어느 폴더에서 왜 실행하려는지 + 사용자가 고를 수 있는 승인 선택지
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS)]
pub struct ExecApprovalRequestEvent {
    /// Identifier for the associated command execution item.
    pub call_id: String,
    /// Identifier for this specific approval callback.
    ///
    /// When absent, the approval is for the command item itself (`call_id`).
    /// This is present for subcommand approvals (via execve intercept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<String>,
    /// Turn ID that this command belongs to.
    /// Uses `#[serde(default)]` for backwards compatibility.
    #[serde(default)]
    pub turn_id: String,
    /// The command to be executed.
    pub command: Vec<String>,
    /// The command's working directory.
    pub cwd: PathBuf,
    /// Optional human-readable reason for the approval (e.g. retry without sandbox).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional network context for a blocked request that can be approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub network_approval_context: Option<NetworkApprovalContext>,
    /// Proposed execpolicy amendment that can be applied to allow future runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
    /// Proposed network policy amendments (for example allow/deny this host in future).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub proposed_network_policy_amendments: Option<Vec<NetworkPolicyAmendment>>,
    /// Optional additional filesystem permissions requested for this command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub additional_permissions: Option<PermissionProfile>,
    /// Ordered list of decisions the client may present for this prompt.
    ///
    /// When absent, clients should derive the legacy default set from the
    /// other fields on this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub available_decisions: Option<Vec<ReviewDecision>>,
    pub parsed_cmd: Vec<ParsedCommand>,
}

impl ExecApprovalRequestEvent {
    /// 🍳 이 함수는 세부 approval id가 있으면 그걸 쓰고,
    ///   없으면 기본 call id를 대표 번호로 쓰는 번호표 선택기다.
    pub fn effective_approval_id(&self) -> String {
        self.approval_id
            .clone()
            .unwrap_or_else(|| self.call_id.clone())
    }

    /// 🍳 이 함수는 새 필드가 비어 있어도 예전 클라이언트와 맞게 기본 버튼 목록을 만들어 준다.
    ///   명시된 결정 목록 또는 legacy 규칙 → 실제 승인 선택지
    pub fn effective_available_decisions(&self) -> Vec<ReviewDecision> {
        // available_decisions is a new field that may not be populated by older
        // senders, so we fall back to the legacy logic if it's not present.
        match &self.available_decisions {
            Some(decisions) => decisions.clone(),
            None => Self::default_available_decisions(
                self.network_approval_context.as_ref(),
                self.proposed_execpolicy_amendment.as_ref(),
                self.proposed_network_policy_amendments.as_deref(),
                self.additional_permissions.as_ref(),
            ),
        }
    }

    pub fn default_available_decisions(
        network_approval_context: Option<&NetworkApprovalContext>,
        proposed_execpolicy_amendment: Option<&ExecPolicyAmendment>,
        proposed_network_policy_amendments: Option<&[NetworkPolicyAmendment]>,
        additional_permissions: Option<&PermissionProfile>,
    ) -> Vec<ReviewDecision> {
        if network_approval_context.is_some() {
            let mut decisions = vec![ReviewDecision::Approved, ReviewDecision::ApprovedForSession];
            if let Some(amendment) = proposed_network_policy_amendments.and_then(|amendments| {
                amendments
                    .iter()
                    .find(|amendment| amendment.action == NetworkPolicyRuleAction::Allow)
            }) {
                decisions.push(ReviewDecision::NetworkPolicyAmendment {
                    network_policy_amendment: amendment.clone(),
                });
            }
            decisions.push(ReviewDecision::Abort);
            return decisions;
        }

        if additional_permissions.is_some() {
            return vec![ReviewDecision::Approved, ReviewDecision::Abort];
        }

        let mut decisions = vec![ReviewDecision::Approved];
        if let Some(prefix) = proposed_execpolicy_amendment {
            decisions.push(ReviewDecision::ApprovedExecpolicyAmendment {
                proposed_execpolicy_amendment: prefix.clone(),
            });
        }
        decisions.push(ReviewDecision::Abort);
        decisions
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema, TS)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[ts(tag = "mode")]
pub enum ElicitationRequest {
    Form {
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        #[ts(optional, rename = "_meta")]
        meta: Option<JsonValue>,
        message: String,
        requested_schema: JsonValue,
    },
    Url {
        #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
        #[ts(optional, rename = "_meta")]
        meta: Option<JsonValue>,
        message: String,
        url: String,
        elicitation_id: String,
    },
}

impl ElicitationRequest {
    pub fn message(&self) -> &str {
        match self {
            Self::Form { message, .. } | Self::Url { message, .. } => message,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema, TS)]
pub struct ElicitationRequestEvent {
    /// Turn ID that this elicitation belongs to, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub turn_id: Option<String>,
    pub server_name: String,
    #[ts(type = "string | number")]
    pub id: RequestId,
    pub request: ElicitationRequest,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS)]
pub struct ApplyPatchApprovalRequestEvent {
    /// Responses API call id for the associated patch apply call, if available.
    pub call_id: String,
    /// Turn ID that this patch belongs to.
    /// Uses `#[serde(default)]` for backwards compatibility with older senders.
    #[serde(default)]
    pub turn_id: String,
    pub changes: HashMap<PathBuf, FileChange>,
    /// Optional explanatory reason (e.g. request for extra write access).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When set, the agent is asking the user to allow writes under this root for the remainder of the session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_root: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn guardian_assessment_action_deserializes_command_shape() {
        let action: GuardianAssessmentAction = serde_json::from_value(serde_json::json!({
            "type": "command",
            "source": "shell",
            "command": "rm -rf /tmp/guardian",
            "cwd": "/tmp",
        }))
        .expect("guardian action");

        assert_eq!(
            action,
            GuardianAssessmentAction::Command {
                source: GuardianCommandSource::Shell,
                command: "rm -rf /tmp/guardian".to_string(),
                cwd: PathBuf::from("/tmp"),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn guardian_assessment_action_round_trips_execve_shape() {
        let value = serde_json::json!({
            "type": "execve",
            "source": "shell",
            "program": "/bin/rm",
            "argv": ["/usr/bin/rm", "-f", "/tmp/file.sqlite"],
            "cwd": "/tmp",
        });
        let action: GuardianAssessmentAction =
            serde_json::from_value(value.clone()).expect("guardian action");

        assert_eq!(
            serde_json::to_value(&action).expect("serialize guardian action"),
            value
        );

        assert_eq!(
            action,
            GuardianAssessmentAction::Execve {
                source: GuardianCommandSource::Shell,
                program: "/bin/rm".to_string(),
                argv: vec![
                    "/usr/bin/rm".to_string(),
                    "-f".to_string(),
                    "/tmp/file.sqlite".to_string(),
                ],
                cwd: PathBuf::from("/tmp"),
            }
        );
    }
}
