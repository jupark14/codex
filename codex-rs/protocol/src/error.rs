//! 📄 이 모듈이 하는 일:
//!   실행 중 생길 수 있는 여러 에러를 한곳에 모아 사람이 읽을 메시지와 프로토콜용 분류로 바꿔 준다.
//!   비유로 말하면 사고가 났을 때 "무슨 사고인지", "다시 시도해도 되는지", "바깥 손님에겐 뭐라고 설명할지"를 적는 상황실 매뉴얼이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/core`
//!   - `codex-rs/protocol/src/lib.rs`
//!   - 에러를 UI/네트워크 응답으로 바꾸는 코드
//!
//! 🧩 핵심 개념:
//!   - `CodexErr` = 대표 에러 묶음
//!   - `Display` 구현 = 내부 에러를 사람이 읽는 문장으로 바꾸는 번역기

use crate::ThreadId;
use crate::auth::KnownPlan;
use crate::auth::PlanType;
pub use crate::auth::RefreshTokenFailedError;
pub use crate::auth::RefreshTokenFailedReason;
use crate::exec_output::ExecToolCallOutput;
use crate::network_policy::NetworkPolicyDecisionPayload;
use crate::protocol::CodexErrorInfo;
use crate::protocol::ErrorEvent;
use crate::protocol::RateLimitSnapshot;
use crate::protocol::TruncationPolicy;
use chrono::DateTime;
use chrono::Datelike;
use chrono::Local;
use chrono::Utc;
use codex_async_utils::CancelErr;
use codex_utils_string::truncate_middle_chars;
use codex_utils_string::truncate_middle_with_token_budget;
use reqwest::StatusCode;
use serde_json;
use std::io;
use std::time::Duration;
use thiserror::Error;
use tokio::task::JoinError;

pub type Result<T> = std::result::Result<T, CodexErr>;

/// Limit UI error messages to a reasonable size while keeping useful context.
/// 🍳 이 숫자는 UI 에러 말풍선이 너무 길어져 화면을 다 가리지 않게 하는 최대 길이 울타리다.
const ERROR_MESSAGE_UI_MAX_BYTES: usize = 2 * 1024;

