// ## 📐 Architecture Overview
//
// 이 파일은 Codex AI 세션(=Thread)의 **생성·보관·분기·종료**를 총괄하는
// 중앙 관제탑이다.
//
// ```
// ┌─────────────────────────────────────────────────────────┐
// │  외부 호출자 (app-server, tui, exec)                     │
// │      │ start_thread / fork_thread / shutdown            │
// └──────┼──────────────────────────────────────────────────┘
//        │
//        ▼
// ┌──────────────────────────────────────┐
// │          ThreadManager               │  ← 공개 API 창구
// │   Arc<ThreadManagerState>  ─────────┐│
// └──────────────────────────────────────┘│
//                                         │
//        ┌────────────────────────────────┘
//        ▼
// ┌──────────────────────────────────────────────────────────┐
// │           ThreadManagerState (Arc 공유 내부 상태)          │
// │                                                          │
// │  threads: HashMap<ThreadId, Arc<CodexThread>>  ← 살아있는 세션 목록
// │  thread_created_tx: broadcast::Sender           ← 구독자에게 알림
// │  models/plugins/mcp/skills managers             ← 공유 자원들
// └──────────────────────────────────────────────────────────┘
//        │ spawn_thread_with_source
//        ▼
// ┌──────────────────────────────────────┐
// │     Codex::spawn(...)                │  ← 실제 AI 엔진 초기화
// │     → CodexThread (Arc)              │  ← 세션 핸들
// └──────────────────────────────────────┘
// ```
//
// **데이터·제어 흐름에서의 위치:**
//   - `codex-rs/app-server` 가 WebSocket 연결마다 이 파일의 API를 호출한다.
//   - 이 파일이 없으면 어떤 AI 대화도 시작될 수 없다.
//   - `AgentControl` 은 이 파일 내부의 `ThreadManagerState` 를 `Weak` 포인터로 역참조한다.
//     (🔗 [[agent/control.rs]])
//
// **파일 내 레이어 분류:**
//   - Deep: `ThreadManager::new`, `spawn_thread_with_source`, `finalize_thread_spawn`,
//            `fork_thread`, `shutdown_all_threads_bounded`, `build_skills_watcher`,
//            `snapshot_turn_state`, `truncate_before_nth_user_message`, `append_interrupted_boundary`
//   - Medium: 나머지 spawn/resume 래퍼들, `send_op`, `list_agent_subtree_thread_ids`
//   - Shallow: getter 메서드들, `From<usize> for ForkSnapshot`

use crate::SkillsManager;
use crate::agent::AgentControl;
use crate::codex::Codex;
use crate::codex::CodexSpawnArgs;
use crate::codex::CodexSpawnOk;
use crate::codex::INITIAL_SUBMIT_ID;
use crate::codex_thread::CodexThread;
use crate::config::Config;
use crate::file_watcher::FileWatcher;
use crate::mcp::McpManager;
use crate::plugins::PluginsManager;
use crate::rollout::RolloutRecorder;
use crate::rollout::truncation;
use crate::shell_snapshot::ShellSnapshot;
use crate::skills_watcher::SkillsWatcher;
use crate::skills_watcher::SkillsWatcherEvent;
use crate::tasks::interrupted_turn_history_marker;
use codex_analytics::AnalyticsEventsClient;
use codex_app_server_protocol::ThreadHistoryBuilder;
use codex_app_server_protocol::TurnStatus;
use codex_exec_server::EnvironmentManager;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_models_manager::collaboration_mode_presets::CollaborationModesConfig;
use codex_models_manager::manager::ModelsManager;
use codex_models_manager::manager::RefreshStrategy;
use codex_protocol::ThreadId;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
#[cfg(test)]
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::McpServerRefreshConfig;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SessionConfiguredEvent;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::protocol::TurnAbortedEvent;
use codex_protocol::protocol::W3cTraceContext;
use codex_state::DirectionalThreadSpawnEdgeStatus;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::runtime::RuntimeFlavor;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tracing::warn;

const THREAD_CREATED_CHANNEL_CAPACITY: usize = 1024;
/// Test-only override for enabling thread-manager behaviors used by integration
/// tests.
///
/// In production builds this value should remain at its default (`false`) and
/// must not be toggled.
static FORCE_TEST_THREAD_MANAGER_BEHAVIOR: AtomicBool = AtomicBool::new(false);

type CapturedOps = Vec<(ThreadId, Op)>;
type SharedCapturedOps = Arc<std::sync::Mutex<CapturedOps>>;

// 단순 setter — 테스트에서 전역 플래그를 켜고 끄는 유일한 경로
pub(crate) fn set_thread_manager_test_mode_for_tests(enabled: bool) {
    FORCE_TEST_THREAD_MANAGER_BEHAVIOR.store(enabled, Ordering::Relaxed);
}

// 단순 getter — 전역 테스트 플래그를 읽는다
fn should_use_test_thread_manager_behavior() -> bool {
    FORCE_TEST_THREAD_MANAGER_BEHAVIOR.load(Ordering::Relaxed)
}

// ## TempCodexHomeGuard
//
// **What:** 테스트 전용 임시 홈 디렉터리를 스코프가 끝날 때 자동으로 삭제하는 가드다.
//
// **How:**
//   1. `with_models_provider_for_tests` 에서 임시 디렉터리를 만든다.
//   2. 해당 경로를 이 구조체에 넣어 `ThreadManager` 에 묶어둔다.
//   3. `ThreadManager` 가 drop될 때 이 구조체도 drop되고, `remove_dir_all` 이 호출된다.
//
// **Why:** Rust의 RAII 패턴 — 자원 획득과 해제를 같은 타입에 묶어서
//   "잊어버릴 수 없는" 정리 코드를 만든다.
//   교과서에서는 이를 **RAII(Resource Acquisition Is Initialization)** 라 부른다.
//   C++ 에서 온 개념이지만 Rust 에서 가장 자연스럽게 구현된다.
struct TempCodexHomeGuard {
    path: PathBuf,
}

