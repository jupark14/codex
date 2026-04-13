//! Text encoding detection and conversion utilities for shell output.
//!
//! Windows users frequently run into code pages such as CP1251 or CP866 when invoking commands
//! through VS Code. Those bytes show up as invalid UTF-8 and used to be replaced with the standard
//! Unicode replacement character. We now lean on `chardetng` and `encoding_rs` so we can
//! automatically detect and decode the vast majority of legacy encodings before falling back to
//! lossy UTF-8 decoding.
//!
//! 📄 이 파일이 하는 일:
//!   shell 출력 바이트를 최대한 사람이 읽을 수 있는 문자열로 복원한다.
//!   비유로 말하면 여러 나라 문자로 적힌 쪽지를 받아 가능한 맞는 해독표를 골라 읽어 주는 번역 돋보기다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/protocol/src/error.rs`
//!   - shell/exec 출력 텍스트를 다루는 코드
//!
//! 🧩 핵심 개념:
//!   - encoding detection = 이 바이트가 어떤 문자표로 써졌는지 맞히는 탐정 과정
//!   - fallback = 확신이 부족하면 최소한 깨지지 않는 UTF-8 lossless/lossy 길로 후퇴하는 안전장치

use chardetng::EncodingDetector;
use encoding_rs::Encoding;
use encoding_rs::IBM866;
use encoding_rs::WINDOWS_1252;
use std::time::Duration;

/// 🍳 이 구조체는 stdout/stderr 한 줄 묶음을 텍스트와 잘림 정보까지 함께 담는 상자다.
#[derive(Debug, Clone)]
pub struct StreamOutput<T: Clone> {
    pub text: T,
    pub truncated_after_lines: Option<u32>,
}

impl StreamOutput<String> {
    /// 🍳 이 함수는 이미 문자열인 출력을 "안 잘린 새 상자"로 감싼다.
    pub fn new(text: String) -> Self {
        Self {
            text,
            truncated_after_lines: None,
        }
    }
}

impl StreamOutput<Vec<u8>> {
    /// 🍳 이 함수는 바이트 출력 상자를 사람이 읽는 문자열 상자로 바꾼다.
    pub fn from_utf8_lossy(&self) -> StreamOutput<String> {
        StreamOutput {
            text: bytes_to_string_smart(&self.text),
            truncated_after_lines: self.truncated_after_lines,
        }
    }
}

/// 🍳 이 구조체는 명령 실행 한 번의 최종 결과 영수증이다.
#[derive(Clone, Debug)]
pub struct ExecToolCallOutput {
    pub exit_code: i32,
    pub stdout: StreamOutput<String>,
    pub stderr: StreamOutput<String>,
    pub aggregated_output: StreamOutput<String>,
    pub duration: Duration,
    pub timed_out: bool,
}

impl Default for ExecToolCallOutput {
    fn default() -> Self {
        Self {
            exit_code: 0,
            stdout: StreamOutput::new(String::new()),
            stderr: StreamOutput::new(String::new()),
            aggregated_output: StreamOutput::new(String::new()),
            duration: Duration::ZERO,
            timed_out: false,
        }
    }
}

/// Attempts to convert arbitrary bytes to UTF-8 with best-effort encoding detection.
/// 🍳 이 함수는 바이트 묶음을 가능한 알맞은 문자표로 해독해 문자열로 만든다.
pub fn bytes_to_string_smart(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    if let Ok(utf8_str) = std::str::from_utf8(bytes) {
        return utf8_str.to_owned();
    }

    let encoding = detect_encoding(bytes);
    decode_bytes(bytes, encoding)
}

// Windows-1252 reassigns a handful of 0x80-0x9F slots to smart punctuation (curly quotes, dashes,
// ™). CP866 uses those *same byte values* for uppercase Cyrillic letters. When chardetng sees shell
// snippets that mix these bytes with ASCII it sometimes guesses IBM866, so “smart quotes” render as
// Cyrillic garbage (“УФЦ”) in VS Code. However, CP866 uppercase tokens are perfectly valid output
// (e.g., `ПРИ test`) so we cannot flip every 0x80-0x9F byte to Windows-1252 either. The compromise
// is to only coerce IBM866 to Windows-1252 when (a) the high bytes are exclusively the punctuation
// values listed below and (b) we spot adjacent ASCII. This targets the real failure case without
// clobbering legitimate Cyrillic text. If another code page has a similar collision, introduce a
// dedicated allowlist (like this one) plus unit tests that capture the actual shell output we want
// to preserve. Windows-1252 byte values for smart punctuation.
const WINDOWS_1252_PUNCT_BYTES: [u8; 8] = [
    0x91, // ‘ (left single quotation mark)
    0x92, // ’ (right single quotation mark)
    0x93, // “ (left double quotation mark)
    0x94, // ” (right double quotation mark)
    0x95, // • (bullet)
    0x96, // – (en dash)
    0x97, // — (em dash)
    0x99, // ™ (trade mark sign)
];