/// 🍳 이 enum은 샌드박스 안에서 난 사고 종류를 분류한 사고 표지판이다.
#[derive(Error, Debug)]
pub enum SandboxErr {
    /// Error from sandbox execution
    #[error(
        "sandbox denied exec error, exit code: {}, stdout: {}, stderr: {}",
        .output.exit_code, .output.stdout.text, .output.stderr.text
    )]
    Denied {
        output: Box<ExecToolCallOutput>,
        network_policy_decision: Option<NetworkPolicyDecisionPayload>,
    },

    /// Error from linux seccomp filter setup
    #[cfg(target_os = "linux")]
    #[error("seccomp setup error")]
    SeccompInstall(#[from] seccompiler::Error),

    /// Error from linux seccomp backend
    #[cfg(target_os = "linux")]
    #[error("seccomp backend error")]
    SeccompBackend(#[from] seccompiler::BackendError),

    /// Command timed out
    #[error("command timed out")]
    Timeout { output: Box<ExecToolCallOutput> },

    /// Command was killed by a signal
    #[error("command was killed by a signal")]
    Signal(i32),

    /// Error from linux landlock
    #[error("Landlock was not able to fully enforce all sandbox rules")]
    LandlockRestrict,
}

/// 🍳 이 enum은 Codex 전체가 공통으로 쓰는 대표 에러 상자다.
///   네트워크/샌드박스/입력 오류 같은 여러 사고를 한 타입으로 묶어 다룬다.
#[derive(Error, Debug)]
pub enum CodexErr {
    #[error("turn aborted. Something went wrong? Hit `/feedback` to report the issue.")]
    TurnAborted,

    /// Returned by ResponsesClient when the SSE stream disconnects or errors out **after** the HTTP
    /// handshake has succeeded but **before** it finished emitting `response.completed`.
    ///
    /// The Session loop treats this as a transient error and will automatically retry the turn.
    ///
    /// Optionally includes the requested delay before retrying the turn.
    #[error("stream disconnected before completion: {0}")]
    Stream(String, Option<Duration>),
    #[error(
        "Codex ran out of room in the model's context window. Start a new thread or clear earlier history before retrying."
    )]
    ContextWindowExceeded,
    #[error("no thread with id: {0}")]
    ThreadNotFound(ThreadId),
    #[error("agent thread limit reached (max {max_threads})")]
    AgentLimitReached { max_threads: usize },
    #[error("session configured event was not the first event in the stream")]
    SessionConfiguredNotFirstEvent,
    /// Returned by run_command_stream when the spawned child process timed out (10s).
    #[error("timeout waiting for child process to exit")]
    Timeout,
    /// Returned by run_command_stream when the child could not be spawned (its stdout/stderr pipes
    /// could not be captured). Analogous to the previous `CodexError::Spawn` variant.
    #[error("spawn failed: child stdout/stderr not captured")]
    Spawn,
    /// Returned by run_command_stream when the user pressed Ctrl-C (SIGINT). Session uses this to
    /// surface a polite FunctionCallOutput back to the model instead of crashing the CLI.
    #[error("interrupted (Ctrl-C). Something went wrong? Hit `/feedback` to report the issue.")]
    Interrupted,
    /// Unexpected HTTP status code.
    #[error("{0}")]
    UnexpectedStatus(UnexpectedResponseError),
    /// Invalid request.
    #[error("{0}")]
    InvalidRequest(String),
    /// Invalid image.
    #[error("Image poisoning")]
    InvalidImageRequest(),
    #[error("{0}")]
    UsageLimitReached(UsageLimitReachedError),
    #[error("Selected model is at capacity. Please try a different model.")]
    ServerOverloaded,
    #[error("{0}")]
    ResponseStreamFailed(ResponseStreamFailed),
    #[error("{0}")]
    ConnectionFailed(ConnectionFailedError),
    #[error("Quota exceeded. Check your plan and billing details.")]
    QuotaExceeded,
    #[error(
        "To use Codex with your ChatGPT plan, upgrade to Plus: https://chatgpt.com/explore/plus."
    )]
    UsageNotIncluded,
    #[error("We're currently experiencing high demand, which may cause temporary errors.")]
    InternalServerError,
    /// Retry limit exceeded.
    #[error("{0}")]
    RetryLimit(RetryLimitReachedError),
    /// Agent loop died unexpectedly
    #[error("internal error; agent loop died unexpectedly")]
    InternalAgentDied,
    /// Sandbox error
    #[error("sandbox error: {0}")]
    Sandbox(#[from] SandboxErr),
    #[error("codex-linux-sandbox was required but not provided")]
    LandlockSandboxExecutableNotProvided,
    #[error("unsupported operation: {0}")]
    UnsupportedOperation(String),
    #[error("{0}")]
    RefreshTokenFailed(RefreshTokenFailedError),
    #[error("Fatal error: {0}")]
    Fatal(String),
    // -----------------------------------------------------------------
    // Automatic conversions for common external error types
    // -----------------------------------------------------------------
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[cfg(target_os = "linux")]
    #[error(transparent)]
    LandlockRuleset(#[from] landlock::RulesetError),
    #[cfg(target_os = "linux")]
    #[error(transparent)]
    LandlockPathFd(#[from] landlock::PathFdError),
    #[error(transparent)]
    TokioJoin(#[from] JoinError),
    #[error("{0}")]
    EnvVar(EnvVarError),
}

impl From<CancelErr> for CodexErr {
    fn from(_: CancelErr) -> Self {
        CodexErr::TurnAborted
    }
}

impl CodexErr {
    /// 🍳 이 함수는 이 에러가 "나중에 한 번 더 시도해 볼 만한 종류"인지 판별한다.
    pub fn is_retryable(&self) -> bool {
        match self {
            CodexErr::TurnAborted
            | CodexErr::Interrupted
            | CodexErr::EnvVar(_)
            | CodexErr::Fatal(_)
            | CodexErr::UsageNotIncluded
            | CodexErr::QuotaExceeded
            | CodexErr::InvalidImageRequest()
            | CodexErr::InvalidRequest(_)
            | CodexErr::RefreshTokenFailed(_)
            | CodexErr::UnsupportedOperation(_)
            | CodexErr::Sandbox(_)
            | CodexErr::LandlockSandboxExecutableNotProvided
            | CodexErr::RetryLimit(_)
            | CodexErr::ContextWindowExceeded
            | CodexErr::ThreadNotFound(_)
            | CodexErr::AgentLimitReached { .. }
            | CodexErr::Spawn
            | CodexErr::SessionConfiguredNotFirstEvent
            | CodexErr::UsageLimitReached(_)
            | CodexErr::ServerOverloaded => false,
            CodexErr::Stream(..)
            | CodexErr::Timeout
            | CodexErr::UnexpectedStatus(_)
            | CodexErr::ResponseStreamFailed(_)
            | CodexErr::ConnectionFailed(_)
            | CodexErr::InternalServerError
            | CodexErr::InternalAgentDied
            | CodexErr::Io(_)
            | CodexErr::Json(_)
            | CodexErr::TokioJoin(_) => true,
            #[cfg(target_os = "linux")]
            CodexErr::LandlockRuleset(_) | CodexErr::LandlockPathFd(_) => false,
        }
    }

    /// Minimal shim so that existing `e.downcast_ref::<CodexErr>()` checks continue to compile
    /// after replacing `anyhow::Error` in the return signature. This mirrors the behavior of
    /// `anyhow::Error::downcast_ref` but works directly on our concrete enum.
    /// 🍳 이 함수는 구체 에러 상자 안에서 원하는 타입이 숨어 있는지 다시 들여다보는 돋보기다.
    pub fn downcast_ref<T: std::any::Any>(&self) -> Option<&T> {
        (self as &dyn std::any::Any).downcast_ref::<T>()
    }

    /// Translate core error to client-facing protocol error.
    /// 🍳 이 함수는 내부 에러를 프로토콜 표준 에러 분류표로 번역한다.
    pub fn to_codex_protocol_error(&self) -> CodexErrorInfo {
        match self {
            CodexErr::ContextWindowExceeded => CodexErrorInfo::ContextWindowExceeded,
            CodexErr::UsageLimitReached(_)
            | CodexErr::QuotaExceeded
            | CodexErr::UsageNotIncluded => CodexErrorInfo::UsageLimitExceeded,
            CodexErr::ServerOverloaded => CodexErrorInfo::ServerOverloaded,
            CodexErr::RetryLimit(_) => CodexErrorInfo::ResponseTooManyFailedAttempts {
                http_status_code: self.http_status_code_value(),
            },
            CodexErr::ConnectionFailed(_) => CodexErrorInfo::HttpConnectionFailed {
                http_status_code: self.http_status_code_value(),
            },
            CodexErr::ResponseStreamFailed(_) => CodexErrorInfo::ResponseStreamConnectionFailed {
                http_status_code: self.http_status_code_value(),
            },
            CodexErr::RefreshTokenFailed(_) => CodexErrorInfo::Unauthorized,
            CodexErr::SessionConfiguredNotFirstEvent
            | CodexErr::InternalServerError
            | CodexErr::InternalAgentDied => CodexErrorInfo::InternalServerError,
            CodexErr::UnsupportedOperation(_)
            | CodexErr::ThreadNotFound(_)
            | CodexErr::AgentLimitReached { .. } => CodexErrorInfo::BadRequest,
            CodexErr::Sandbox(_) => CodexErrorInfo::SandboxError,
            _ => CodexErrorInfo::Other,
        }
    }

    pub fn to_error_event(&self, message_prefix: Option<String>) -> ErrorEvent {
        let error_message = self.to_string();
        let message: String = match message_prefix {
            Some(prefix) => format!("{prefix}: {error_message}"),
            None => error_message,
        };
        ErrorEvent {
            message,
            codex_error_info: Some(self.to_codex_protocol_error()),
        }
    }

    /// 🍳 이 함수는 HTTP 상태 코드가 있으면 숫자만 뽑아 UI/프로토콜이 재사용하게 한다.
    pub fn http_status_code_value(&self) -> Option<u16> {
        let http_status_code = match self {
            CodexErr::RetryLimit(err) => Some(err.status),
            CodexErr::UnexpectedStatus(err) => Some(err.status),
            CodexErr::ConnectionFailed(err) => err.source.status(),
            CodexErr::ResponseStreamFailed(err) => err.source.status(),
            _ => None,
        };
        http_status_code.as_ref().map(StatusCode::as_u16)
    }
}

/// 🍳 이 구조체는 "서버에 닿기도 전에 연결이 실패했다"는 사실을 감싼 포장지다.
#[derive(Debug)]
pub struct ConnectionFailedError {
    pub source: reqwest::Error,
}

impl std::fmt::Display for ConnectionFailedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Connection failed: {}", self.source)
    }
}

/// 🍳 이 구조체는 연결은 됐지만 응답 스트림을 읽다가 중간에 끊긴 사고를 적는다.
#[derive(Debug)]
pub struct ResponseStreamFailed {
    pub source: reqwest::Error,
    pub request_id: Option<String>,
}

impl std::fmt::Display for ResponseStreamFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Error while reading the server response: {}{}",
            self.source,
            self.request_id
                .as_ref()
                .map(|id| format!(", request id: {id}"))
                .unwrap_or_default()
        )
    }
}

