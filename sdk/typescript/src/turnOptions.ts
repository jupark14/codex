/**
 * ## 📐 Architecture Overview
 *
 * 이 파일은 "한 번의 대화 턴(turn)에 한정된 옵션"의 타입만 정의한다.
 * Codex SDK에서 옵션은 3단계 스코프로 분리된다:
 *
 *     [CodexOptions]    →  Codex 인스턴스 전체에 적용 (CLI 경로, env, baseUrl 등)
 *           ↓
 *     [ThreadOptions]   →  Thread(대화 세션) 전체에 적용 (model, sandbox, cwd)
 *           ↓
 *     ★ [TurnOptions] ★ →  단 한 번의 run()/runStreamed() 호출에만 적용
 *
 * 즉 가장 좁은 스코프. 매 턴마다 바뀔 수 있는 값들이 여기 모인다 (현재는 2개뿐).
 *
 * 호출 흐름에서의 위치:
 *   [[thread.ts::Thread.run]] / [[thread.ts::Thread.runStreamed]] 의 2번째 인자.
 */

/*
 * Layer 1 — What
 *     "이번 한 번의 턴"에만 적용될 옵션. outputSchema와 AbortSignal 두 가지.
 *
 * Layer 2 — How
 *     Thread.run(input, turnOptions)에 매 호출마다 새로 넘긴다.
 *     Thread 자체에 저장되지 않으므로 다음 턴에는 영향 없음.
 *
 * Layer 4 — Why
 *     - outputSchema: 같은 thread라도 어떤 턴은 자유 텍스트, 어떤 턴은 JSON 응답이
 *       필요할 수 있다. 그래서 thread가 아닌 턴 단위로 잡아야 자연스러움.
 *     - signal (AbortSignal): "이 한 번의 요청만 취소"를 표현하기 위함. 브라우저 fetch와
 *       동일한 컨벤션을 따라 친숙함을 확보.
 *
 * Layer 5 — Why Not
 *     - 모든 옵션을 ThreadOptions에 합치지 않은 이유: schema는 매 턴 다를 수 있음.
 *       매번 thread를 새로 만들 수는 없음. (thread는 대화 맥락을 누적하는 객체)
 *     - 클래스로 만들지 않은 이유: 단순 plain object. TS의 구조적 타이핑(structural typing)
 *       덕에 사용자가 `{ outputSchema: ... }` 리터럴만 던져도 됨. 학습 곡선 최소화.
 *
 * Layer 6 — Lesson
 *     📌 패턴: "옵션을 스코프별로 분리" — Global → Session → Per-Call.
 *     📌 적용 시점: API 클라이언트, ORM, HTTP fetch wrapper 등 옵션이 많은 라이브러리.
 *     📌 교과서 이름: Hierarchical Configuration Pattern.
 */
export type TurnOptions = {
  /** JSON schema describing the expected agent output. */
  outputSchema?: unknown;
  /** AbortSignal to cancel the turn. */
  signal?: AbortSignal;
};

/**
 * ## 🎓 What to Steal for Your Own Projects
 *
 * 1. **3단계 옵션 계층 (Global / Session / Per-Call)**
 *    - 어디에: TurnOptions = 가장 좁은 스코프. 위에 ThreadOptions, CodexOptions이 쌓임.
 *    - 왜 유용: 사용자가 "한 번만" 바꾸고 싶은 값을 매번 클라이언트 재생성 없이 넘길 수 있음.
 *    - 적용 예: REST 클라이언트의 `client.get(url, { timeout })` — 그 호출에만 timeout 적용.
 *
 * 2. **표준 AbortSignal 채택**
 *    - 어디에: signal?: AbortSignal.
 *    - 왜 유용: fetch, axios, Node 표준이 모두 같은 패턴. 학습 비용 zero.
 *    - 적용 예: 직접 만든 비동기 함수에도 signal 인자만 받으면 cancellable해짐.
 *
 * 3. **`unknown` 타입으로 schema 받기**
 *    - 어디에: outputSchema?: unknown.
 *    - 왜 유용: JSON 스키마는 임의 형태라 강타입화가 힘듦. `unknown`은 `any`보다 안전
 *      (사용 직전에 type narrowing 강제).
 *    - 적용 예: 서드파티 객체를 받는 모든 인터페이스에서 `any` 대신 `unknown`을 디폴트로.
 */
