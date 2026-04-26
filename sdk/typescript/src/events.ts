/**
 * ## 📐 Architecture Overview
 *
 * 이 파일은 **CLI ↔ SDK 사이의 "방송 프로토콜"** 을 정의한다. Codex CLI는 한 번의 턴 동안
 * 여러 사건(thread 시작, item 도착, turn 종료...)을 JSONL로 stdout에 흘리는데, 그 줄들을
 * SDK에서 어떻게 해석할지가 여기 적혀 있다. 즉 "어떤 type 필드가 오면 어떤 모양의 객체인지"
 * 의 계약서.
 *
 *     [codex CLI stdout]
 *           │  (한 줄 = 한 JSON 객체)
 *           ▼
 *     {"type":"thread.started","thread_id":"abc"}    ← ThreadStartedEvent
 *     {"type":"turn.started"}                        ← TurnStartedEvent
 *     {"type":"item.started","item":{...}}           ← ItemStartedEvent
 *     {"type":"item.updated","item":{...}}           ← ItemUpdatedEvent
 *     {"type":"item.completed","item":{...}}         ← ItemCompletedEvent
 *     {"type":"turn.completed","usage":{...}}        ← TurnCompletedEvent
 *     ...
 *           │  ([[thread.ts::Thread.runStreamedInternal]]가 JSON.parse)
 *           ▼
 *     ★ 이 파일의 ThreadEvent union ★
 *           │
 *           ▼
 *     사용자 코드 (`for await (const event of stream)`)
 *
 * 데이터 흐름에서의 위치:
 *   Rust 쪽 [[codex-rs/exec/src/exec_events.rs]]가 발행 → JSONL → 이 파일이 디코딩 모양 정의.
 *   즉 이 파일은 "Rust 타입의 TypeScript 미러(mirror)"다.
 *
 * 주요 구성:
 *   - 6개의 개별 이벤트 타입 + 1개의 에러 이벤트 타입
 *   - 1개의 Usage 객체 타입 (토큰 사용량)
 *   - 1개의 ThreadError 타입 (실패 사유)
 *   - 마지막에 ThreadEvent = 위 7개의 union (★ 핵심 ★)
 */

// based on event types from codex-rs/exec/src/exec_events.rs

import type { ThreadItem } from "./items";

/*
 * Layer 1 — What
 *     "새 대화방이 열렸어요" 이벤트. 한 턴에서 가장 먼저 단 한 번만 발생.
 *
 * Layer 4 — Why
 *     - thread_id를 여기서 받는 이유: thread를 만들 때 SDK가 ID를 알 길이 없음.
 *       CLI(코덱스)가 발급해서 던져주는 첫 이벤트가 이것. [[thread.ts::Thread.runStreamedInternal]]는
 *       이 이벤트가 오면 `this._id = parsed.thread_id`로 저장. 이후 resume할 때 사용.
 *
 * Layer 5 — Why Not
 *     - thread_id를 클라이언트(SDK)가 미리 만들어 보내지 않은 이유: ID 발급 권한은 CLI에
 *       있음. ~/.codex/sessions의 디렉토리 이름과도 일치해야 하므로 CLI가 단독 결정자.
 */
/** Emitted when a new thread is started as the first event. */
export type ThreadStartedEvent = {
  type: "thread.started";
  /** The identifier of the new thread. Can be used to resume the thread later. */
  thread_id: string;
};

/*
 * Layer 1 — What
 *     "프롬프트 처리가 시작됐어요" 이벤트. 한 턴 안에서 모델 실행이 막 시작됐음을 알림.
 *
 * Layer 4 — Why
 *     - 빈 데이터(payload)인 이유: 시작 시점에는 아직 알릴 내용이 없음.
 *       하지만 "시작 알림"이 있어야 사용자 UI가 spinner를 켜는 등 반응할 수 있음.
 *       (UX적으로 "요청 보냈는데 화면이 멈춰있다"가 가장 나쁜 경험)
 */
/**
 * Emitted when a turn is started by sending a new prompt to the model.
 * A turn encompasses all events that happen while the agent is processing the prompt.
 */
export type TurnStartedEvent = {
  type: "turn.started";
};