/// 🍳 이 구조체는 기대와 다른 HTTP 응답이 왔을 때
///   상태코드/본문/추가 식별자를 함께 들고 있는 기록 카드다.
#[derive(Debug)]
pub struct UnexpectedResponseError {
    pub status: StatusCode,
    pub body: String,
    pub url: Option<String>,
    pub cf_ray: Option<String>,
    pub request_id: Option<String>,
    pub identity_authorization_error: Option<String>,
    pub identity_error_code: Option<String>,
}

const CLOUDFLARE_BLOCKED_MESSAGE: &str =
    "Access blocked by Cloudflare. This usually happens when connecting from a restricted region";
const UNEXPECTED_RESPONSE_BODY_MAX_BYTES: usize = 1000;

impl UnexpectedResponseError {
    /// 🍳 이 함수는 응답 본문에서 사람에게 보여 줄 핵심 문장만 골라 만든다.
    fn display_body(&self) -> String {
        if let Some(message) = self.extract_error_message() {
            return message;
        }

        let trimmed_body = self.body.trim();
        if trimmed_body.is_empty() {
            return "Unknown error".to_string();
        }

        truncate_with_ellipsis(trimmed_body, UNEXPECTED_RESPONSE_BODY_MAX_BYTES)
    }

    /// 🍳 이 함수는 JSON body 안의 `error.message`를 찾아 에러 핵심 문구만 꺼낸다.
    fn extract_error_message(&self) -> Option<String> {
        let json = serde_json::from_str::<serde_json::Value>(&self.body).ok()?;
        let message = json
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)?;
        let message = message.trim();
        if message.is_empty() {
            None
        } else {
            Some(message.to_string())
        }
    }

    /// 🍳 이 함수는 Cloudflare 차단처럼 자주 헷갈리는 403을
    ///   조금 더 친절한 안내 문장으로 바꿀 수 있는지 확인한다.
    fn friendly_message(&self) -> Option<String> {
        if self.status != StatusCode::FORBIDDEN {
            return None;
        }

        if !self.body.contains("Cloudflare") || !self.body.contains("blocked") {
            return None;
        }

        let status = self.status;
        let mut message = format!("{CLOUDFLARE_BLOCKED_MESSAGE} (status {status})");
        if let Some(url) = &self.url {
            message.push_str(&format!(", url: {url}"));
        }
        if let Some(cf_ray) = &self.cf_ray {
            message.push_str(&format!(", cf-ray: {cf_ray}"));
        }
        if let Some(id) = &self.request_id {
            message.push_str(&format!(", request id: {id}"));
        }
        if let Some(auth_error) = &self.identity_authorization_error {
            message.push_str(&format!(", auth error: {auth_error}"));
        }
        if let Some(error_code) = &self.identity_error_code {
            message.push_str(&format!(", auth error code: {error_code}"));
        }

        Some(message)
    }
}