impl Drop for TempCodexHomeGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ## build_skills_watcher
//
// **What:** 파일시스템을 감시해 스킬 파일이 바뀌면 캐시를 자동 무효화하는 감시자를 만든다.
//
// **How:**
//   1. 테스트 환경(단일 스레드 Tokio 런타임) 인지 확인한다.
//      - 맞으면 아무것도 안 하는 noop 감시자를 반환한다. (⚠️ 아래 이유 참고)
//   2. OS 파일시스템 이벤트를 수신하는 `FileWatcher` 를 생성한다.
//   3. `SkillsWatcher` 를 파일 감시자에 연결한다.
//   4. Tokio 백그라운드 태스크를 띄워 `SkillsWatcherEvent::SkillsChanged` 가 올 때마다
//      `skills_manager.clear_cache()` 를 호출한다.
//
// **Macro Role:**
//   - 스킬 핫리로드의 핵심 파이프라인이다.
//   - 이게 없으면 사용자가 스킬 파일을 수정해도 프로세스를 재시작해야 적용된다.
//
// **Why:** 백그라운드 루프를 `handle.spawn(...)` 으로 분리한 이유는
//   감시 이벤트가 언제 올지 예측할 수 없어 메인 스레드를 블록해선 안 되기 때문이다.
//
// **Why Not:**
//   - *폴링 방식*: 일정 주기로 파일 변경을 확인하는 것도 가능하지만,
//     변경이 없을 때도 CPU를 소비하고, 반응 지연이 생긴다.
//   - *동기 파일 감시*: OS 이벤트를 동기적으로 기다리면 메인 스레드가 블록된다.
//
// ⚠️ 테스트에서 noop을 쓰는 이유: `current_thread` 런타임은 스레드가 하나뿐이다.
//    실제 감시자는 내부에서 백그라운드 태스크를 띄우는데, 단일 스레드 런타임에서는
//    이 태스크가 yield를 안 하면 테스트 코드가 영원히 대기에 빠진다(starvation).
fn build_skills_watcher(skills_manager: Arc<SkillsManager>) -> Arc<SkillsWatcher> {
    if should_use_test_thread_manager_behavior()
        && let Ok(handle) = Handle::try_current()
        && handle.runtime_flavor() == RuntimeFlavor::CurrentThread
    {
        // The real watcher spins background tasks that can starve the
        // current-thread test runtime and cause event waits to time out.
        warn!("using noop skills watcher under current-thread test runtime");
        return Arc::new(SkillsWatcher::noop());
    }

    let file_watcher = match FileWatcher::new() {
        Ok(file_watcher) => Arc::new(file_watcher),
        Err(err) => {
            warn!("failed to initialize file watcher: {err}");
            Arc::new(FileWatcher::noop())
        }
    };
    let skills_watcher = Arc::new(SkillsWatcher::new(&file_watcher));

    let mut rx = skills_watcher.subscribe();
    let skills_manager = Arc::clone(&skills_manager);
    if let Ok(handle) = Handle::try_current() {
        handle.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(SkillsWatcherEvent::SkillsChanged { .. }) => {
                        skills_manager.clear_cache();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
    } else {
        warn!("skills watcher listener skipped: no Tokio runtime available");
    }

    skills_watcher
}

/// Represents a newly created Codex thread (formerly called a conversation), including the first event
/// (which is [`EventMsg::SessionConfigured`]).
pub struct NewThread {
    pub thread_id: ThreadId,
    pub thread: Arc<CodexThread>,
    pub session_configured: SessionConfiguredEvent,
}

// TODO(ccunningham): Add an explicit non-interrupting live-turn snapshot once
// core can represent sampling boundaries directly instead of relying on
// whichever items happened to be persisted mid-turn.
//
// Two likely future variants:
// - `TruncateToLastSamplingBoundary` for callers that want a coherent fork from
//   the last stable model boundary without synthesizing an interrupt.
// - `WaitUntilNextSamplingBoundary` (or similar) for callers that prefer to
//   fork after the next sampling boundary rather than interrupting immediately.

// ## ForkSnapshot
//
// **What:** 스레드를 포크할 때 "어디까지의 히스토리를 가져갈 것인가"를 결정하는 전략 열거형이다.
//
// **How:** `fork_thread` 에 넘겨주면, 두 가지 방식 중 하나로 히스토리를 자른다:
//   - `TruncateBeforeNthUserMessage(n)`: n번째 유저 메시지 *앞*에서 자른다.
//     게임의 세이브 포인트로 돌아가는 것과 같다.
//   - `Interrupted`: 현재 진행 중인 턴을 중단된 것처럼 처리하고 포크한다.
//     "지금 이 순간 Ctrl+C를 눌렀다면 어떤 기록이 남을까?"를 시뮬레이션한다.
//
// **Macro Role:** `fork_thread` 의 핵심 파라미터. 이 열거형 없이는 포크 시 히스토리 전략을
//   외부에서 타입 안전하게 전달할 방법이 없다.
//
// **Why:** 두 변형을 같은 타입으로 묶은 이유는 호출 지점에서 `match` 하나로
//   모든 포크 전략을 처리하기 위해서다 — 분기를 호출자로 올리지 않는 것.
//
// **Why Not:**
//   - *bool 파라미터*: `fork_thread(n, is_interrupted: bool)` 처럼 쓰면
//     호출 지점에서 `true`/`false` 의 의미를 알 수 없다.
//   - *별도 함수 두 개*: 중복 로직이 생기고, 미래 변형 추가 시 API가 폭발한다.
//
// 📌 이런 "전략을 열거형으로 표현" 패턴은 교과서의 **Strategy 패턴**을
//    함수형으로 구현한 것이다. 인터페이스 대신 enum을 쓰는 Rust다운 방식이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkSnapshot {
    /// Fork a committed prefix ending strictly before the nth user message.
    ///
    /// When `n` is within range, this cuts before that 0-based user-message
    /// boundary. When `n` is out of range and the source thread is currently
    /// mid-turn, this instead cuts before the active turn's opening boundary
    /// so the fork drops the unfinished turn suffix. When `n` is out of range
    /// and the source thread is already at a turn boundary, this returns the
    /// full committed history unchanged.
    TruncateBeforeNthUserMessage(usize),

    /// Fork the current persisted history as if the source thread had been
    /// interrupted now.
    ///
    /// If the persisted snapshot ends mid-turn, this appends the same
    /// `<turn_aborted>` marker produced by a real interrupt. If the snapshot is
    /// already at a turn boundary, this returns the current persisted history
    /// unchanged.
    Interrupted,
}