/*
 * Layer 1 — What
 *     이번 턴에서 모델이 사용한 토큰 수를 담는 객체. 이벤트가 아니라 다른 이벤트의 일부.
 *
 * Layer 2 — How
 *     [[items.ts::TurnCompletedEvent]]의 `usage` 필드에 담겨서 전달됨.
 *     세 종류의 토큰을 분리해서 받음: input / cached_input / output.
 *
 * Layer 4 — Why
 *     - cached_input_tokens를 별도로 받는 이유: 캐시 히트는 비용이 90% 저렴.
 *       앱이 비용을 정확히 계산하려면 일반 input과 캐시된 input을 구분해야 함.
 *     - 세 개를 하나의 number로 합치지 않은 이유: 합치면 비용 추적 불가능.
 *
 * Layer 5 — Why Not
 *     - 비용(price)을 직접 계산해서 담아주지 않은 이유: 가격은 모델/시점/할인에 따라 변동.
 *       SDK가 가격을 들고 있으면 가격표 변경 시 SDK도 패치해야 함. 그냥 토큰 수만 주고
 *       사용자가 비용 계산을 책임지게 하는 게 안전.
 *
 * Layer 6 — Lesson
 *     📌 패턴: "raw 데이터만 노출, 해석은 사용자에게" — 가격, 통화 등 변동 메타는 노출 안 함.
 *     📌 적용 시점: 비용/시간/통화 등 외부 환경에 의존하는 값을 다룰 때.
 */
/** Describes the usage of tokens during a turn. */
export type Usage = {
  /** The number of input tokens used during the turn. */
  input_tokens: number;
  /** The number of cached input tokens used during the turn. */
  cached_input_tokens: number;
  /** The number of output tokens used during the turn. */
  output_tokens: number;
};

/*
 * Layer 1 — What
 *     "이번 턴 성공적으로 끝났어요" 이벤트. assistant 응답이 모두 도착한 시점.
 *
 * Layer 4 — Why
 *     - usage가 여기서 함께 도착하는 이유: 턴이 끝나야만 정확한 토큰 수가 확정됨.
 *       시작 시점에 알 수 없는 값이라 종료 이벤트에 묶임.
 */
/** Emitted when a turn is completed. Typically right after the assistant's response. */
export type TurnCompletedEvent = {
  type: "turn.completed";
  usage: Usage;
};

/*
 * Layer 1 — What
 *     "이번 턴 실패했어요" 이벤트. error 객체 1개를 동봉.
 *
 * Layer 2 — How
 *     [[thread.ts::Thread.run]]에서 이 이벤트가 오면 stream을 break하고 throw로 변환.
 *     즉 사용자 코드의 try/catch에서 잡힘.
 *
 * Layer 5 — Why Not
 *     - JS Error 타입을 그대로 안 쓴 이유: JSONL 와이어 포맷이라 직렬화/역직렬화 가능한
 *       plain object가 필요. JS의 Error는 stack 등 직렬화 안 되는 필드가 있음.
 */
/** Indicates that a turn failed with an error. */
export type TurnFailedEvent = {
  type: "turn.failed";
  error: ThreadError;
};

/*
 * Layer 1 — What
 *     "새 아이템이 생겼어요" 이벤트. 보통 in_progress 상태로 시작.
 *
 * Layer 2 — How
 *     세 단계 라이프사이클: started → updated(N번) → completed.
 *     명령 실행, 파일 변경, MCP 도구 호출 등 모든 "행동"이 이 사이클을 따름.
 *     💡 이 분리(start/update/complete) 덕에 사용자 UI가 진행률을 보여줄 수 있음.
 *
 * Layer 5 — Why Not
 *     - "completed" 단일 이벤트만 보내지 않은 이유: 긴 명령 실행은 몇 초~몇 분 걸림.
 *       그동안 UI가 멍하니 있으면 안 됨. 진행 중에도 stream으로 부분 결과를 보여주려면
 *       started/updated 이벤트가 필요.
 */
/** Emitted when a new item is added to the thread. Typically the item is initially "in progress". */
export type ItemStartedEvent = {
  type: "item.started";
  item: ThreadItem;
};