impl std::fmt::Display for UnexpectedResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(friendly) = self.friendly_message() {
            write!(f, "{friendly}")
        } else {
            let status = self.status;
            let body = self.display_body();
            let mut message = format!("unexpected status {status}: {body}");
            if let Some(url) = &self.url {
                message.push_str(&format!(", url: {url}"));
            }
            if let Some(cf_ray) = &self.cf_ray {
                message.push_str(&format!(", cf-ray: {cf_ray}"));
            }
            if let Some(id) = &self.request_id {
                message.push_str(&format!(", request id: {id}"));
            }
            if let Some(auth_error) = &self.identity_authorization_error {
                message.push_str(&format!(", auth error: {auth_error}"));
            }
            if let Some(error_code) = &self.identity_error_code {
                message.push_str(&format!(", auth error code: {error_code}"));
            }
            write!(f, "{message}")
        }
    }
}

impl std::error::Error for UnexpectedResponseError {}

/// 🍳 이 함수는 너무 긴 문장을 앞/뒤 핵심만 남기고 줄이는 가위다.
fn truncate_with_ellipsis(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let mut cut = max_bytes;
    while !text.is_char_boundary(cut) {
        cut = cut.saturating_sub(1);
    }
    let mut truncated = text[..cut].to_string();
    truncated.push_str("...");
    truncated
}

