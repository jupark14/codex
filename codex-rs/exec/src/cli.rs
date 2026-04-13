//! 📄 이 모듈이 하는 일:
//!   `codex exec` 명령줄 옵션을 읽어서 실행 준비 상자로 정리한다.
//!   비유로 말하면 여행 가기 전에 가방 칸마다 짐을 나눠 담는 안내표다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/exec/src/main.rs`
//!   - `codex-rs/exec/src/lib.rs`
//!
//! 🧩 핵심 개념:
//!   - `clap` = 사용자가 터미널에 적은 글자를 규칙표대로 해석하는 접수 창구
//!   - `Args`/`Parser` derive = 어떤 칸에 어떤 값을 담을지 미리 그려 둔 설계도

use clap::Args;
use clap::FromArgMatches;
use clap::Parser;
use clap::ValueEnum;
use codex_utils_cli::CliConfigOverrides;
use std::path::PathBuf;

/// 🍳 이 구조체는 여행 체크리스트처럼 `codex exec`의 전체 입력을 한 상자에 모아 둔다.
///   터미널 인자들 → 실행 설정 묶음
///   예시: `codex exec -m gpt-5.4 "fix bug"` → `Cli { model: Some(...), prompt: Some(...) }`
#[derive(Parser, Debug)]
#[command(
    version,
    override_usage = "codex exec [OPTIONS] [PROMPT]\n       codex exec [OPTIONS] <COMMAND> [ARGS]"
)]
pub struct Cli {
    /// Action to perform. If omitted, runs a new non-interactive session.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Optional image(s) to attach to the initial prompt.
    #[arg(
        long = "image",
        short = 'i',
        value_name = "FILE",
        value_delimiter = ',',
        num_args = 1..
    )]
    pub images: Vec<PathBuf>,

    /// Model the agent should use.
    #[arg(long, short = 'm', global = true)]
    pub model: Option<String>,

    /// Use open-source provider.
    #[arg(long = "oss", default_value_t = false)]
    pub oss: bool,

    /// Specify which local provider to use (lmstudio or ollama).
    /// If not specified with --oss, will use config default or show selection.
    #[arg(long = "local-provider")]
    pub oss_provider: Option<String>,

    /// Select the sandbox policy to use when executing model-generated shell
    /// commands.
    #[arg(long = "sandbox", short = 's', value_enum)]
    pub sandbox_mode: Option<codex_utils_cli::SandboxModeCliArg>,

    /// Configuration profile from config.toml to specify default options.
    #[arg(long = "profile", short = 'p')]
    pub config_profile: Option<String>,

    /// Convenience alias for low-friction sandboxed automatic execution (--sandbox workspace-write).
    #[arg(long = "full-auto", default_value_t = false, global = true)]
    pub full_auto: bool,

    /// Skip all confirmation prompts and execute commands without sandboxing.
    /// EXTREMELY DANGEROUS. Intended solely for running in environments that are externally sandboxed.
    #[arg(
        long = "dangerously-bypass-approvals-and-sandbox",
        alias = "yolo",
        default_value_t = false,
        global = true,
        conflicts_with = "full_auto"
    )]
    pub dangerously_bypass_approvals_and_sandbox: bool,

    /// Tell the agent to use the specified directory as its working root.
    #[clap(long = "cd", short = 'C', value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Allow running Codex outside a Git repository.
    #[arg(long = "skip-git-repo-check", global = true, default_value_t = false)]
    pub skip_git_repo_check: bool,

    /// Additional directories that should be writable alongside the primary workspace.
    #[arg(long = "add-dir", value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    pub add_dir: Vec<PathBuf>,

    /// Run without persisting session files to disk.
    #[arg(long = "ephemeral", global = true, default_value_t = false)]
    pub ephemeral: bool,

    /// Path to a JSON Schema file describing the model's final response shape.
    #[arg(long = "output-schema", value_name = "FILE")]
    pub output_schema: Option<PathBuf>,

    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Specifies color settings for use in the output.
    #[arg(long = "color", value_enum, default_value_t = Color::Auto)]
    pub color: Color,

    /// Print events to stdout as JSONL.
    #[arg(
        long = "json",
        alias = "experimental-json",
        default_value_t = false,
        global = true
    )]
    pub json: bool,

    /// Specifies file where the last message from the agent should be written.
    #[arg(
        long = "output-last-message",
        short = 'o',
        value_name = "FILE",
        global = true
    )]
    pub last_message_file: Option<PathBuf>,

    /// Initial instructions for the agent. If not provided as an argument (or
    /// if `-` is used), instructions are read from stdin. If stdin is piped and
    /// a prompt is also provided, stdin is appended as a `<stdin>` block.
    #[arg(value_name = "PROMPT", value_hint = clap::ValueHint::Other)]
    pub prompt: Option<String>,
}

/// 🍳 이 enum은 갈림길 표지판처럼 큰 실행 흐름을 고른다.
///   하위 명령 이름 → 어떤 작업으로 들어갈지
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Resume a previous session by id or pick the most recent with --last.
    Resume(ResumeArgs),

    /// Run a code review against the current repository.
    Review(ReviewArgs),
}