/// Preserve legacy `fork_thread(usize, ...)` callsites by mapping them to the
/// existing truncate-before-nth-user-message snapshot mode.
// 단순 변환 — usize를 받던 구형 API와의 하위 호환성을 유지한다
impl From<usize> for ForkSnapshot {
    fn from(value: usize) -> Self {
        Self::TruncateBeforeNthUserMessage(value)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ThreadShutdownReport {
    pub completed: Vec<ThreadId>,
    pub submit_failed: Vec<ThreadId>,
    pub timed_out: Vec<ThreadId>,
}

enum ShutdownOutcome {
    Complete,
    SubmitFailed,
    TimedOut,
}

// ## ThreadManager
//
// **What:** 살아있는 모든 AI 스레드를 생성하고 관리하는 최상위 관리자 구조체다.
//   비유하면 **콜센터 교환기** 와 같다. 상담원(CodexThread)을 고용하고, 연결을 중개하고,
//   자리를 배정하지만, 실제 통화 내용은 몰라도 된다.
//
// **How:**
//   - 내부 상태를 `ThreadManagerState` 에 위임하고, 이를 `Arc` 로 공유한다.
//   - `Arc` 를 분리한 이유: `AgentControl` 이 `Weak<ThreadManagerState>` 를 들고
//     순환 참조 없이 역참조해야 하기 때문이다.
//     `Arc<ThreadManager>` 를 `Weak` 로 만들면 `ThreadManager` 의 공개 메서드가
//     사라지므로 내부 상태만 `Arc` 로 분리한다.
//
// **Macro Role:**
//   - `codex-rs/app-server` 의 모든 스레드 생성 요청은 여기서 시작된다.
//   - `ThreadManager` 가 없으면 단 한 개의 AI 대화도 시작할 수 없다.
//
// **Why:** 공개 API(`ThreadManager`)와 공유 상태(`ThreadManagerState`)를 분리한 이유는
//   **캡슐화** 와 **약한 참조** 를 동시에 달성하기 위해서다.
//   `ThreadManager` 는 외부 호출자가 갖는 핸들이고,
//   `ThreadManagerState` 는 내부 컴포넌트들이 공유하는 데이터다.
//
// **Why Not:**
//   - *단일 구조체*: `ThreadManager` 하나에 모두 넣으면 `AgentControl` 이
//     `Arc<ThreadManager>` 를 들어야 한다. `ThreadManager` 를 drop할 때
//     `AgentControl` 이 살아있으면 drop이 안 되는 순환 문제가 생긴다.
//
// **Lesson:** "공개 핸들 + 공유 내부 상태" 분리는 Rust에서 순환 참조를 피하는
//   표준 관용구다. `Arc<Outer>` / `Weak<Inner>` 패턴을 훔쳐가서 유사한 상황에 써라.

/// [`ThreadManager`] is responsible for creating threads and maintaining
/// them in memory.
pub struct ThreadManager {
    state: Arc<ThreadManagerState>,
    _test_codex_home_guard: Option<TempCodexHomeGuard>,
}

/// Shared, `Arc`-owned state for [`ThreadManager`]. This `Arc` is required to have a single
/// `Arc` reference that can be downgraded to by `AgentControl` while preventing every single
/// function to require an `Arc<&Self>`.
pub(crate) struct ThreadManagerState {
    threads: Arc<RwLock<HashMap<ThreadId, Arc<CodexThread>>>>,
    thread_created_tx: broadcast::Sender<ThreadId>,
    auth_manager: Arc<AuthManager>,
    models_manager: Arc<ModelsManager>,
    environment_manager: Arc<EnvironmentManager>,
    skills_manager: Arc<SkillsManager>,
    plugins_manager: Arc<PluginsManager>,
    mcp_manager: Arc<McpManager>,
    skills_watcher: Arc<SkillsWatcher>,
    session_source: SessionSource,
    analytics_events_client: Option<AnalyticsEventsClient>,
    // Captures submitted ops for testing purpose when test mode is enabled.
    ops_log: Option<SharedCapturedOps>,
}

impl ThreadManager {
    // ## ThreadManager::new
    //
    // **What:** 프로덕션 환경에서 쓸 `ThreadManager` 를 초기화한다.
    //   모든 하위 매니저들(모델·플러그인·MCP·스킬)을 한 자리에서 생성한다.
    //
    // **How:**
    //   1. Config 에서 OpenAI 프로바이더 정보를 꺼낸다 (없으면 기본값 생성).
    //   2. 스레드 생성 알림용 broadcast 채널을 만든다.
    //   3. PluginsManager → McpManager → SkillsManager → SkillsWatcher 순서로 생성.
    //      💡 순서가 중요하다: McpManager 는 PluginsManager 에 의존하고,
    //         SkillsWatcher 는 SkillsManager 에 의존한다.
    //   4. 모든 상태를 `ThreadManagerState` 에 묶어 `Arc` 로 감싼다.
    //
    // **Macro Role:** 애플리케이션 시작 시 딱 한 번 호출되는 루트 초기화 함수.
    //   여기서 생성된 매니저들은 모든 스레드가 공유한다.
    //
    // **Why:** 매니저들을 `Arc` 로 공유하는 이유는 스레드마다 복사본을 만들면
    //   메모리가 낭비되고, 상태 동기화가 불가능해지기 때문이다.
    //
    // **Why Not:**
    //   - *의존성 주입 프레임워크*: Java/Kotlin 처럼 DI 컨테이너를 쓸 수도 있지만,
    //     Rust 에는 성숙한 런타임 DI 프레임워크가 없고, 이 정도 복잡도면 직접 구성이 낫다.
    //   - *lazy static 싱글턴*: 전역 변수로 관리하면 테스트 격리가 깨진다.
    pub fn new(
        config: &Config,
        auth_manager: Arc<AuthManager>,
        session_source: SessionSource,
        collaboration_modes_config: CollaborationModesConfig,
        environment_manager: Arc<EnvironmentManager>,
        analytics_events_client: Option<AnalyticsEventsClient>,
    ) -> Self {
        let codex_home = config.codex_home.clone();
        let restriction_product = session_source.restriction_product();
        let openai_models_provider = config
            .model_providers
            .get(OPENAI_PROVIDER_ID)
            .cloned()
            .unwrap_or_else(|| ModelProviderInfo::create_openai_provider(/*base_url*/ None));
        let (thread_created_tx, _) = broadcast::channel(THREAD_CREATED_CHANNEL_CAPACITY);
        let plugins_manager = Arc::new(PluginsManager::new_with_restriction_product(
            codex_home.clone(),
            restriction_product,
        ));
        let mcp_manager = Arc::new(McpManager::new(Arc::clone(&plugins_manager)));
        let skills_manager = Arc::new(SkillsManager::new_with_restriction_product(
            codex_home.clone(),
            config.bundled_skills_enabled(),
            restriction_product,
        ));
        let skills_watcher = build_skills_watcher(Arc::clone(&skills_manager));
        Self {
            state: Arc::new(ThreadManagerState {
                threads: Arc::new(RwLock::new(HashMap::new())),
                thread_created_tx,
                models_manager: Arc::new(ModelsManager::new_with_provider(
                    codex_home,
                    auth_manager.clone(),
                    config.model_catalog.clone(),
                    collaboration_modes_config,
                    openai_models_provider,
                )),
                environment_manager,
                skills_manager,
                plugins_manager,
                mcp_manager,
                skills_watcher,
                auth_manager,
                session_source,
                analytics_events_client,
                ops_log: should_use_test_thread_manager_behavior()
                    .then(|| Arc::new(std::sync::Mutex::new(Vec::new()))),
            }),
            _test_codex_home_guard: None,
        }
    }

    // ## with_models_provider_for_tests
    //
    // **What:** 통합 테스트 전용 생성자. 실제 인증 없이 특정 모델 프로바이더만 주입한다.
    //   임시 홈 디렉터리를 만들고, 테스트 종료 시 자동 삭제를 보장한다.
    //
    // **How:**
    //   1. 테스트 모드 플래그를 켠다.
    //   2. 고유 UUID를 가진 임시 디렉터리를 생성한다.
    //   3. `with_models_provider_and_home_for_tests` 에 위임한다.
    //   4. `TempCodexHomeGuard` 를 `_test_codex_home_guard` 에 묶어 스코프 종료 시 삭제.
    //
    // **Why:** 테스트마다 격리된 홈 디렉터리가 필요한 이유는 실제 `~/.codex` 를 오염시키지
    //   않기 위해서다. UUID 로 고유성을 보장해 병렬 테스트 간 충돌을 막는다.

    /// Construct with a dummy AuthManager containing the provided CodexAuth.
    /// Used for integration tests: should not be used by ordinary business logic.
    pub(crate) fn with_models_provider_for_tests(
        auth: CodexAuth,
        provider: ModelProviderInfo,
    ) -> Self {
        set_thread_manager_test_mode_for_tests(/*enabled*/ true);
        let codex_home = std::env::temp_dir().join(format!(
            "codex-thread-manager-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&codex_home)
            .unwrap_or_else(|err| panic!("temp codex home dir create failed: {err}"));
        let mut manager = Self::with_models_provider_and_home_for_tests(
            auth,
            provider,
            codex_home.clone(),
            Arc::new(EnvironmentManager::new(/*exec_server_url*/ None)),
        );
        manager._test_codex_home_guard = Some(TempCodexHomeGuard { path: codex_home });
        manager
    }

    /// Construct with a dummy AuthManager containing the provided CodexAuth and codex home.
    /// Used for integration tests: should not be used by ordinary business logic.
    pub(crate) fn with_models_provider_and_home_for_tests(
        auth: CodexAuth,
        provider: ModelProviderInfo,
        codex_home: PathBuf,
        environment_manager: Arc<EnvironmentManager>,
    ) -> Self {
        set_thread_manager_test_mode_for_tests(/*enabled*/ true);
        let auth_manager = AuthManager::from_auth_for_testing(auth);
        let (thread_created_tx, _) = broadcast::channel(THREAD_CREATED_CHANNEL_CAPACITY);
        let restriction_product = SessionSource::Exec.restriction_product();
        let plugins_manager = Arc::new(PluginsManager::new_with_restriction_product(
            codex_home.clone(),
            restriction_product,
        ));
        let mcp_manager = Arc::new(McpManager::new(Arc::clone(&plugins_manager)));
        let skills_manager = Arc::new(SkillsManager::new_with_restriction_product(
            codex_home.clone(),
            /*bundled_skills_enabled*/ true,
            restriction_product,
        ));
        let skills_watcher = build_skills_watcher(Arc::clone(&skills_manager));
        Self {
            state: Arc::new(ThreadManagerState {
                threads: Arc::new(RwLock::new(HashMap::new())),
                thread_created_tx,
                models_manager: Arc::new(ModelsManager::with_provider_for_tests(
                    codex_home,
                    auth_manager.clone(),
                    provider,
                )),
                environment_manager,
                skills_manager,
                plugins_manager,
                mcp_manager,
                skills_watcher,
                auth_manager,
                session_source: SessionSource::Exec,
                analytics_events_client: None,
                ops_log: should_use_test_thread_manager_behavior()
                    .then(|| Arc::new(std::sync::Mutex::new(Vec::new()))),
            }),
            _test_codex_home_guard: None,
        }
    }

    // 단순 getter — SessionSource 복제 반환
    pub fn session_source(&self) -> SessionSource {
        self.state.session_source.clone()
    }

    // 단순 getter — AuthManager Arc 복제 반환
    pub fn auth_manager(&self) -> Arc<AuthManager> {
        self.state.auth_manager.clone()
    }

    // 단순 getter — SkillsManager Arc 복제 반환
    pub fn skills_manager(&self) -> Arc<SkillsManager> {
        self.state.skills_manager.clone()
    }

    // 단순 getter — PluginsManager Arc 복제 반환
    pub fn plugins_manager(&self) -> Arc<PluginsManager> {
        self.state.plugins_manager.clone()
    }

    // 단순 getter — McpManager Arc 복제 반환
    pub fn mcp_manager(&self) -> Arc<McpManager> {
        self.state.mcp_manager.clone()
    }

    // 단순 getter — ModelsManager Arc 복제 반환
    pub fn get_models_manager(&self) -> Arc<ModelsManager> {
        self.state.models_manager.clone()
    }

    // 단순 위임 — models_manager 에 refresh 전략과 함께 위임
    pub async fn list_models(&self, refresh_strategy: RefreshStrategy) -> Vec<ModelPreset> {
        self.state
            .models_manager
            .list_models(refresh_strategy)
            .await
    }

    // 단순 위임 — models_manager 의 협업 모드 목록 반환
    pub fn list_collaboration_modes(&self) -> Vec<CollaborationModeMask> {
        self.state.models_manager.list_collaboration_modes()
    }

    // 단순 위임 — 살아있는 스레드 ID 목록 반환
    pub async fn list_thread_ids(&self) -> Vec<ThreadId> {
        self.state.list_thread_ids().await
    }

    // ## refresh_mcp_servers
    //
    // **What:** 살아있는 모든 스레드에 MCP 서버 새로고침 명령을 브로드캐스트한다.
    //
    // **How:** 현재 스레드 맵을 스냅샷으로 복사 후 각 스레드에 `Op::RefreshMcpServers` 를 전송.
    //   실패 시 warn 로그만 남기고 계속 진행 (best-effort).
    //
    // **Why:** 읽기 잠금을 잠깐 잡아 스냅샷을 만든 뒤 잠금을 풀고 작업하는 이유는
    //   `submit` 이 await 를 포함할 수 있어 잠금을 들고 기다리면 다른 작업이 막히기 때문이다.
    pub async fn refresh_mcp_servers(&self, refresh_config: McpServerRefreshConfig) {
        let threads = self
            .state
            .threads
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for thread in threads {
            if let Err(err) = thread
                .submit(Op::RefreshMcpServers {
                    config: refresh_config.clone(),
                })
                .await
            {
                warn!("failed to request MCP server refresh: {err}");
            }
        }
    }

    // 단순 위임 — 스레드 생성 이벤트 구독자 반환
    pub fn subscribe_thread_created(&self) -> broadcast::Receiver<ThreadId> {
        self.state.thread_created_tx.subscribe()
    }

    // 단순 위임 — ID 로 스레드를 찾거나 ThreadNotFound 반환
    pub async fn get_thread(&self, thread_id: ThreadId) -> CodexResult<Arc<CodexThread>> {
        self.state.get_thread(thread_id).await
    }

    // ## list_agent_subtree_thread_ids
    //
    // **What:** 주어진 스레드와 그 스레드가 생성한 모든 자손 스레드의 ID 목록을 반환한다.
    //   비유하면 가족 관계도에서 특정 인물과 그 모든 자녀·손녀를 찾는 것과 같다.
    //
    // **How:**
    //   1. 시작 스레드를 목록에 추가하고 방문 집합에 기록.
    //   2. DB의 스폰 엣지(Open/Closed 두 상태 모두) 에서 자손 ID를 가져온다.
    //   3. 메모리의 라이브 에이전트 서브트리에서도 자손 ID를 가져온다.
    //   4. `HashSet` 으로 중복을 제거하면서 결과 목록에 추가.
    //
    // **Why:** 두 소스(DB + 메모리)를 모두 확인하는 이유는
    //   DB에는 이미 완료된 자손 정보가, 메모리에는 현재 실행 중인 자손 정보가 있기 때문이다.
    //   어느 하나만 보면 전체 그림을 못 얻는다.

    /// List `thread_id` plus all known descendants in its spawn subtree.
    pub async fn list_agent_subtree_thread_ids(
        &self,
        thread_id: ThreadId,
    ) -> CodexResult<Vec<ThreadId>> {
        let thread = self.state.get_thread(thread_id).await?;

        let mut subtree_thread_ids = Vec::new();
        let mut seen_thread_ids = HashSet::new();
        subtree_thread_ids.push(thread_id);
        seen_thread_ids.insert(thread_id);

        if let Some(state_db_ctx) = thread.state_db() {
            for status in [
                DirectionalThreadSpawnEdgeStatus::Open,
                DirectionalThreadSpawnEdgeStatus::Closed,
            ] {
                for descendant_id in state_db_ctx
                    .list_thread_spawn_descendants_with_status(thread_id, status)
                    .await
                    .map_err(|err| {
                        CodexErr::Fatal(format!("failed to load thread-spawn descendants: {err}"))
                    })?
                {
                    if seen_thread_ids.insert(descendant_id) {
                        subtree_thread_ids.push(descendant_id);
                    }
                }
            }
        }

        for descendant_id in thread
            .codex
            .session
            .services
            .agent_control
            .list_live_agent_subtree_thread_ids(thread_id)
            .await?
        {
            if seen_thread_ids.insert(descendant_id) {
                subtree_thread_ids.push(descendant_id);
            }
        }

        Ok(subtree_thread_ids)
    }

    // ## start_thread / start_thread_with_tools / start_thread_with_tools_and_service_name
    //
    // **What:** 새 스레드를 시작하는 공개 API 계층이다. 세 함수는 선택적 파라미터를
    //   점진적으로 추가하는 **퍼사드(facade) 래퍼 체인**이다.
    //
    // **How:**
    //   `start_thread` → `start_thread_with_tools` → `start_thread_with_tools_and_service_name`
    //   → `state.spawn_thread`
    //   각 단계에서 기본값을 채워 다음 단계에 위임한다.
    //
    // **Why Box::pin():** 💡 이게 핵심 트릭이다.
    //   Rust 의 `async fn` 은 컴파일 타임에 미래(Future)의 크기를 알아야 한다.
    //   이 함수들은 서로를 호출하고, 그 안에서 또 `spawn_thread_with_source` 를 호출하는데,
    //   이 호출 체인이 컴파일러 입장에서 재귀처럼 보일 수 있어 크기 추론에 실패한다.
    //   `Box::pin(async { ... })` 으로 힙에 올리면 크기를 알 수 있게 된다(포인터 크기만).
    //
    // **Why Not:**
    //   - *단일 함수 + 옵션 구조체*: `StartThreadOptions { tools: ..., service_name: ... }` 처럼
    //     쓰면 호출 지점이 단순해지지만, 필드 추가 시 기존 코드가 컴파일 오류가 날 수 있다.
    //   - *매크로*: 과잉 추상화다.

    pub async fn start_thread(&self, config: Config) -> CodexResult<NewThread> {
        // Box delegated thread-spawn futures so these convenience wrappers do
        // not inline the full spawn path into every caller's async state.
        Box::pin(self.start_thread_with_tools(
            config,
            Vec::new(),
            /*persist_extended_history*/ false,
        ))
        .await
    }

    pub async fn start_thread_with_tools(
        &self,
        config: Config,
        dynamic_tools: Vec<codex_protocol::dynamic_tools::DynamicToolSpec>,
        persist_extended_history: bool,
    ) -> CodexResult<NewThread> {
        Box::pin(self.start_thread_with_tools_and_service_name(
            config,
            InitialHistory::New,
            dynamic_tools,
            persist_extended_history,
            /*metrics_service_name*/ None,
            /*parent_trace*/ None,
        ))
        .await
    }

    pub async fn start_thread_with_tools_and_service_name(
        &self,
        config: Config,
        initial_history: InitialHistory,
        dynamic_tools: Vec<codex_protocol::dynamic_tools::DynamicToolSpec>,
        persist_extended_history: bool,
        metrics_service_name: Option<String>,
        parent_trace: Option<W3cTraceContext>,
    ) -> CodexResult<NewThread> {
        Box::pin(self.state.spawn_thread(
            config,
            initial_history,
            Arc::clone(&self.state.auth_manager),
            self.agent_control(),
            dynamic_tools,
            persist_extended_history,
            metrics_service_name,
            parent_trace,
            /*user_shell_override*/ None,
        ))
        .await
    }

    // ## resume_thread_from_rollout
    //
    // **What:** 저장된 rollout 파일에서 이전 대화 기록을 불러와 스레드를 재개한다.
    //   비유하면 중단된 책의 책갈피 위치에서 다시 읽기 시작하는 것과 같다.
    //
    // **How:** rollout 파일을 읽어 `InitialHistory` 로 변환한 뒤 `resume_thread_with_history` 에 위임.
    //
    // **Why:** rollout 에서 히스토리를 로드하는 로직을 별도 함수로 뺀 이유는
    //   "어떻게 히스토리를 얻는가"와 "어떻게 스레드를 시작하는가"를 분리하기 위해서다.

    pub async fn resume_thread_from_rollout(
        &self,
        config: Config,
        rollout_path: PathBuf,
        auth_manager: Arc<AuthManager>,
        parent_trace: Option<W3cTraceContext>,
    ) -> CodexResult<NewThread> {
        let initial_history = RolloutRecorder::get_rollout_history(&rollout_path).await?;
        Box::pin(self.resume_thread_with_history(
            config,
            initial_history,
            auth_manager,
            /*persist_extended_history*/ false,
            parent_trace,
        ))
        .await
    }

    // 단순 위임 — 주어진 히스토리로 새 스레드를 스폰
    pub async fn resume_thread_with_history(
        &self,
        config: Config,
        initial_history: InitialHistory,
        auth_manager: Arc<AuthManager>,
        persist_extended_history: bool,
        parent_trace: Option<W3cTraceContext>,
    ) -> CodexResult<NewThread> {
        Box::pin(self.state.spawn_thread(
            config,
            initial_history,
            auth_manager,
            self.agent_control(),
            Vec::new(),
            persist_extended_history,
            /*metrics_service_name*/ None,
            parent_trace,
            /*user_shell_override*/ None,
        ))
        .await
    }

    // 테스트 전용 — 특정 셸을 강제 주입해 스레드를 시작
    pub(crate) async fn start_thread_with_user_shell_override_for_tests(
        &self,
        config: Config,
        user_shell_override: crate::shell::Shell,
    ) -> CodexResult<NewThread> {
        Box::pin(self.state.spawn_thread(
            config,
            InitialHistory::New,
            Arc::clone(&self.state.auth_manager),
            self.agent_control(),
            Vec::new(),
            /*persist_extended_history*/ false,
            /*metrics_service_name*/ None,
            /*parent_trace*/ None,
            /*user_shell_override*/ Some(user_shell_override),
        ))
        .await
    }

    // 테스트 전용 — rollout 에서 불러오되 특정 셸을 강제 주입
    pub(crate) async fn resume_thread_from_rollout_with_user_shell_override_for_tests(
        &self,
        config: Config,
        rollout_path: PathBuf,
        auth_manager: Arc<AuthManager>,
        user_shell_override: crate::shell::Shell,
    ) -> CodexResult<NewThread> {
        let initial_history = RolloutRecorder::get_rollout_history(&rollout_path).await?;
        Box::pin(self.state.spawn_thread(
            config,
            initial_history,
            auth_manager,
            self.agent_control(),
            Vec::new(),
            /*persist_extended_history*/ false,
            /*metrics_service_name*/ None,
            /*parent_trace*/ None,
            /*user_shell_override*/ Some(user_shell_override),
        ))
        .await
    }

    /// Removes the thread from the manager's internal map, though the thread is stored
    /// as `Arc<CodexThread>`, it is possible that other references to it exist elsewhere.
    /// Returns the thread if the thread was found and removed.
    pub async fn remove_thread(&self, thread_id: &ThreadId) -> Option<Arc<CodexThread>> {
        self.state.threads.write().await.remove(thread_id)
    }

    // ## shutdown_all_threads_bounded
    //
    // **What:** 타임아웃 안에 살아있는 모든 스레드를 동시에 종료하고
    //   완료·실패·타임아웃 결과를 분류해 보고한다.
    //
    // **How:**
    //   1. 읽기 잠금으로 현재 스레드 목록을 스냅샷.
    //   2. 각 스레드에 대해 `shutdown_and_wait` + `tokio::time::timeout` 을 조합한 Future 를 만든다.
    //   3. `FuturesUnordered` 에 모아 동시 실행. 💡 완료되는 순서대로 결과를 수확.
    //   4. 완료된 스레드만 맵에서 제거. 실패·타임아웃 된 것은 그대로 둔다 (호출자가 재시도 가능).
    //   5. 결과 목록을 정렬해 결정론적 순서 보장 (테스트 비교 용이).
    //
    // **Macro Role:** 앱 서버 종료 시 호출. 이 함수 없이는 종료 시 스레드가 유령처럼 남는다.
    //
    // **Why FuturesUnordered:** 💡 이 부분이 핵심이다.
    //   `join_all` 을 쓰면 모든 Future 가 완료될 때까지 가장 느린 것을 기다린다.
    //   `FuturesUnordered` 는 완료된 것부터 즉시 처리하고, 각 Future 에 개별 타임아웃을 적용할 수 있다.
    //   10개 스레드 중 9개가 0.1초에 끝나고 1개가 5초 걸려도, 9개는 즉시 정리된다.
    //
    // **Why Not:**
    //   - *순차 종료*: `for thread in threads { thread.shutdown().await }` 는 한 스레드가
    //     블록되면 나머지가 모두 기다린다. 전체 종료 시간이 최악의 경우 N × timeout.
    //   - *tokio::spawn 후 join*: 가능하지만 FuturesUnordered 가 더 직접적이고 취소가 쉽다.
    //
    // 📌 배울 점: "독립적인 비동기 작업 N개를 동시에 실행하고 완료 순서대로 처리"할 때는
    //   `FuturesUnordered` 를 꺼내라.

    /// Tries to shut down all tracked threads concurrently within the provided timeout.
    /// Threads that complete shutdown are removed from the manager; incomplete shutdowns
    /// remain tracked so callers can retry or inspect them later.
    pub async fn shutdown_all_threads_bounded(&self, timeout: Duration) -> ThreadShutdownReport {
        let threads = {
            let threads = self.state.threads.read().await;
            threads
                .iter()
                .map(|(thread_id, thread)| (*thread_id, Arc::clone(thread)))
                .collect::<Vec<_>>()
        };

        let mut shutdowns = threads
            .into_iter()
            .map(|(thread_id, thread)| async move {
                let outcome = match tokio::time::timeout(timeout, thread.shutdown_and_wait()).await
                {
                    Ok(Ok(())) => ShutdownOutcome::Complete,
                    Ok(Err(_)) => ShutdownOutcome::SubmitFailed,
                    Err(_) => ShutdownOutcome::TimedOut,
                };
                (thread_id, outcome)
            })
            .collect::<FuturesUnordered<_>>();
        let mut report = ThreadShutdownReport::default();

        while let Some((thread_id, outcome)) = shutdowns.next().await {
            match outcome {
                ShutdownOutcome::Complete => report.completed.push(thread_id),
                ShutdownOutcome::SubmitFailed => report.submit_failed.push(thread_id),
                ShutdownOutcome::TimedOut => report.timed_out.push(thread_id),
            }
        }

        let mut tracked_threads = self.state.threads.write().await;
        for thread_id in &report.completed {
            tracked_threads.remove(thread_id);
        }

        report
            .completed
            .sort_by_key(std::string::ToString::to_string);
        report
            .submit_failed
            .sort_by_key(std::string::ToString::to_string);
        report
            .timed_out
            .sort_by_key(std::string::ToString::to_string);
        report
    }

    // ## fork_thread
    //
    // **What:** 기존 스레드의 히스토리를 특정 지점까지 잘라 새 스레드를 만든다.
    //   비유하면 **게임 세이브 파일을 특정 체크포인트에서 복사**해 새 플레이스루를 시작하는 것.
    //
    // **How:**
    //   1. rollout 파일에서 전체 히스토리를 로드한다.
    //   2. `snapshot_turn_state` 로 히스토리가 턴 중간에 있는지 확인한다.
    //   3. `ForkSnapshot` 에 따라 히스토리를 가공한다:
    //      - `TruncateBeforeNthUserMessage(n)`: n번째 유저 메시지 앞에서 자른다.
    //      - `Interrupted`: 진행 중 턴이면 `TurnAborted` 마커를 붙여 중단된 것처럼 만든다.
    //   4. 가공된 히스토리로 새 스레드를 스폰한다.
    //
    // **Macro Role:** UI 에서 "이 시점부터 다시 시작" 기능의 핵심 구현체.
    //   포크 없이는 대화 분기가 불가능하다.
    //
    // **Why:** 히스토리 조작을 이 함수 안에서 끝내는 이유는
    //   `spawn_thread_with_source` 가 히스토리 형태에 대해 알 필요 없게 하기 위해서다.
    //   관심사 분리: "어떤 히스토리로?"는 여기서, "어떻게 스폰하는가?"는 spawn 에서.
    //
    // **Why Not:**
    //   - *클라이언트 측에서 히스토리 자르기*: 외부 호출자가 히스토리를 직접 가공하면
    //     `TurnAborted` 마커 같은 내부 구현 세부사항을 알아야 한다. 캡슐화가 깨진다.

    /// Fork an existing thread by snapshotting rollout history according to
    /// `snapshot` and starting a new thread with identical configuration
    /// (unless overridden by the caller's `config`). The new thread will have
    /// a fresh id.
    pub async fn fork_thread<S>(
        &self,
        snapshot: S,
        config: Config,
        path: PathBuf,
        persist_extended_history: bool,
        parent_trace: Option<W3cTraceContext>,
    ) -> CodexResult<NewThread>
    where
        S: Into<ForkSnapshot>,
    {
        let snapshot = snapshot.into();
        let history = RolloutRecorder::get_rollout_history(&path).await?;
        let snapshot_state = snapshot_turn_state(&history);
        let history = match snapshot {
            ForkSnapshot::TruncateBeforeNthUserMessage(nth_user_message) => {
                truncate_before_nth_user_message(history, nth_user_message, &snapshot_state)
            }
            ForkSnapshot::Interrupted => {
                let history = match history {
                    InitialHistory::New => InitialHistory::New,
                    InitialHistory::Cleared => InitialHistory::Cleared,
                    InitialHistory::Forked(history) => InitialHistory::Forked(history),
                    InitialHistory::Resumed(resumed) => InitialHistory::Forked(resumed.history),
                };
                if snapshot_state.ends_mid_turn {
                    append_interrupted_boundary(history, snapshot_state.active_turn_id)
                } else {
                    history
                }
            }
        };
        Box::pin(self.state.spawn_thread(
            config,
            history,
            Arc::clone(&self.state.auth_manager),
            self.agent_control(),
            Vec::new(),
            persist_extended_history,
            /*metrics_service_name*/ None,
            parent_trace,
            /*user_shell_override*/ None,
        ))
        .await
    }

    // ## agent_control
    //
    // **What:** `AgentControl` 을 만들어 반환한다. AgentControl 은 에이전트 서브태스크가
    //   ThreadManager 에 역방향으로 접근할 수 있는 약한 참조 핸들이다.
    //
    // **Why Weak:** `Arc::downgrade` 로 약한 참조를 만드는 이유는 순환 참조를 막기 위해서다.
    //   `ThreadManager` → `CodexThread` → `AgentControl` → (back to ThreadManager)
    //   이 체인에서 `AgentControl` 이 `Arc` 를 들면 순환 참조가 생겨 메모리가 해제되지 않는다.
    //   `Weak` 은 참조 카운트에 포함되지 않으므로 순환이 깨진다.
    //
    // 💡 배울 점: "부모가 자식을 `Arc` 로, 자식이 부모를 `Weak` 로" 패턴은
    //   트리 구조에서 순환 참조를 피하는 Rust 표준 관용구다.
    pub(crate) fn agent_control(&self) -> AgentControl {
        AgentControl::new(Arc::downgrade(&self.state))
    }

    // 테스트 전용 — ops_log 에 기록된 제출 Op 목록을 반환
    #[cfg(test)]
    pub(crate) fn captured_ops(&self) -> Vec<(ThreadId, Op)> {
        self.state
            .ops_log
            .as_ref()
            .and_then(|ops_log| ops_log.lock().ok().map(|log| log.clone()))
            .unwrap_or_default()
    }
}

impl ThreadManagerState {
    // 단순 쿼리 — 현재 살아있는 스레드 ID 목록 반환
    pub(crate) async fn list_thread_ids(&self) -> Vec<ThreadId> {
        self.threads.read().await.keys().copied().collect()
    }

    /// Fetch a thread by ID or return ThreadNotFound.
    // 단순 조회 — HashMap 에서 스레드를 찾거나 ThreadNotFound 오류 반환
    pub(crate) async fn get_thread(&self, thread_id: ThreadId) -> CodexResult<Arc<CodexThread>> {
        let threads = self.threads.read().await;
        threads
            .get(&thread_id)
            .cloned()
            .ok_or_else(|| CodexErr::ThreadNotFound(thread_id))
    }

    // ## send_op
    //
    // **What:** 특정 스레드에 Op(명령)을 전송하고, 테스트 모드라면 ops_log 에도 기록한다.
    //
    // **How:**
    //   1. `get_thread` 로 스레드를 찾는다.
    //   2. 테스트 모드면 `ops_log` 에 `(thread_id, op.clone())` 을 추가한다.
    //   3. 스레드에 `submit(op)` 을 호출한다.
    //
    // **Why ops_log:** 테스트에서 어떤 Op 이 어느 스레드에 전송됐는지 사후 검증할 수 있게 한다.
    //   실제 AI 엔진 없이 "어떤 명령이 어디로 갔는가"를 테스트하는 패턴이다.

    /// Send an operation to a thread by ID.
    pub(crate) async fn send_op(&self, thread_id: ThreadId, op: Op) -> CodexResult<String> {
        let thread = self.get_thread(thread_id).await?;
        if let Some(ops_log) = &self.ops_log
            && let Ok(mut log) = ops_log.lock()
        {
            log.push((thread_id, op.clone()));
        }
        thread.submit(op).await
    }

    #[cfg(test)]
    /// Append a prebuilt message to a thread by ID outside the normal user-input path.
    pub(crate) async fn append_message(
        &self,
        thread_id: ThreadId,
        message: ResponseItem,
    ) -> CodexResult<String> {
        let thread = self.get_thread(thread_id).await?;
        thread.append_message(message).await
    }

    /// Remove a thread from the manager by ID, returning it when present.
    // 단순 삭제 — HashMap 에서 스레드 제거 후 반환
    pub(crate) async fn remove_thread(&self, thread_id: &ThreadId) -> Option<Arc<CodexThread>> {
        self.threads.write().await.remove(thread_id)
    }

    // ## spawn_new_thread / spawn_new_thread_with_source
    //
    // **What:** 빈 히스토리로 새 스레드를 시작하는 내부 헬퍼 래퍼들.
    //   `AgentControl` 같은 내부 컴포넌트가 새 서브스레드를 생성할 때 쓴다.
    //
    // **How:** `spawn_thread_with_source` 에 기본값을 채워 위임.
    //   `Box::pin` 으로 재귀적 Future 크기 문제를 해결.

    /// Spawn a new thread with no history using a provided config.
    pub(crate) async fn spawn_new_thread(
        &self,
        config: Config,
        agent_control: AgentControl,
    ) -> CodexResult<NewThread> {
        Box::pin(self.spawn_new_thread_with_source(
            config,
            agent_control,
            self.session_source.clone(),
            /*persist_extended_history*/ false,
            /*metrics_service_name*/ None,
            /*inherited_shell_snapshot*/ None,
            /*inherited_exec_policy*/ None,
        ))
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn spawn_new_thread_with_source(
        &self,
        config: Config,
        agent_control: AgentControl,
        session_source: SessionSource,
        persist_extended_history: bool,
        metrics_service_name: Option<String>,
        inherited_shell_snapshot: Option<Arc<ShellSnapshot>>,
        inherited_exec_policy: Option<Arc<crate::exec_policy::ExecPolicyManager>>,
    ) -> CodexResult<NewThread> {
        Box::pin(self.spawn_thread_with_source(
            config,
            InitialHistory::New,
            Arc::clone(&self.auth_manager),
            agent_control,
            session_source,
            Vec::new(),
            persist_extended_history,
            metrics_service_name,
            inherited_shell_snapshot,
            inherited_exec_policy,
            /*parent_trace*/ None,
            /*user_shell_override*/ None,
        ))
        .await
    }

    // 단순 위임 — rollout 로드 후 spawn_thread_with_source 에 위임 (소스 태깅 포함)
    pub(crate) async fn resume_thread_from_rollout_with_source(
        &self,
        config: Config,
        rollout_path: PathBuf,
        agent_control: AgentControl,
        session_source: SessionSource,
        inherited_shell_snapshot: Option<Arc<ShellSnapshot>>,
        inherited_exec_policy: Option<Arc<crate::exec_policy::ExecPolicyManager>>,
    ) -> CodexResult<NewThread> {
        let initial_history = RolloutRecorder::get_rollout_history(&rollout_path).await?;
        Box::pin(self.spawn_thread_with_source(
            config,
            initial_history,
            Arc::clone(&self.auth_manager),
            agent_control,
            session_source,
            Vec::new(),
            /*persist_extended_history*/ false,
            /*metrics_service_name*/ None,
            inherited_shell_snapshot,
            inherited_exec_policy,
            /*parent_trace*/ None,
            /*user_shell_override*/ None,
        ))
        .await
    }

    // 단순 위임 — 포크된 히스토리로 소스 태깅과 함께 스폰
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn fork_thread_with_source(
        &self,
        config: Config,
        initial_history: InitialHistory,
        agent_control: AgentControl,
        session_source: SessionSource,
        persist_extended_history: bool,
        inherited_shell_snapshot: Option<Arc<ShellSnapshot>>,
        inherited_exec_policy: Option<Arc<crate::exec_policy::ExecPolicyManager>>,
    ) -> CodexResult<NewThread> {
        Box::pin(self.spawn_thread_with_source(
            config,
            initial_history,
            Arc::clone(&self.auth_manager),
            agent_control,
            session_source,
            Vec::new(),
            persist_extended_history,
            /*metrics_service_name*/ None,
            inherited_shell_snapshot,
            inherited_exec_policy,
            /*parent_trace*/ None,
            /*user_shell_override*/ None,
        ))
        .await
    }

    // ## spawn_thread
    //
    // **What:** `ThreadManager` 공개 API 에서 호출되는 중간 레이어 스폰 함수.
    //   세션 소스를 `self.session_source` 로 고정하고 `spawn_thread_with_source` 에 위임.
    //
    // **Why:** 공개 API 는 세션 소스를 직접 지정할 필요가 없다 (초기화 시 이미 결정됨).
    //   내부 에이전트(sub-agent) 스폰은 다른 소스 태그가 필요할 수 있어 `_with_source` 를 직접 쓴다.

    /// Spawn a new thread with optional history and register it with the manager.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn spawn_thread(
        &self,
        config: Config,
        initial_history: InitialHistory,
        auth_manager: Arc<AuthManager>,
        agent_control: AgentControl,
        dynamic_tools: Vec<codex_protocol::dynamic_tools::DynamicToolSpec>,
        persist_extended_history: bool,
        metrics_service_name: Option<String>,
        parent_trace: Option<W3cTraceContext>,
        user_shell_override: Option<crate::shell::Shell>,
    ) -> CodexResult<NewThread> {
        Box::pin(self.spawn_thread_with_source(
            config,
            initial_history,
            auth_manager,
            agent_control,
            self.session_source.clone(),
            dynamic_tools,
            persist_extended_history,
            metrics_service_name,
            /*inherited_shell_snapshot*/ None,
            /*inherited_exec_policy*/ None,
            parent_trace,
            user_shell_override,
        ))
        .await
    }

    // ## spawn_thread_with_source
    //
    // **What:** 모든 스레드 스폰 경로가 최종적으로 합류하는 **핵심 스폰 함수**다.
    //   모든 파라미터를 받아 `Codex::spawn` 을 호출하고 `finalize_thread_spawn` 으로 마무리한다.
    //
    // **How:**
    //   1. `skills_watcher.register_config` 로 이 스레드의 스킬 파일 감시를 등록한다.
    //   2. `Codex::spawn(CodexSpawnArgs { ... })` 으로 실제 AI 엔진 세션을 시작한다.
    //      - 이 호출이 AI 모델 연결, 컨텍스트 초기화, 세션 상태 셋업을 모두 한다.
    //   3. `finalize_thread_spawn` 에 결과를 넘겨 스레드를 등록하고 첫 이벤트를 기다린다.
    //
    // **Macro Role:** 모든 start/resume/fork/spawn 변형의 최종 목적지.
    //   이 함수가 없으면 어떤 스레드도 존재할 수 없다.
    //
    // **Why 파라미터가 많은가:** 스폰에 필요한 컨텍스트가 실제로 이렇게 많다.
    //   `#[allow(clippy::too_many_arguments)]` 가 달려있는 이유가 이것이다.
    //   옵션 구조체로 묶을 수도 있지만, 이미 `CodexSpawnArgs` 가 내부에서 그 역할을 한다.
    //
    // **Why Not:**
    //   - *Builder 패턴*: `ThreadSpawnBuilder::new().with_history(...).build()` 처럼 쓰면
    //     가독성은 좋아지지만, 빌더가 또 다른 보일러플레이트 파일을 요구한다.
    //     이 코드베이스는 직접 함수 파라미터를 선호하는 경향이 있다.

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn spawn_thread_with_source(
        &self,
        config: Config,
        initial_history: InitialHistory,
        auth_manager: Arc<AuthManager>,
        agent_control: AgentControl,
        session_source: SessionSource,
        dynamic_tools: Vec<codex_protocol::dynamic_tools::DynamicToolSpec>,
        persist_extended_history: bool,
        metrics_service_name: Option<String>,
        inherited_shell_snapshot: Option<Arc<ShellSnapshot>>,
        inherited_exec_policy: Option<Arc<crate::exec_policy::ExecPolicyManager>>,
        parent_trace: Option<W3cTraceContext>,
        user_shell_override: Option<crate::shell::Shell>,
    ) -> CodexResult<NewThread> {
        let watch_registration = self.skills_watcher.register_config(
            &config,
            self.skills_manager.as_ref(),
            self.plugins_manager.as_ref(),
        );
        let CodexSpawnOk {
            codex, thread_id, ..
        } = Codex::spawn(CodexSpawnArgs {
            config,
            auth_manager,
            models_manager: Arc::clone(&self.models_manager),
            environment_manager: Arc::clone(&self.environment_manager),
            skills_manager: Arc::clone(&self.skills_manager),
            plugins_manager: Arc::clone(&self.plugins_manager),
            mcp_manager: Arc::clone(&self.mcp_manager),
            skills_watcher: Arc::clone(&self.skills_watcher),
            conversation_history: initial_history,
            session_source,
            agent_control,
            dynamic_tools,
            persist_extended_history,
            metrics_service_name,
            inherited_shell_snapshot,
            inherited_exec_policy,
            user_shell_override,
            parent_trace,
            analytics_events_client: self.analytics_events_client.clone(),
        })
        .await?;
        self.finalize_thread_spawn(codex, thread_id, watch_registration)
            .await
    }

    // ## finalize_thread_spawn
    //
    // **What:** 스폰된 `Codex` 엔진이 준비됐음을 확인하고, 스레드 맵에 등록하고, `NewThread` 를 반환한다.
    //
    // **How:**
    //   1. `codex.next_event()` 로 첫 이벤트를 기다린다.
    //   2. 💡 이 첫 이벤트는 반드시 `SessionConfigured` 여야 한다 — 다른 이벤트가 오면 오류.
    //      이 확인이 "엔진이 제대로 초기화됐는가"를 검증하는 유일한 시점이다.
    //   3. `CodexThread` 를 `Arc` 로 감싸 스레드 맵에 삽입한다.
    //   4. `NewThread` 구조체로 묶어 반환한다. 호출자는 이를 통해 스레드를 제어한다.
    //
    // **Macro Role:** 스폰의 마지막 단계. 이 함수가 성공해야 스레드가 "살아있다"고 볼 수 있다.
    //
    // **Why SessionConfigured 확인:** 클라이언트가 스레드를 사용하기 전에 세션 설정이
    //   완료됐음을 프로토콜 수준에서 보장하기 위해서다.
    //   이 검증 없이는 스레드가 초기화 중인 상태에서 사용자 Op 이 들어올 수 있다.
    //
    // **Why Not:**
    //   - *이벤트 확인 생략*: `Codex::spawn` 이 이미 준비된 상태를 반환한다고 가정할 수도 있지만,
    //     비동기 초기화 과정에서 실패가 발생할 수 있어 이 확인이 안전망 역할을 한다.
    async fn finalize_thread_spawn(
        &self,
        codex: Codex,
        thread_id: ThreadId,
        watch_registration: crate::file_watcher::WatchRegistration,
    ) -> CodexResult<NewThread> {
        let event = codex.next_event().await?;
        let session_configured = match event {
            Event {
                id,
                msg: EventMsg::SessionConfigured(session_configured),
            } if id == INITIAL_SUBMIT_ID => session_configured,
            _ => {
                return Err(CodexErr::SessionConfiguredNotFirstEvent);
            }
        };

        let thread = Arc::new(CodexThread::new(
            codex,
            session_configured.rollout_path.clone(),
            watch_registration,
        ));
        let mut threads = self.threads.write().await;
        threads.insert(thread_id, thread.clone());

        Ok(NewThread {
            thread_id,
            thread,
            session_configured,
        })
    }

    // 단순 알림 — 스레드 생성 이벤트를 broadcast 채널로 전송 (구독자 없어도 무시)
    pub(crate) fn notify_thread_created(&self, thread_id: ThreadId) {
        let _ = self.thread_created_tx.send(thread_id);
    }
}

// ## truncate_before_nth_user_message
//
// **What:** rollout 히스토리에서 n번째 유저 메시지 앞까지만 잘라낸 새 히스토리를 반환한다.
//   비유하면 대화 기록에서 "3번째 내 발언 이전까지만 복사"하는 편집기 기능이다.
//
// **How:**
//   1. 히스토리를 `RolloutItem` 벡터로 펼친다.
//   2. 유저 메시지가 있는 인덱스 목록을 구한다.
//   3. 두 가지 엣지케이스를 처리한다:
//      - n이 범위를 벗어나 + 히스토리가 턴 중간이면: 활성 턴 시작 위치에서 자른다.
//      - 그 외: `truncation::truncate_rollout_before_nth_user_message_from_start` 에 위임.
//   4. 잘린 결과가 비어있으면 `InitialHistory::New`, 아니면 `InitialHistory::Forked` 반환.
//
// **Why 엣지케이스 처리:** n이 범위 밖인데 진행 중 턴이 있으면,
//   미완성 턴을 포함한 채로 포크하면 AI가 혼란스러운 상태를 이어받는다.
//   활성 턴 시작점에서 자르면 "아직 AI가 응답 안 한 지점"에서 깔끔하게 시작한다.
//
// **Why Not:**
//   - *항상 `truncation::truncate_rollout_before_nth_user_message_from_start` 사용*:
//     이 함수는 진행 중 턴을 인식하지 못한다. 미완성 상태로 포크하면 불일치가 생긴다.

/// Return a fork snapshot cut strictly before the nth user message (0-based).
///
/// Out-of-range values keep the full committed history at a turn boundary, but
/// when the source thread is currently mid-turn they fall back to cutting
/// before the active turn's opening boundary so the fork omits the unfinished
/// suffix entirely.
fn truncate_before_nth_user_message(
    history: InitialHistory,
    n: usize,
    snapshot_state: &SnapshotTurnState,
) -> InitialHistory {
    let items: Vec<RolloutItem> = history.get_rollout_items();
    let user_positions = truncation::user_message_positions_in_rollout(&items);
    let rolled = if snapshot_state.ends_mid_turn && n >= user_positions.len() {
        if let Some(cut_idx) = snapshot_state
            .active_turn_start_index
            .or_else(|| user_positions.last().copied())
        {
            items[..cut_idx].to_vec()
        } else {
            items
        }
    } else {
        truncation::truncate_rollout_before_nth_user_message_from_start(&items, n)
    };

    if rolled.is_empty() {
        InitialHistory::New
    } else {
        InitialHistory::Forked(rolled)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct SnapshotTurnState {
    ends_mid_turn: bool,
    active_turn_id: Option<String>,
    active_turn_start_index: Option<usize>,
}

// ## snapshot_turn_state
//
// **What:** 저장된 히스토리가 "턴 중간에 끝나는가"를 분석해 포크 전략에 필요한 상태를 반환한다.
//   비유하면 **녹음 테이프를 재생해 "마지막으로 녹음이 끊긴 위치가 문장 중간인가?"를 확인**하는 것.
//
// **How:** 두 경로로 분기된다:
//
//   경로 A — 명시적 턴 생명주기 이벤트가 있는 경우:
//   1. `ThreadHistoryBuilder` 로 히스토리 아이템을 재생한다.
//   2. 활성 턴 ID가 있는지 확인한다.
//   3. 활성 턴의 상태가 `InProgress` 가 아닌 경우(완료/중단됨): 미드턴 아님으로 처리.
//   4. `InProgress` 라면: ends_mid_turn=true, turn_id와 시작 인덱스를 기록.
//
//   경로 B — 명시적 턴 이벤트 없는 합성(synthetic) 히스토리:
//   5. 마지막 유저 메시지 이후에 `TurnComplete`/`TurnAborted` 이벤트가 없으면 미드턴으로 판단.
//
// **Why 두 경로:** 포크/재개 히스토리에는 실제 대화와 달리 명시적 턴 이벤트가 없을 수 있다.
//   (주석 "Synthetic fork/resume histories" 참고)
//   이런 경우도 안전하게 처리하기 위해 두 번째 판단 경로가 필요하다.
//
// **Why Not:**
//   - *단순 "마지막 아이템이 TurnComplete인가" 체크*: 명시적 이벤트가 없는 합성 히스토리에서는
//     이 방법이 통하지 않는다. 위 경로 B 가 이를 해결한다.
//
// 💡 `ThreadHistoryBuilder` 는 이벤트를 재생해 현재 상태를 추적하는 **상태 머신**이다.
//   이 패턴은 이벤트 소싱(Event Sourcing) 아키텍처의 핵심 개념이다.
fn snapshot_turn_state(history: &InitialHistory) -> SnapshotTurnState {
    let rollout_items = history.get_rollout_items();
    let mut builder = ThreadHistoryBuilder::new();
    for item in &rollout_items {
        builder.handle_rollout_item(item);
    }
    let active_turn_id = builder.active_turn_id_if_explicit();
    if builder.has_active_turn() && active_turn_id.is_some() {
        let active_turn_snapshot = builder.active_turn_snapshot();
        if active_turn_snapshot
            .as_ref()
            .is_some_and(|turn| turn.status != TurnStatus::InProgress)
        {
            return SnapshotTurnState {
                ends_mid_turn: false,
                active_turn_id: None,
                active_turn_start_index: None,
            };
        }

        return SnapshotTurnState {
            ends_mid_turn: true,
            active_turn_id,
            active_turn_start_index: builder.active_turn_start_index(),
        };
    }

    let Some(last_user_position) = truncation::user_message_positions_in_rollout(&rollout_items)
        .last()
        .copied()
    else {
        return SnapshotTurnState {
            ends_mid_turn: false,
            active_turn_id: None,
            active_turn_start_index: None,
        };
    };

    // Synthetic fork/resume histories can contain user/assistant response items
    // without explicit turn lifecycle events. If the persisted snapshot has no
    // terminating boundary after its last user message, treat it as mid-turn.
    SnapshotTurnState {
        ends_mid_turn: !rollout_items[last_user_position + 1..].iter().any(|item| {
            matches!(
                item,
                RolloutItem::EventMsg(EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_))
            )
        }),
        active_turn_id: None,
        active_turn_start_index: None,
    }
}

// ## append_interrupted_boundary
//
// **What:** 진행 중인 턴이 있는 포크 히스토리 끝에 "중단됨" 마커를 붙인다.
//   비유하면 누군가가 말하다 멈춘 대화 기록에 "[통화 끊김]" 주석을 추가하는 것.
//
// **How:**
//   1. `TurnAbortedEvent { reason: Interrupted, ... }` 를 `RolloutItem` 으로 만든다.
//   2. `InitialHistory` 변형에 따라 다르게 처리한다:
//      - `New`/`Cleared`: 빈 히스토리에도 마커만 붙인 `Forked` 히스토리를 만든다.
//      - `Forked`/`Resumed`: 기존 아이템 뒤에 마커를 추가한다.
//   3. 결과는 항상 `InitialHistory::Forked` 형태다.
//
// **Why interrupted_turn_history_marker():** `TurnAbortedEvent` 외에 추가 마커도 붙이는 이유는
//   실제 인터럽트 경로가 두 가지 아이템을 기록하기 때문이다.
//   이 함수는 그 경로를 **정확히** 재현해 포크된 히스토리가 실제 중단된 것처럼 보이게 한다.
//
// **Why Not:**
//   - *항상 빈 히스토리에 마커만 붙이기*: `New` 일 때도 기존 히스토리가 없으면 마커가 의미 없어 보이지만,
//     프로토콜 일관성을 위해 항상 붙인다. 클라이언트는 마커를 보고 "이 세션은 중단된 상태에서 시작됐다"를 안다.

/// Append the same persisted interrupt boundary used by the live interrupt path
/// to an existing fork snapshot after the source thread has been confirmed to
/// be mid-turn.
fn append_interrupted_boundary(history: InitialHistory, turn_id: Option<String>) -> InitialHistory {
    let aborted_event = RolloutItem::EventMsg(EventMsg::TurnAborted(TurnAbortedEvent {
        turn_id,
        reason: TurnAbortReason::Interrupted,
        completed_at: None,
        duration_ms: None,
    }));

    match history {
        InitialHistory::New | InitialHistory::Cleared => InitialHistory::Forked(vec![
            RolloutItem::ResponseItem(interrupted_turn_history_marker()),
            aborted_event,
        ]),
        InitialHistory::Forked(mut history) => {
            history.push(RolloutItem::ResponseItem(interrupted_turn_history_marker()));
            history.push(aborted_event);
            InitialHistory::Forked(history)
        }
        InitialHistory::Resumed(mut resumed) => {
            resumed
                .history
                .push(RolloutItem::ResponseItem(interrupted_turn_history_marker()));
            resumed.history.push(aborted_event);
            InitialHistory::Forked(resumed.history)
        }
    }
}

#[cfg(test)]
#[path = "thread_manager_tests.rs"]
mod tests;

// ## 🎓 What to Steal for Your Own Projects
//
// 이 파일에서 훔쳐갈 수 있는 패턴들:
//
// ### 1. 공개 핸들 + Arc 공유 내부 상태 분리 패턴
// ```rust
// pub struct MyManager { state: Arc<MyManagerState>, ... }
// struct MyManagerState { ... }
// impl MyManager { fn inner_handle(&self) -> InnerControl { InnerControl(Arc::downgrade(&self.state)) } }
// ```
// **언제:** 외부 컴포넌트가 역방향 참조가 필요할 때. 순환 Arc 참조를 피해야 할 때.
//
// ### 2. FuturesUnordered 동시 종료 패턴
// ```rust
// let futs = items.into_iter().map(|item| async move { ... }).collect::<FuturesUnordered<_>>();
// while let Some(result) = futs.next().await { ... }
// ```
// **언제:** N개의 독립적인 비동기 작업을 동시에 실행하고 완료 순서대로 처리할 때.
// `join_all` 대신 쓰면 개별 타임아웃 적용이 가능하고 조기 결과 처리가 된다.
//
// ### 3. Box::pin 재귀 async 스택오버플로 방지 패턴
// ```rust
// pub async fn my_fn(&self, ...) -> Result<...> {
//     Box::pin(self.inner_fn(...)).await
// }
// ```
// **언제:** async 함수들이 서로 호출하는 체인이 컴파일러에게 재귀처럼 보일 때.
// Future 를 힙에 올려 스택 크기 추론 문제를 해결한다.
//
// ### 4. TempGuard RAII 패턴 (Drop 으로 정리)
// ```rust
// struct TempDirGuard { path: PathBuf }
// impl Drop for TempDirGuard { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); } }
// ```
// **언제:** 테스트나 임시 작업에서 "무조건 정리"를 보장해야 할 때.
// `defer` 키워드 없는 Rust 에서 Go의 defer 를 구현하는 방법이다.
//
// ### 5. AtomicBool 전역 테스트 모드 플래그
// ```rust
// static TEST_MODE: AtomicBool = AtomicBool::new(false);
// fn set_test_mode(enabled: bool) { TEST_MODE.store(enabled, Ordering::Relaxed); }
// fn is_test_mode() -> bool { TEST_MODE.load(Ordering::Relaxed) }
// ```
// **언제:** 전역 싱글턴 동작을 테스트에서만 바꿔야 할 때.
// `cfg(test)` 대신 런타임 플래그를 써서 통합 테스트(별도 바이너리)에서도 동작하게 한다.
