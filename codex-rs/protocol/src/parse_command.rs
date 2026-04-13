//! 📄 이 모듈이 하는 일:
//!   shell 명령을 "읽기", "파일 목록 보기", "검색" 같은 큰 의도로 나눠 담는 타입을 정의한다.
//!   비유로 말하면 선생님이 학생 행동을 "책 읽기", "서랍 뒤지기", "단어 찾기"처럼 관찰 종류표에 적는 분류표다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/approvals.rs`
//!   - 명령 파싱/승인 UI 표시 코드
//!
//! 🧩 핵심 개념:
//!   - parsed command = 원본 셸 명령을 사람이 이해하는 의도 카드로 다시 적은 것
//!   - `Unknown` = 아직 어떤 의도인지 못 맞힌 명령을 임시로 담는 칸

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;
use ts_rs::TS;

/// 🍳 이 enum은 shell 명령이 무슨 종류 행동인지 분류한 카드다.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParsedCommand {
    Read {
        cmd: String,
        name: String,
        /// (Best effort) Path to the file being read by the command. When
        /// possible, this is an absolute path, though when relative, it should
        /// be resolved against the `cwd`` that will be used to run the command
        /// to derive the absolute path.
        path: PathBuf,
    },
    ListFiles {
        cmd: String,
        path: Option<String>,
    },
    Search {
        cmd: String,
        query: Option<String>,
        path: Option<String>,
    },
    Unknown {
        cmd: String,
    },
}