/// 🍳 이 함수는 byte 기준 또는 token 기준 정책에 맞춰 긴 글을 줄인다.
fn truncate_text(content: &str, policy: TruncationPolicy) -> String {
    match policy {
        TruncationPolicy::Bytes(bytes) => truncate_middle_chars(content, bytes),
        TruncationPolicy::Tokens(tokens) => truncate_middle_with_token_budget(content, tokens).0,
    }
}

/// 🍳 이 구조체는 재시도 횟수를 다 써 버렸다는 사실을 담는 경고판이다.
#[derive(Debug)]
pub struct RetryLimitReachedError {
    pub status: StatusCode,
    pub request_id: Option<String>,
}

impl std::fmt::Display for RetryLimitReachedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "exceeded retry limit, last status: {}{}",
            self.status,
            self.request_id
                .as_ref()
                .map(|id| format!(", request id: {id}"))
                .unwrap_or_default()
        )
    }
}

/// 🍳 이 구조체는 사용량 한도 초과 상황에서
///   어떤 플랜인지, 언제 풀리는지, 추가 안내가 뭔지 함께 들고 다닌다.
#[derive(Debug)]
pub struct UsageLimitReachedError {
    pub plan_type: Option<PlanType>,
    pub resets_at: Option<DateTime<Utc>>,
    pub rate_limits: Option<Box<RateLimitSnapshot>>,
    pub promo_message: Option<String>,
}

impl std::fmt::Display for UsageLimitReachedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(limit_name) = self
            .rate_limits
            .as_ref()
            .and_then(|snapshot| snapshot.limit_name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            && !limit_name.eq_ignore_ascii_case("codex")
        {
            return write!(
                f,
                "You've hit your usage limit for {limit_name}. Switch to another model now,{}",
                retry_suffix_after_or(self.resets_at.as_ref())
            );
        }

        if let Some(promo_message) = &self.promo_message {
            return write!(
                f,
                "You've hit your usage limit. {promo_message},{}",
                retry_suffix_after_or(self.resets_at.as_ref())
            );
        }

        let message = match self.plan_type.as_ref() {
            Some(PlanType::Known(KnownPlan::Plus)) => format!(
                "You've hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/codex/settings/usage to purchase more credits{}",
                retry_suffix_after_or(self.resets_at.as_ref())
            ),
            Some(PlanType::Known(
                KnownPlan::Team
                | KnownPlan::SelfServeBusinessUsageBased
                | KnownPlan::Business
                | KnownPlan::EnterpriseCbpUsageBased,
            )) => {
                format!(
                    "You've hit your usage limit. To get more access now, send a request to your admin{}",
                    retry_suffix_after_or(self.resets_at.as_ref())
                )
            }
            Some(PlanType::Known(KnownPlan::Free)) | Some(PlanType::Known(KnownPlan::Go)) => {
                format!(
                    "You've hit your usage limit. Upgrade to Plus to continue using Codex (https://chatgpt.com/explore/plus),{}",
                    retry_suffix_after_or(self.resets_at.as_ref())
                )
            }
            Some(PlanType::Known(KnownPlan::Pro | KnownPlan::ProLite)) => format!(
                "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits{}",
                retry_suffix_after_or(self.resets_at.as_ref())
            ),
            Some(PlanType::Known(KnownPlan::Enterprise))
            | Some(PlanType::Known(KnownPlan::Edu)) => format!(
                "You've hit your usage limit.{}",
                retry_suffix(self.resets_at.as_ref())
            ),
            Some(PlanType::Unknown(_)) | None => format!(
                "You've hit your usage limit.{}",
                retry_suffix(self.resets_at.as_ref())
            ),
        };

        write!(f, "{message}")
    }
}