/// 🍳 이 구조체는 `resume` 명령의 원본 입력 봉투다.
///   clap이 읽은 위치 인자/옵션 → 아직 해석 전인 값들
#[derive(Args, Debug)]
struct ResumeArgsRaw {
    // Note: This is the direct clap shape. We reinterpret the positional when --last is set
    // so "codex resume --last <prompt>" treats the positional as a prompt, not a session id.
    /// Conversation/session id (UUID) or thread name. UUIDs take precedence if it parses.
    /// If omitted, use --last to pick the most recent recorded session.
    #[arg(value_name = "SESSION_ID")]
    session_id: Option<String>,

    /// Resume the most recent recorded session (newest) without specifying an id.
    #[arg(long = "last", default_value_t = false)]
    last: bool,

    /// Show all sessions (disables cwd filtering).
    #[arg(long = "all", default_value_t = false)]
    all: bool,

    /// Optional image(s) to attach to the prompt sent after resuming.
    #[arg(
        long = "image",
        short = 'i',
        value_name = "FILE",
        value_delimiter = ',',
        num_args = 1
    )]
    images: Vec<PathBuf>,

    /// Prompt to send after resuming the session. If `-` is used, read from stdin.
    #[arg(value_name = "PROMPT", value_hint = clap::ValueHint::Other)]
    prompt: Option<String>,
}

/// 🍳 이 구조체는 `resume` 입력을 사람이 이해하기 쉬운 모양으로 다시 포장한 결과물이다.
///   `ResumeArgsRaw` → 조건 해석이 끝난 resume 설정
#[derive(Debug)]
pub struct ResumeArgs {
    /// Conversation/session id (UUID) or thread name. UUIDs take precedence if it parses.
    /// If omitted, use --last to pick the most recent recorded session.
    pub session_id: Option<String>,

    /// Resume the most recent recorded session (newest) without specifying an id.
    pub last: bool,

    /// Show all sessions (disables cwd filtering).
    pub all: bool,

    /// Optional image(s) to attach to the prompt sent after resuming.
    pub images: Vec<PathBuf>,

    /// Prompt to send after resuming the session. If `-` is used, read from stdin.
    pub prompt: Option<String>,
}

impl From<ResumeArgsRaw> for ResumeArgs {
    fn from(raw: ResumeArgsRaw) -> Self {
        // 🤔 `--last`를 썼는데 prompt 칸이 비어 있으면,
        //    위치 인자를 session id가 아니라 "재개 후 보낼 한마디"로 읽는다.
        // When --last is used without an explicit prompt, treat the positional as the prompt
        // (clap can’t express this conditional positional meaning cleanly).
        let (session_id, prompt) = if raw.last && raw.prompt.is_none() {
            (None, raw.session_id)
        } else {
            (raw.session_id, raw.prompt)
        };
        Self {
            session_id,
            last: raw.last,
            all: raw.all,
            images: raw.images,
            prompt,
        }
    }
}

impl Args for ResumeArgs {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        ResumeArgsRaw::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        ResumeArgsRaw::augment_args_for_update(cmd)
    }
}

impl FromArgMatches for ResumeArgs {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        ResumeArgsRaw::from_arg_matches(matches).map(Self::from)
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        // 🔄 기존 상자를 통째로 새로 갈아 끼워서,
        //    부분만 섞이다가 이전 값이 남는 사고를 막는다.
        *self = ResumeArgsRaw::from_arg_matches(matches).map(Self::from)?;
        Ok(())
    }
}

/// 🍳 이 구조체는 코드 리뷰 주문서를 적는 폼이다.
///   리뷰 기준(브랜치/커밋/로컬 변경) + 추가 지시 → 리뷰 실행 입력
#[derive(Parser, Debug)]
pub struct ReviewArgs {
    /// Review staged, unstaged, and untracked changes.
    #[arg(
        long = "uncommitted",
        default_value_t = false,
        conflicts_with_all = ["base", "commit", "prompt"]
    )]
    pub uncommitted: bool,

    /// Review changes against the given base branch.
    #[arg(
        long = "base",
        value_name = "BRANCH",
        conflicts_with_all = ["uncommitted", "commit", "prompt"]
    )]
    pub base: Option<String>,

    /// Review the changes introduced by a commit.
    #[arg(
        long = "commit",
        value_name = "SHA",
        conflicts_with_all = ["uncommitted", "base", "prompt"]
    )]
    pub commit: Option<String>,

    /// Optional commit title to display in the review summary.
    #[arg(long = "title", value_name = "TITLE", requires = "commit")]
    pub commit_title: Option<String>,

    /// Custom review instructions. If `-` is used, read from stdin.
    #[arg(value_name = "PROMPT", value_hint = clap::ValueHint::Other)]
    pub prompt: Option<String>,
}

/// 🍳 이 enum은 신호등처럼 색을 언제 켤지 고른다.
///   환경 상태 → ANSI 색 사용 여부
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Color {
    Always,
    Never,
    #[default]
    Auto,
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
