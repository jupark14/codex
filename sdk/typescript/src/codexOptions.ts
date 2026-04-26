/**
 * ## 📐 Architecture Overview
 *
 * 이 파일은 **가장 바깥쪽 스코프**의 옵션 타입을 정의한다. Codex 인스턴스 자체에 적용되는
 * "이 SDK 클라이언트가 누구이며 어디로 통신하는가" 수준의 설정.
 *
 * 옵션 계층:
 *
 *     ★ [CodexOptions] ★  →  Codex 인스턴스 = "이 클라이언트의 정체"
 *           ↓
 *       [ThreadOptions]   →  Thread 인스턴스 = "이 대화방의 규칙"
 *           ↓
 *       [TurnOptions]     →  단일 호출 = "이번 메시지에만"
 *
 * 호출 흐름에서의 위치:
 *   사용자 코드  →  `new Codex(codexOptions)`  →  내부에서 [[exec.ts::CodexExec]] 인스턴스에 저장
 *
 * 핵심 타입 3개:
 *   - CodexConfigValue : 재귀(recursive) 타입. JSON-like 값을 표현
 *   - CodexConfigObject: 위 값들의 객체 (key=string)
 *   - CodexOptions     : 사용자가 `new Codex({...})`에 넘기는 최상위 옵션
 */

/*
 * Layer 1 — What
 *     CLI에 `--config key=value`로 넘길 수 있는 값의 가능한 형태들.
 *     문자열, 숫자, 불리언, 또는 위 값들의 배열/객체(중첩 가능).
 *
 * Layer 2 — How
 *     자기 자신을 참조하는 재귀 타입(recursive type).
 *     원시값(primitive) | 자기자신의 배열 | 자기자신을 값으로 갖는 객체.
 *     💡 TS의 type alias는 자기 참조가 가능 — 트리/JSON 같은 재귀 자료구조를 한 줄로 표현.
 *
 * Layer 4 — Why
 *     - JSON 호환 형태: TOML이 받아들일 수 있는 거의 모든 값과 호환.
 *     - 재귀 정의: 사용자가 `{ a: { b: { c: 1 } } }` 처럼 임의 깊이로 중첩해도 타입 OK.
 *
 * Layer 5 — Why Not
 *     - `any` 안 쓴 이유: any는 모든 타입 검사를 무력화. unknown도 너무 자유로움.
 *       이렇게 명시적으로 풀어주면 IDE가 잘못된 값(함수, Symbol 등)을 거부함.
 *     - `unknown`로 받지 않은 이유: 사용자에게 "이런 값들만 됩니다"를 IDE가 알려주려면
 *       정확한 union이 필요. unknown은 type narrowing을 사용자가 직접 해야 함.
 *
 * Layer 6 — Lesson
 *     📌 패턴: 재귀 타입(recursive type alias)으로 트리/JSON 구조 표현.
 *     📌 적용 시점: 설정 파일, AST, 트리 구조 데이터, GraphQL 쿼리 트리 등.
 *     📌 교과서 이름: Algebraic Data Type (ADT). Haskell의 `data Json = ...`와 동형.
 */
export type CodexConfigValue = string | number | boolean | CodexConfigValue[] | CodexConfigObject;

/*
 * Layer 1 — What
 *     문자열 키 → CodexConfigValue로 매핑되는 임의 객체. 즉 "JSON 객체".
 *
 * Layer 4 — Why
 *     - Index signature `[key: string]: ...`: 미리 정해진 키가 없는 동적 객체를 표현하는
 *       표준 방식. 사용자가 어떤 키든 던질 수 있게 허용.
 */
export type CodexConfigObject = { [key: string]: CodexConfigValue };

