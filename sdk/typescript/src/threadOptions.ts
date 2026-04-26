/**
 * ## 📐 Architecture Overview
 *
 * 이 파일은 "한 Thread(대화 세션) 전체에 적용되는 옵션"의 타입을 모은다.
 * TurnOptions가 "한 번의 호출"이라면, ThreadOptions는 "이 대화방 자체의 설정".
 *
 * 데이터 흐름:
 *
 *     codex.startThread(threadOptions)  →  Thread 객체 보관
 *                                                ↓
 *                                  매 턴마다 [[exec.ts::CodexExec.run]] 호출 시 그대로 넘겨짐
 *                                                ↓
 *                                  CLI 플래그(--sandbox, --model 등)로 변환되어 child process에 전달
 *
 * 여기 정의된 enum 타입들은 CLI의 가능한 값 집합과 1:1 매칭된다.
 * 즉 SDK 사용자가 잘못된 문자열을 적으면 컴파일 타임에 잡힌다 (런타임 에러 X).
 */

/*
 * Layer 1 — What
 *     사용자 명령에 대한 승인(approval) 정책 종류.
 *
 * Layer 4 — Why
 *     - 문자열 union 타입: enum 대신 string literal union을 쓴 이유는 런타임 객체가
 *       생성되지 않아 트리쉐이킹/번들 사이즈에 유리하고, JSON 직렬화 시 그대로 호환됨.
 *     - "never" / "untrusted" 등 의미 있는 단어로 표현: 숫자 enum보다 디버깅 시 친절.
 *
 * Layer 5 — Why Not
 *     - TypeScript `enum` 안 쓴 이유: enum은 런타임에 객체를 만들어 사이드 이펙트를
 *       일으킴 ("sideEffects": false 정책에 충돌). 또 reverse-mapping 등 굳이 필요 없음.
 *     - 별도 클래스 안 만든 이유: 그냥 플래그값일 뿐 메서드 필요 없음. KISS 원칙.
 */
export type ApprovalMode = "never" | "on-request" | "on-failure" | "untrusted";

/*
 * Layer 1 — What
 *     에이전트가 실행되는 샌드박스(격리 환경) 종류.
 *
 * Layer 4 — Why
 *     - "read-only" / "workspace-write" / "danger-full-access" 3단계 — 보안과 편의의
 *       균형. 위험할수록 이름이 명시적("danger-")이라 사용자가 실수로 선택하기 어렵게.
 *
 * Layer 5 — Why Not
 *     - boolean(`sandbox: true/false`)로 안 한 이유: 샌드박스는 0/1이 아니라 스펙트럼.
 *       세밀한 조절이 안 되면 사용자가 결국 보안을 통째로 끄게 됨.
 */
export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";

/*
 * Layer 1 — What
 *     모델이 추론(reasoning)에 얼마나 토큰/시간을 쓸지의 강도.
 *
 * Layer 4 — Why
 *     - 5단계 — minimal부터 xhigh까지. 단순 분류/포맷팅은 minimal로 비용 절약,
 *       복잡한 디버깅은 xhigh로 정확도 확보.
 */
export type ModelReasoningEffort = "minimal" | "low" | "medium" | "high" | "xhigh";

/*
 * Layer 1 — What
 *     웹 검색 모드. 끔 / 캐시 사용 / 실시간.
 *
 * Layer 4 — Why
 *     - "cached"가 별도로 있는 이유: 같은 쿼리를 반복 검색할 때 비용/지연 감소.
 *       완전 끄거나 완전 켜는 것 외에 중간 옵션을 명시적으로 제공.
 */
export type WebSearchMode = "disabled" | "cached" | "live";

/*
 * Layer 1 — What
 *     Thread를 시작할 때 한 번 정해두면 그 대화 내내 적용되는 옵션 묶음.
 *
 * Layer 2 — How
 *     1. codex.startThread(options)에 전달.
 *     2. Thread 인스턴스 내부에 저장.
 *     3. 매 턴마다 [[exec.ts]]가 이 객체를 읽어서 CLI 플래그로 변환.
 *
 * Layer 4 — Why
 *     - 모두 optional(?): 사용자가 일일이 다 채울 필요 없음. CLI 디폴트 활용.
 *     - 평평한 객체 (중첩 X): TS의 인텔리센스가 잘 동작하고, JSON 직렬화 친화적.
 *
 * Layer 5 — Why Not
 *     - Builder 패턴 안 쓴 이유: 옵션이 10개 미만이고 단순. 빌더는 Java/C# 식 보일러플레이트.
 *       JS/TS의 객체 리터럴이 이미 충분히 가독성 좋음.
 *     - webSearchMode와 webSearchEnabled 둘 다 있는 이유: 후자는 legacy boolean API
 *       (deprecated 예정). 같이 두면 점진 마이그레이션 가능. ([[exec.ts::CodexExec.run]]에서
 *       webSearchMode 우선 처리)
 *
 * Layer 6 — Lesson
 *     📌 패턴: "Discriminated string union" 으로 enum-like 값 표현.
 *     📌 적용 시점: 라이브러리 공개 API, JSON으로 직렬화될 값, 트리쉐이킹이 중요한 코드.
 */
export type ThreadOptions = {
  model?: string;
  sandboxMode?: SandboxMode;
  workingDirectory?: string;
  skipGitRepoCheck?: boolean;
  modelReasoningEffort?: ModelReasoningEffort;
  networkAccessEnabled?: boolean;
  webSearchMode?: WebSearchMode;
  webSearchEnabled?: boolean;
  approvalPolicy?: ApprovalMode;
  additionalDirectories?: string[];
};

/**
 * ## 🎓 What to Steal for Your Own Projects
 *
 * 1. **enum 대신 string literal union**
 *    - 어디에: ApprovalMode, SandboxMode, ModelReasoningEffort, WebSearchMode.
 *    - 왜 유용: 런타임 코드 0바이트 + JSON 직렬화 자연스러움 + 자동완성 잘 됨.
 *    - 적용 예: 자기 프로젝트의 모든 "고정된 문자열 집합"을 enum 대신 union으로.
 *
 * 2. **Optional 필드로만 구성된 옵션 객체**
 *    - 어디에: ThreadOptions의 모든 필드가 `?`.
 *    - 왜 유용: 사용자는 자기가 신경 쓰는 것만 채우면 됨. 디폴트는 한 단계 아래(CLI)가 책임.
 *    - 적용 예: 모든 SDK의 클라이언트 옵션. "필수 0개, 옵션 N개" 패턴이 사용자 친화적.
 *
 * 3. **위험한 모드 이름에 'danger' 프리픽스**
 *    - 어디에: SandboxMode = "danger-full-access".
 *    - 왜 유용: IDE 자동완성에 'danger-'가 뜨는 순간 사용자가 멈칫. 휴먼 안전망.
 *    - 적용 예: 강제 삭제 함수, force-push 등 위험 옵션 모두에 적용.
 *
 * 4. **Legacy 옵션과 신규 옵션 병기 (마이그레이션 전략)**
 *    - 어디에: webSearchMode (신규) + webSearchEnabled (legacy boolean).
 *    - 왜 유용: 기존 사용자 코드 안 깨면서 점진 전환 가능. major bump 없이 진화.
 *    - 적용 예: 옵션 시그니처를 바꿀 때 항상 이 패턴. (절대 한 번에 쳐내지 말 것.)
 */
