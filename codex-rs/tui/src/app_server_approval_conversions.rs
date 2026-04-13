//! 📄 이 파일이 하는 일:
//!   app-server 쪽 승인/권한 타입을 TUI가 쓰는 core/protocol 타입으로 바꿔 준다.
//!   비유로 말하면 바깥 양식으로 온 신청서를 우리 학교 행정실 양식으로 다시 옮겨 적는 번역 창구다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui/src/app.rs`
//!   - `codex-rs/tui/src/app/app_server_requests.rs`
//!
//! 🧩 핵심 개념:
//!   - conversion helper = 비슷하지만 다른 타입 두 묶음을 연결하는 변환 다리
//!   - granted profile = 사용자가 허락한 최종 권한 묶음

use codex_app_server_protocol::AdditionalFileSystemPermissions;
use codex_app_server_protocol::AdditionalNetworkPermissions;
use codex_app_server_protocol::GrantedPermissionProfile;
use codex_app_server_protocol::NetworkApprovalContext as AppServerNetworkApprovalContext;
use codex_protocol::protocol::NetworkApprovalContext;
use codex_protocol::protocol::NetworkApprovalProtocol;
use codex_protocol::request_permissions::RequestPermissionProfile as CoreRequestPermissionProfile;

/// 🍳 이 함수는 app-server 네트워크 승인 문맥을 core 프로토콜 타입으로 바꾼다.
pub(crate) fn network_approval_context_to_core(
    value: AppServerNetworkApprovalContext,
) -> NetworkApprovalContext {
    NetworkApprovalContext {
        host: value.host,
        protocol: match value.protocol {
            codex_app_server_protocol::NetworkApprovalProtocol::Http => {
                NetworkApprovalProtocol::Http
            }
            codex_app_server_protocol::NetworkApprovalProtocol::Https => {
                NetworkApprovalProtocol::Https
            }
            codex_app_server_protocol::NetworkApprovalProtocol::Socks5Tcp => {
                NetworkApprovalProtocol::Socks5Tcp
            }
            codex_app_server_protocol::NetworkApprovalProtocol::Socks5Udp => {
                NetworkApprovalProtocol::Socks5Udp
            }
        },
    }
}

/// 🍳 이 함수는 TUI가 받은 권한 요청 응답을
///   app-server가 기대하는 최종 권한 프로필로 바꿔 담는다.
pub(crate) fn granted_permission_profile_from_request(
    value: CoreRequestPermissionProfile,
) -> GrantedPermissionProfile {
    GrantedPermissionProfile {
        network: value.network.map(|network| AdditionalNetworkPermissions {
            enabled: network.enabled,
        }),
        file_system: value
            .file_system
            .map(|file_system| AdditionalFileSystemPermissions {
                read: file_system.read,
                write: file_system.write,
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::granted_permission_profile_from_request;
    use super::network_approval_context_to_core;
    use codex_protocol::models::FileSystemPermissions;
    use codex_protocol::models::NetworkPermissions;
    use codex_protocol::protocol::NetworkApprovalContext;
    use codex_protocol::protocol::NetworkApprovalProtocol;
    use codex_protocol::request_permissions::RequestPermissionProfile as CoreRequestPermissionProfile;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    fn absolute_path(path: &str) -> AbsolutePathBuf {
        AbsolutePathBuf::try_from(PathBuf::from(path)).expect("path must be absolute")
    }

    #[test]
    fn converts_app_server_network_approval_context_to_core() {
        assert_eq!(
            network_approval_context_to_core(codex_app_server_protocol::NetworkApprovalContext {
                host: "example.com".to_string(),
                protocol: codex_app_server_protocol::NetworkApprovalProtocol::Socks5Tcp,
            }),
            NetworkApprovalContext {
                host: "example.com".to_string(),
                protocol: NetworkApprovalProtocol::Socks5Tcp,
            }
        );
    }

    #[test]
    fn converts_request_permissions_into_granted_permissions() {
        assert_eq!(
            granted_permission_profile_from_request(CoreRequestPermissionProfile {
                network: Some(NetworkPermissions {
                    enabled: Some(true),
                }),
                file_system: Some(FileSystemPermissions {
                    read: Some(vec![absolute_path("/tmp/read-only")]),
                    write: Some(vec![absolute_path("/tmp/write")]),
                }),
            }),
            codex_app_server_protocol::GrantedPermissionProfile {
                network: Some(codex_app_server_protocol::AdditionalNetworkPermissions {
                    enabled: Some(true),
                }),
                file_system: Some(codex_app_server_protocol::AdditionalFileSystemPermissions {
                    read: Some(vec![absolute_path("/tmp/read-only")]),
                    write: Some(vec![absolute_path("/tmp/write")]),
                }),
            }
        );
    }
}