/*
 * Layer 1 — What
 *     "기존 아이템이 변했어요" 이벤트. 명령의 stdout이 추가되거나 todo 상태가 바뀔 때 등.
 *
 * Layer 4 — Why
 *     - 매번 전체 item을 다시 보내는 이유: stateless 디자인. delta(차이분)만 보내면
 *       클라이언트가 상태를 누적해야 하는데, 중간에 이벤트 하나라도 놓치면 영원히 어긋남.
 *       "마지막으로 받은 게 곧 진실"이라는 단순함이 더 안전.
 */
/** Emitted when an item is updated. */
export type ItemUpdatedEvent = {
  type: "item.updated";
  item: ThreadItem;
};

/*
 * Layer 1 — What
 *     "이 아이템 끝났어요" 이벤트. 성공이든 실패든 종착점.
 *
 * Layer 2 — How
 *     [[thread.ts::Thread.run]]은 이 이벤트만 골라서 items 배열에 push.
 *     즉 in_progress 상태는 무시하고 final 상태만 누적 — buffered API의 약속과 일치.
 *
 * Layer 4 — Why
 *     - 별도 이벤트 타입으로 분리한 이유: started/updated와 동일 구조지만 의미가 다름.
 *       사용자가 `if (event.type === "item.completed")` 한 줄로 종착점만 필터 가능.
 */
/** Signals that an item has reached a terminal state—either success or failure. */
export type ItemCompletedEvent = {
  type: "item.completed";
  item: ThreadItem;
};

/*
 * Layer 1 — What
 *     에러 정보를 담는 객체. message 한 줄짜리 단순 구조.
 *
 * Layer 5 — Why Not
 *     - code/category 등 추가 필드가 없는 이유: 에러 분류 체계는 CLI 측에서 유동적.
 *       message string만 있으면 사용자가 로깅/표시는 충분히 함. 미래에 필요하면
 *       optional 필드 추가하면 호환성 깨지지 않음.
 */
/** Fatal error emitted by the stream. */
export type ThreadError = {
  message: string;
};

/*
 * Layer 1 — What
 *     스트림이 통째로 끝나버리는 치명적 에러 이벤트. 보통 CLI가 죽거나 통신이 깨질 때.
 *
 * Layer 5 — Why Not
 *     - turn.failed와 분리한 이유: turn.failed는 "이번 턴은 실패했지만 thread는 살아있음".
 *       error는 "스트림 자체가 망가짐". 회복 가능성과 처리 방식이 다르므로 별도 타입.
 */
/** Represents an unrecoverable error emitted directly by the event stream. */
export type ThreadErrorEvent = {
  type: "error";
  message: string;
};

/*
 * Layer 1 — What
 *     CLI에서 받는 모든 이벤트의 union 타입. JSONL 한 줄 한 줄이 이 중 하나.
 *
 * Layer 2 — How
 *     "discriminated union" 패턴: 각 멤버에 공통 필드 `type`이 있고 값이 서로 다름.
 *     [[thread.ts::Thread.runStreamedInternal]]에서 JSON.parse 결과를 이 타입으로 캐스팅.
 *     사용자 코드는 `switch (event.type)`로 분기 — TS가 자동으로 타입을 좁혀줌(narrowing).
 *     💡 이게 TS의 진짜 매력. switch case 안에서 event.usage에 접근하면 IDE가 자동으로
 *        "TurnCompletedEvent로 좁혀졌으니 usage 접근 OK"라고 인지.
 *
 * Layer 3 — Macro Role
 *     이 union은 SDK 사용자가 다루는 **유일한 이벤트 타입**. README의 streaming 예제에서
 *     `for await (const event of events)`의 그 event가 바로 이것.
 *
 * Layer 4 — Why
 *     - union으로 묶은 이유: 다양한 이벤트를 하나의 채널로 흘리되, 사용자가 type 필드 하나로
 *       완벽히 분기 가능. switch 외에는 분기 코드를 짤 일이 없음 — 단순성.
 *
 * Layer 5 — Why Not
 *     - 이벤트마다 별도 메서드(onThreadStarted, onTurnCompleted 등)로 분리 안 한 이유:
 *       콜백 지옥 + EventEmitter 의존성 + 타입 안전성 ↓. async generator + discriminated
 *       union이 훨씬 모던하고 타입 친화적.
 *     - 클래스 기반 이벤트(`class TurnCompleted extends Event {}`) 안 쓴 이유: JSONL로
 *       오는 plain object를 일일이 클래스 인스턴스화하는 건 오버헤드. 게다가 type narrowing
 *       이 instanceof보다 string literal 비교가 더 빠르고 명확.
 *
 * Layer 6 — Lesson
 *     📌 패턴: **Discriminated Union (= Tagged Union, Sum Type)**.
 *        공통 string 필드 + 각자 다른 필드. switch로 안전하게 분기.
 *     📌 적용 시점: 여러 종류의 메시지/이벤트/상태를 한 채널로 흘릴 때.
 *        Redux action, GraphQL response, 모든 프로토콜 메시지 등.
 *     📌 교과서 이름: ADT (Algebraic Data Type)의 sum type. Rust의 enum, Haskell의
 *        `data`와 동일 개념. TS에선 string literal + union으로 표현.
 */
