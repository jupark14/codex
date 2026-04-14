//! 📄 이 모듈이 하는 일:
//!   thread id를 UUID 기반 새 타입으로 감싸서 문자열과 섞이지 않게 관리한다.
//!   비유로 말하면 각 대화방에 붙는 고유 출석번호를 종이 메모 대신 전용 카드로 보관하는 번호표 상자다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/protocol.rs`
//!   - thread 조회/저장/재개 코드
//!
//! 🧩 핵심 개념:
//!   - newtype = 그냥 문자열 대신 "검사된 thread 번호" 전용 포장지
//!   - UUID v7 = 시간순 정렬에도 유리한 최신 번호표 형식

use std::fmt::Display;

use schemars::JsonSchema;
use schemars::r#gen::SchemaGenerator;
use schemars::schema::Schema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;
use uuid::Uuid;

/// 🍳 이 구조체는 thread 식별자를 UUID 카드 한 장으로 감싼다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, TS, Hash)]
#[ts(type = "string")]
pub struct ThreadId {
    uuid: Uuid,
}

impl ThreadId {
    /// 🍳 이 함수는 새 대화방용 thread 번호표를 발급한다.
    pub fn new() -> Self {
        Self {
            uuid: Uuid::now_v7(),
        }
    }

    /// 🍳 이 함수는 문자열 번호를 읽어 `ThreadId` 카드로 바꾼다.
    pub fn from_string(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self {
            uuid: Uuid::parse_str(s)?,
        })
    }
}

impl TryFrom<&str> for ThreadId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_string(value)
    }
}

impl TryFrom<String> for ThreadId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_string(value.as_str())
    }
}

impl From<ThreadId> for String {
    fn from(value: ThreadId) -> Self {
        value.to_string()
    }
}

impl Default for ThreadId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.uuid, f)
    }
}

impl Serialize for ThreadId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(&self.uuid)
    }
}

impl<'de> Deserialize<'de> for ThreadId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let uuid = Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
        Ok(Self { uuid })
    }
}

impl JsonSchema for ThreadId {
    fn schema_name() -> String {
        "ThreadId".to_string()
    }

    /// 🍳 이 함수는 JSON Schema 입장에서는 thread id를 문자열 한 칸으로 보이게 한다.
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <String>::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_thread_id_default_is_not_zeroes() {
        let id = ThreadId::default();
        assert_ne!(id.uuid, Uuid::nil());
    }
}
