//! 📄 이 모듈이 하는 일:
//!   `codex-protocol` crate가 바깥에 내보낼 공통 프로토콜 부품들을 문 앞에서 다시 모아 준다.
//!   비유로 말하면 여러 교실에서 만든 자료를 학교 현관 안내판에 과목별로 정리해 붙이는 목록표다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/core`
//!   - `codex-rs/exec`
//!   - `codex-rs/app-server`
//!
//! 🧩 핵심 개념:
//!   - `pub mod` = 바깥 crate가 직접 들어가 볼 수 있게 교실 문을 여는 것
//!   - `pub use` = 자주 쓰는 타입을 현관 바로 옆에 다시 진열하는 것

pub mod account;
mod agent_path;
pub mod auth;
mod thread_id;
pub use agent_path::AgentPath;
pub use thread_id::ThreadId;
pub mod approvals;
pub mod config_types;
pub mod dynamic_tools;
pub mod error;
pub mod exec_output;
pub mod items;
pub mod mcp;
pub mod memory_citation;
pub mod message_history;
pub mod models;
pub mod network_policy;
pub mod num_format;
pub mod openai_models;
pub mod parse_command;
pub mod permissions;
pub mod plan_tool;
pub mod protocol;
pub mod request_permissions;
pub mod request_user_input;
pub mod user_input;