/// 🍳 이 함수는 바이트를 보고 가장 그럴듯한 문자표를 고른다.
fn detect_encoding(bytes: &[u8]) -> &'static Encoding {
    let mut detector = EncodingDetector::new();
    detector.feed(bytes, true);
    let (encoding, _is_confident) = detector.guess_assess(None, true);

    // chardetng occasionally reports IBM866 for short strings that only contain Windows-1252 “smart
    // punctuation” bytes (0x80-0x9F) because that range maps to Cyrillic letters in IBM866. When
    // those bytes show up alongside an ASCII word (typical shell output: `"“`test), we know the
    // intent was likely CP1252 quotes/dashes. Prefer WINDOWS_1252 in that specific situation so we
    // render the characters users expect instead of Cyrillic junk. References:
    // - Windows-1252 reserving 0x80-0x9F for curly quotes/dashes:
    //   https://en.wikipedia.org/wiki/Windows-1252
    // - CP866 mapping 0x93/0x94/0x96 to Cyrillic letters, so the same bytes show up as “УФЦ” when
    //   mis-decoded: https://www.unicode.org/Public/MAPPINGS/VENDORS/MICSFT/PC/CP866.TXT
    if encoding == IBM866 && looks_like_windows_1252_punctuation(bytes) {
        return WINDOWS_1252;
    }

    encoding
}

/// 🍳 이 함수는 고른 문자표로 실제 문자열 해독을 시도하고,
///   깨짐이 심하면 UTF-8 lossy 경로로 물러난다.
fn decode_bytes(bytes: &[u8], encoding: &'static Encoding) -> String {
    let (decoded, _, had_errors) = encoding.decode(bytes);

    if had_errors {
        return String::from_utf8_lossy(bytes).into_owned();
    }

    decoded.into_owned()
}

/// Detect whether the byte stream looks like Windows-1252 “smart punctuation” wrapped around
/// otherwise-ASCII text.
///
/// Context: IBM866 and Windows-1252 share the 0x80-0x9F slot range. In IBM866 these bytes decode to
/// Cyrillic letters, whereas Windows-1252 maps them to curly quotes and dashes. chardetng can guess
/// IBM866 for short snippets that only contain those bytes, which turns shell output such as
/// `“test”` into unreadable Cyrillic. To avoid that, we treat inputs comprising a handful of bytes
/// from the problematic range plus ASCII letters as CP1252 punctuation. We deliberately do *not*
/// cap how many of those punctuation bytes we accept: VS Code frequently prints several quoted
/// phrases (e.g., `"foo" - "bar"`), and truncating the count would once again mis-decode those as
/// Cyrillic. If we discover additional encodings with overlapping byte ranges, prefer adding
/// encoding-specific byte allowlists like `WINDOWS_1252_PUNCT` and tests that exercise real-world
/// shell snippets.
fn looks_like_windows_1252_punctuation(bytes: &[u8]) -> bool {
    let mut saw_extended_punctuation = false;
    let mut saw_ascii_word = false;

    for &byte in bytes {
        if byte >= 0xA0 {
            return false;
        }
        if (0x80..=0x9F).contains(&byte) {
            if !is_windows_1252_punct(byte) {
                return false;
            }
            saw_extended_punctuation = true;
        }
        if byte.is_ascii_alphabetic() {
            saw_ascii_word = true;
        }
    }

    saw_extended_punctuation && saw_ascii_word
}

/// 🍳 이 함수는 문제 되는 바이트가 "Windows-1252 스마트 문장부호 허용 목록"인지 확인한다.
fn is_windows_1252_punct(byte: u8) -> bool {
    WINDOWS_1252_PUNCT_BYTES.contains(&byte)
}

#[cfg(test)]
#[path = "exec_output_tests.rs"]
mod tests;