/*
 * Layer 1 — What
 *     `new Codex({...})`에 넘기는 옵션 묶음. CLI 경로 / 인증 / 환경변수 / 설정 오버라이드.
 *
 * Layer 2 — How
 *     [[codex.ts::Codex constructor]]에서 destructure되어
 *     [[exec.ts::CodexExec]]에 그대로 전달됨. SDK 사용자는 이 객체만 신경 쓰면 됨.
 *
 * Layer 4 — Why
 *     - codexPathOverride: 테스트에서 mock 바이너리를 가리키거나, 모노레포에서 로컬 빌드를
 *       지정할 때 필요. 없으면 npm 패키지의 vendor 디렉토리에서 자동 탐색.
 *     - apiKey: 환경변수가 아닌 코드에서 직접 주입할 수 있게. 멀티 테넌트 서버 등에서 유용.
 *     - config: CLI의 `--config` 플래그를 풍부하게 노출. SDK가 직접 모든 옵션을
 *       타입화할 필요 없이, "탈출구(escape hatch)"를 제공해 유연성 확보.
 *     - env: process.env를 일부러 격리하고 싶은 사용자 (예: 서버사이드에서 호출자별 격리)
 *       에게 명시적 통제권 부여.
 *
 * Layer 5 — Why Not
 *     - `config`를 평평한 string 배열(`["key1=val1", ...]`)로 받지 않은 이유: JS/TS
 *       사용자에게는 객체 리터럴이 자연스러움. 평면화/직렬화는 [[exec.ts::serializeConfigOverrides]]
 *       내부에서 처리하므로 사용자에게 노출 안 함.
 *     - env를 partial merge하지 않은 이유: 명시성 우선. env를 주면 process.env를
 *       "전부" 무시. 일부만 덮으면 디버깅이 어려워짐 ("내 PATH 어디 갔지?").
 *
 * Layer 6 — Lesson
 *     📌 패턴: "Escape hatch via config object" — 모든 CLI 옵션을 SDK가 추상화하지 않음.
 *        충분히 자주 쓰는 것만 1급 필드로, 나머지는 generic config로 노출.
 *     📌 적용 시점: 외부 도구를 래핑할 때. 100% 추상화는 라이브러리가 도구 진화를 따라가지
 *        못해 결국 막힘. 탈출구를 항상 남겨둘 것.
 */
export type CodexOptions = {
  codexPathOverride?: string;
  baseUrl?: string;
  apiKey?: string;
  /**
   * Additional `--config key=value` overrides to pass to the Codex CLI.
   *
   * Provide a JSON object and the SDK will flatten it into dotted paths and
   * serialize values as TOML literals so they are compatible with the CLI's
   * `--config` parsing.
   */
  config?: CodexConfigObject;
  /**
   * Environment variables passed to the Codex CLI process. When provided, the SDK
   * will not inherit variables from `process.env`.
   */
  env?: Record<string, string>;
};

/**
 * ## 🎓 What to Steal for Your Own Projects
 *
 * 1. **재귀 type alias로 JSON-like 트리 표현**
 *    - 어디에: CodexConfigValue가 자기 자신을 참조.
 *    - 왜 유용: any 없이 임의 깊이 트리를 안전하게 표현. IDE 자동완성도 그대로 유지.
 *    - 적용 예: 설정 파일 파서, AST 노드, JSON 비스무리한 모든 자료구조.
 *
 * 2. **Escape hatch 필드 (config 같은 generic bag)**
 *    - 어디에: CodexOptions.config?: CodexConfigObject.
 *    - 왜 유용: SDK가 모든 CLI 옵션을 추상화하면 CLI 진화에 못 따라감. "잘 쓰이는 건
 *      1급 필드, 나머지는 config object로 던지기" 패턴이 라이브러리 수명을 늘림.
 *    - 적용 예: HTTP 클라이언트 wrapper에서 자주 쓰는 옵션은 typed로, 나머지는
 *      `extraHeaders`/`extraOptions`로.
 *
 * 3. **All-or-nothing env 정책**
 *    - 어디에: env가 있으면 process.env 무시.
 *    - 왜 유용: 부분 머지는 "어디서 온 값인지" 디버깅을 지옥으로 만듦. 명시성이 결국 더 친절.
 *    - 적용 예: 환경변수, 로깅 컨텍스트, request 컨텍스트 — 모두 동일한 원칙 적용 가능.
 */
