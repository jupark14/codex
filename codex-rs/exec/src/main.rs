//! Entry-point for the `codex-exec` binary.
//!
//! When this CLI is invoked normally, it parses the standard `codex-exec` CLI
//! options and launches the non-interactive Codex agent. However, if it is
//! invoked with arg0 as `codex-linux-sandbox`, we instead treat the invocation
//! as a request to run the logic for the standalone `codex-linux-sandbox`
//! executable (i.e., parse any -s args and then run a *sandboxed* command under
//! Landlock + seccomp.
//!
//! This allows us to ship a completely separate set of functionality as part
//! of the `codex-exec` binary.
//!
//! 📄 이 파일이 하는 일:
//!   `codex-exec` 바이너리가 처음 켜질 때 어느 입구로 들어갈지 고른다.
//!   비유로 말하면 같은 건물 문이라도 "일반 손님"인지 "보안 점검 기사"인지 보고 다른 안내 데스크로 보내는 로비다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/exec/src/lib.rs`
//!   - 실제 `codex-exec` 실행 바이너리 진입점
//!
//! 🧩 핵심 개념:
//!   - `arg0` 분기 = 실행 파일 이름표를 보고 어떤 기능으로 켜졌는지 판단하는 갈림길
//!   - `TopCli` = 바깥 공통 옵션과 실제 exec 옵션을 한 번에 담는 접수 카드
use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_exec::Cli;
use codex_exec::run_main;
use codex_utils_cli::CliConfigOverrides;

/// 🍳 이 구조체는 문 앞 안내 데스크에서 받은 공통 옵션과 실제 exec 옵션을 한 상자에 담는다.
///   루트 override + 내부 `Cli` → 최종 진입 입력
#[derive(Parser, Debug)]
struct TopCli {
    #[clap(flatten)]
    config_overrides: CliConfigOverrides,

    #[clap(flatten)]
    inner: Cli,
}

/// 🍳 이 함수는 건물 정문 경비처럼,
///   실행 파일 이름과 CLI 입력을 보고 올바른 시작 루트로 연결한다.
///   프로세스 시작 → `codex exec` 실행 또는 sandbox 진입
fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        let top_cli = TopCli::parse();
        // 🧺 바깥쪽 override를 안쪽 CLI 앞에 먼저 끼워 넣어서,
        //    downstream 로직은 그대로 두고도 최상단 옵션이 우선 적용되게 만든다.
        // Merge root-level overrides into inner CLI struct so downstream logic remains unchanged.
        let mut inner = top_cli.inner;
        inner
            .config_overrides
            .raw_overrides
            .splice(0..0, top_cli.config_overrides.raw_overrides);

        run_main(inner, arg0_paths).await?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