/// 🍳 이 함수는 "언제 다시 시도하면 되는지" 꼬리 문장을 만든다.
fn retry_suffix(resets_at: Option<&DateTime<Utc>>) -> String {
    if let Some(resets_at) = resets_at {
        let formatted = format_retry_timestamp(resets_at);
        format!(" Try again at {formatted}.")
    } else {
        " Try again later.".to_string()
    }
}

/// 🍳 이 함수는 기존 안내 뒤에 "또는 몇 시에 다시 시도" 문장을 붙이는 버전이다.
fn retry_suffix_after_or(resets_at: Option<&DateTime<Utc>>) -> String {
    if let Some(resets_at) = resets_at {
        let formatted = format_retry_timestamp(resets_at);
        format!(" or try again at {formatted}.")
    } else {
        " or try again later.".to_string()
    }
}

/// 🍳 이 함수는 UTC 시간을 로컬 사람이 읽기 쉬운 시각 문자열로 바꾼다.
fn format_retry_timestamp(resets_at: &DateTime<Utc>) -> String {
    let local_reset = resets_at.with_timezone(&Local);
    let local_now = now_for_retry().with_timezone(&Local);
    if local_reset.date_naive() == local_now.date_naive() {
        local_reset.format("%-I:%M %p").to_string()
    } else {
        let suffix = day_suffix(local_reset.day());
        local_reset
            .format(&format!("%b %-d{suffix}, %Y %-I:%M %p"))
            .to_string()
    }
}

/// 🍳 이 함수는 날짜 숫자 뒤에 붙는 `st/nd/rd/th` 꼬리표를 골라 준다.
fn day_suffix(day: u32) -> &'static str {
    match day {
        11..=13 => "th",
        _ => match day % 10 {
            1 => "st",
            2 => "nd", // codespell:ignore
            3 => "rd",
            _ => "th",
        },
    }
}

#[cfg(test)]
thread_local! {
    static NOW_OVERRIDE: std::cell::RefCell<Option<DateTime<Utc>>> =
        const { std::cell::RefCell::new(None) };
}

fn now_for_retry() -> DateTime<Utc> {
    #[cfg(test)]
    {
        if let Some(now) = NOW_OVERRIDE.with(|cell| *cell.borrow()) {
            return now;
        }
    }
    Utc::now()
}

/// 🍳 이 구조체는 빠진 환경변수 이름과 해결 힌트를 담는 안내문이다.
#[derive(Debug)]
pub struct EnvVarError {
    /// Name of the environment variable that is missing.
    pub var: String,
    /// Optional instructions to help the user get a valid value for the
    /// variable and set it.
    pub instructions: Option<String>,
}

impl std::fmt::Display for EnvVarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Missing environment variable: `{}`.", self.var)?;
        if let Some(instructions) = &self.instructions {
            write!(f, " {instructions}")?;
        }
        Ok(())
    }
}

/// 🍳 이 함수는 내부 에러를 UI 말풍선에 넣기 좋은 길이와 표현으로 다듬는다.
pub fn get_error_message_ui(e: &CodexErr) -> String {
    let message = match e {
        CodexErr::Sandbox(SandboxErr::Denied { output, .. }) => {
            // 📦 샌드박스 거부는 보통 실제 명령 출력이 제일 도움이 되어서,
            //    aggregated output이 있으면 그걸 우선 보여 준다.
            let aggregated = output.aggregated_output.text.trim();
            if !aggregated.is_empty() {
                output.aggregated_output.text.clone()
            } else {
                let stderr = output.stderr.text.trim();
                let stdout = output.stdout.text.trim();
                match (stderr.is_empty(), stdout.is_empty()) {
                    (false, false) => format!("{stderr}\n{stdout}"),
                    (false, true) => output.stderr.text.clone(),
                    (true, false) => output.stdout.text.clone(),
                    (true, true) => format!(
                        "command failed inside sandbox with exit code {}",
                        output.exit_code
                    ),
                }
            }
        }
        // Timeouts are not sandbox errors from a UX perspective; present them plainly.
        CodexErr::Sandbox(SandboxErr::Timeout { output }) => {
            format!(
                "error: command timed out after {} ms",
                output.duration.as_millis()
            )
        }
        _ => e.to_string(),
    };

    truncate_text(
        &message,
        TruncationPolicy::Bytes(ERROR_MESSAGE_UI_MAX_BYTES),
    )
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