/** Top-level JSONL events emitted by codex exec. */
export type ThreadEvent =
  | ThreadStartedEvent
  | TurnStartedEvent
  | TurnCompletedEvent
  | TurnFailedEvent
  | ItemStartedEvent
  | ItemUpdatedEvent
  | ItemCompletedEvent
  | ThreadErrorEvent;

/**
 * ## 🎓 What to Steal for Your Own Projects
 *
 * 1. **Discriminated Union으로 이벤트 프로토콜 정의**
 *    - 어디에: ThreadEvent = 7개 이벤트의 union, 모두 공통 `type` 필드.
 *    - 왜 유용: 사용자는 switch 하나로 안전하게 분기. TS가 자동으로 타입 좁혀줌.
 *      EventEmitter 패턴(`on('event', cb)`)보다 100배 타입 친화적.
 *    - 적용 예: WebSocket 메시지, Redux action, 워크플로우 상태머신, 알림 시스템 등.
 *
 * 2. **Lifecycle 3단계: started / updated / completed**
 *    - 어디에: ItemStartedEvent → ItemUpdatedEvent → ItemCompletedEvent.
 *    - 왜 유용: 긴 작업의 진행률을 자연스럽게 표현. UI는 started에 spinner, updated에
 *      진행률, completed에 결과 표시.
 *    - 적용 예: 파일 업로드, 빌드 파이프라인, AI 추론 등 "시간이 걸리는 모든 것".
 *
 * 3. **Stateless 업데이트 (delta가 아닌 전체 객체 송신)**
 *    - 어디에: ItemUpdatedEvent가 매번 item 전체를 보냄.
 *    - 왜 유용: 클라이언트가 누적 상태를 들고 있을 필요 없음. 메시지 유실/순서 변경에 강함.
 *    - 적용 예: React 상태 업데이트, 게임 서버의 월드 스냅샷, IDE 프로토콜(LSP).
 *
 * 4. **회복 가능 vs 치명적 에러 분리**
 *    - 어디에: TurnFailedEvent (이 턴만 실패) ≠ ThreadErrorEvent (스트림 통째로 죽음).
 *    - 왜 유용: 호출자가 다르게 대응. 전자는 retry 가능, 후자는 client 재생성 필요.
 *    - 적용 예: HTTP 4xx vs 5xx 분리, 비즈니스 에러 vs 시스템 에러 분리.
 *
 * 5. **Raw 메트릭만 노출 (가공은 사용자 책임)**
 *    - 어디에: Usage가 토큰 수만 주고, 비용(달러)은 안 알려줌.
 *    - 왜 유용: 가격표가 바뀌어도 SDK는 그대로. 환율, 할인 등 외부 변수에서 자유로움.
 *    - 적용 예: 라이브러리는 raw 측정값(bytes, ms, count)만 주고, "사람이 읽을 형식"
 *      (MB, "1.2 sec", "$0.05")으로의 변환은 호출자에게 위임.
 */
