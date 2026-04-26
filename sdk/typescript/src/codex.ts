/**
 * ## 📐 Architecture Overview
 *
 * 이 파일은 SDK의 **현관문(front door)** 이다. 사용자는 거의 무조건 이 파일의 `Codex`
 * 클래스부터 시작한다 — `new Codex().startThread()` 한 줄이 전체 라이브러리의 진입점.
 *
 * 역할은 한 줄로 요약 가능: "사용자 옵션을 받아서 Thread를 생산하는 팩토리".
 *
 * 데이터/제어 흐름에서의 위치:
 *
 *     [User code]
 *         ↓ new Codex(opts)
 *     ★ [Codex] ★  ─── owns ───>  [[exec.ts::CodexExec]]   (CLI를 실제로 spawn하는 객체)
 *         ↓ startThread() / resumeThread()
 *     [[thread.ts::Thread]]       (대화 세션 1개)
 *         ↓ thread.run() / runStreamed()
 *     [[exec.ts::CodexExec.run]]   (child process 생성 → JSONL 스트림)
 *
 * 의존성:
 *   - [[codexOptions.ts]] : 사용자가 던지는 최상위 옵션 타입
 *   - [[threadOptions.ts]]: 각 Thread의 설정 타입
 *   - [[exec.ts]]         : CLI를 실제로 실행하는 저수준 모듈
 *   - [[thread.ts]]       : 대화 단위 객체
 *
 * 핵심 함수 3개 (전부 짧음):
 *   - constructor       : exec 인스턴스 1개를 만들고 옵션을 보관 (Medium)
 *   - startThread()     : 새 Thread를 찍어냄 (Medium)
 *   - resumeThread()    : 과거 thread_id로 Thread를 복원 (Medium)
 */

import { CodexOptions } from "./codexOptions";
import { CodexExec } from "./exec";
import { Thread } from "./thread";
import { ThreadOptions } from "./threadOptions";

/*
 * Layer 1 — What
 *     SDK의 진입 클래스. 사용자가 가장 먼저 만나는 객체.
 *     Thread를 새로 만들거나(startThread), 이전 대화를 이어받는(resumeThread)
 *     팩토리 역할을 한다.
 *
 * Layer 2 — How
 *     1. 생성자가 [[exec.ts::CodexExec]] 인스턴스 1개를 만든다 — 이게 실제 CLI 호출자.
 *     2. CodexOptions 자체도 보관해 둠 — 매 Thread를 만들 때 baseUrl/apiKey 등을 넘기기 위함.
 *     3. startThread/resumeThread는 단순히 `new Thread(...)`를 반환. 클래스 자체는 상태 거의 없음.
 *
 *     💡 핵심 설계: "Codex는 무겁지 않다". exec 인스턴스 1개만 들고 있음.
 *        모든 비싼 작업(child process 생성, JSONL 파싱)은 Thread.run() 시점에 일어남.
 *
 * Layer 3 — Macro Role
 *     이 클래스는 "라이브러리의 정체성"을 담당. 사용자 코드의 첫 줄.
 *     없으면 사용자가 매번 직접 CodexExec, Thread를 조립해야 함 — DX(개발자 경험) 망함.
 *
 *     [[index.ts]]에서 export됨 → 패키지 사용자에게 공개됨.
 *     하위 모듈([[exec.ts]] 등)은 internal — 직접 import하는 사용자가 없도록.
 *
 * Layer 4 — Why
 *     - **팩토리 패턴**: Thread를 직접 export하지 않고 Codex.startThread()로 생성하게 함.
 *       이유: Thread 생성자에는 CodexExec, CodexOptions 등 사용자가 직접 다룰 필요 없는
 *       의존성이 들어감. 팩토리로 가리면 사용자는 "Codex를 만들고 thread를 받는다"만 알면 됨.
 *     - **exec 1개를 모든 Thread가 공유**: CodexExec는 stateless에 가까움 (CLI 경로,
 *       env, config 오버라이드만 보관). 매 Thread마다 새로 만들 이유 없음. 메모리 절약.
 *     - **CodexOptions를 통째로 보관(this.options)**: startThread 호출 시 baseUrl, apiKey
 *       등을 Thread에 다시 넘겨야 하기 때문. 사용자가 한 번만 적게 하려는 배려.
 *
 * Layer 5 — Why Not (★)
 *     - **싱글톤(singleton)으로 안 만든 이유**: 한 프로세스에서 여러 환경(다른 baseUrl이나
 *       apiKey)을 동시에 쓰는 케이스가 실재 — 멀티테넌트 서버, 테스트 격리 등.
 *       싱글톤이면 이걸 강제로 막아버려 활용성 떨어짐.
 *     - **Thread를 직접 `new Thread()`로 노출 안 한 이유**: 위 "팩토리 패턴" 참고.
 *       Thread 생성자에 internal 인자(CodexExec)가 있어 외부에 노출하면 캡슐화가 깨짐.
 *       그래서 Thread 생성자에 `/* @internal *​/` 주석이 붙어 있음 (thread.ts 참고).
 *     - **resumeThread가 fetch/검증을 안 하는 이유**: id를 받아 즉시 객체만 만들어 반환.
 *       실제 thread 존재 여부는 첫 run() 호출 시점에 CLI가 알려줌. 게으른 접근(lazy)이라
 *       네트워크/디스크 I/O 없이 객체 생성이 동기적이고 빠름.
 *
 * Layer 6 — Lesson
 *     📌 패턴: **Factory Method** — 객체 생성을 메서드 뒤에 숨겨 의존성 조립을 캡슐화.
 *     📌 적용 시점: 생성자에 internal 의존성이 있고, 사용자가 그걸 알 필요가 없을 때.
 *     📌 교과서 이름: GoF Factory Method Pattern. (Java의 `Database.getConnection()`,
 *        Python의 `requests.Session()`도 같은 계열)
 *     📌 부가 패턴: **Lazy initialization** — resumeThread가 즉시 검증하지 않고 첫 사용
 *        시점에 자연스레 검증되도록 함. CLI/네트워크 도구에서 흔한 패턴.
 */
/**
 * Codex is the main class for interacting with the Codex agent.
 *
 * Use the `startThread()` method to start a new thread or `resumeThread()` to resume a previously started thread.
 */
export class Codex {
  private exec: CodexExec;
  private options: CodexOptions;

  /*
   * Layer 1 — What
   *     생성자. 옵션을 받아 내부 상태(exec + options)를 초기화.
   *
   * Layer 2 — How
   *     1. CodexOptions를 destructure해서 [[exec.ts::CodexExec]]가 필요로 하는 3개 필드만 추출.
   *     2. CodexExec 인스턴스 생성 — 이 시점에 CLI 바이너리 경로 탐색이 일어남
   *        ([[exec.ts::findCodexPath]]).
   *     3. 원본 options도 그대로 보관 — startThread 시점에 baseUrl 등이 필요하기 때문.
   *
   * Layer 4 — Why
   *     - 디폴트값 `options: CodexOptions = {}`: 사용자가 `new Codex()`만 적어도 동작.
   *       0-config 시작점.
   *     - exec과 options를 둘 다 보관: 책임 분리. exec은 "CLI를 어떻게 부를지"만 담당,
   *       options는 "Thread에 무엇을 전달할지"만 담당. 한 객체가 둘을 다 갖지 않음.
   *
   * Layer 5 — Why Not
   *     - 옵션 검증(예: apiKey 형식 체크)을 안 하는 이유: 검증은 첫 run() 시점에 CLI가
   *       해줌. SDK가 중복 검증하면 CLI 정책 변경 시 SDK까지 패치해야 함 — 유지보수 비용.
   */
  constructor(options: CodexOptions = {}) {
    const { codexPathOverride, env, config } = options;
    this.exec = new CodexExec(codexPathOverride, env, config);
    this.options = options;
  }

  /*
   * Layer 1 — What
   *     새 대화 세션을 시작. 새 [[thread.ts::Thread]] 인스턴스를 반환한다.
   *
   * Layer 2 — How
   *     1. Thread 생성자에 (exec, codex의 options, threadOptions, id=null) 4개를 전달.
   *     2. id=null이라는 건 "아직 thread_id가 없다" — 첫 run() 호출 시 CLI가 발급해서
   *        [[thread.ts::Thread.runStreamedInternal]]가 채워줌.
   *
   * Layer 4 — Why
   *     - 빈 객체 디폴트(`= {}`): "특별한 설정 없이 그냥 시작"이라는 가장 흔한 케이스를 지원.
   *     - threadOptions를 매 호출마다 새로 받음: 같은 Codex 인스턴스에서 sandbox 모드가
   *       다른 여러 Thread를 동시에 쓸 수 있음.
   *
   * Layer 5 — Why Not
   *     - thread_id를 여기서 발급(예: UUID)하지 않은 이유: thread_id의 발급 권한은 CLI에
   *       있음. SDK가 임의 ID를 만들면 CLI 측 세션 디렉토리(~/.codex/sessions)와 어긋남.
   *       "ID는 서버(CLI)가 만든다"는 일관된 규칙을 지킴.
   */
  /**
   * Starts a new conversation with an agent.
   * @returns A new thread instance.
   */
  startThread(options: ThreadOptions = {}): Thread {
    return new Thread(this.exec, this.options, options);
  }

  /*
   * Layer 1 — What
   *     이전에 시작했던 Thread를 id로 다시 찾아 이어가는 진입점.
   *
   * Layer 2 — How
   *     1. startThread와 거의 동일하지만 마지막 인자로 `id`를 넘김.
   *     2. Thread 객체 자체는 즉시 반환 — 디스크/네트워크 검증 없음 (lazy).
   *     3. 다음 run() 호출 시 [[exec.ts::CodexExec.run]]에서 `commandArgs.push("resume", id)`로
   *        CLI에 `codex exec resume <id>`를 실행 → CLI가 ~/.codex/sessions에서 세션 복원.
   *     💡 즉 "resumeThread"는 사실 메모리에 빈 껍데기를 만들 뿐. 실제 복원은 CLI 책임.
   *
   * Layer 4 — Why
   *     - 동기적 반환: 사용자가 `await codex.resumeThread(id)`처럼 비동기 처리하지 않아도 됨.
   *       객체 생성은 빠르고, 실제 I/O는 첫 turn 시점에 자연스럽게 발생.
   *     - 결과적으로 startThread와 resumeThread가 거의 동일한 시그니처 → 학습 부담 ↓.
   *
   * Layer 5 — Why Not
   *     - CLI를 미리 호출해서 "id가 진짜 있는지" 검증 안 한 이유:
   *       (1) child process를 띄우는 것 자체가 비쌈 — 실제 사용 직전까지 미루는 게 효율적.
   *       (2) 만약 검증 후 실제 run() 사이에 세션 파일이 지워질 수도 있음. 어차피 run()
   *           시점에 다시 확인하는 게 단일 진실 공급원(SSOT)이 됨.
   *     - id 형식(UUID, 정수 등) 검증 안 한 이유: id 포맷은 CLI 영역의 결정사항. SDK가
   *       앞서 강제하면 CLI가 포맷 바꿀 때 SDK가 발목 잡음. 그냥 string으로 통과시킴.
   *
   * Layer 6 — Lesson
   *     📌 패턴: **Pass-through ID** — 라이브러리는 ID 발급/검증에 관여하지 않고 그냥 들고
   *        다닌다. 발급자(CLI)가 단일 책임자.
   *     📌 적용 시점: 외부 시스템의 ID(데이터베이스 PK, 외부 API의 resource id 등)를 다룰 때.
   *        SDK가 "관리자 행세"를 하지 말 것. 들고 가서 그대로 던지는 우체부 역할이 정답.
   */
  /**
   * Resumes a conversation with an agent based on the thread id.
   * Threads are persisted in ~/.codex/sessions.
   *
   * @param id The id of the thread to resume.
   * @returns A new thread instance.
   */
  resumeThread(id: string, options: ThreadOptions = {}): Thread {
    return new Thread(this.exec, this.options, options, id);
  }
}

/**
 * ## 🎓 What to Steal for Your Own Projects
 *
 * 1. **Factory Method로 internal 의존성 숨기기**
 *    - 어디에: Codex.startThread()가 Thread 생성자(internal exec 의존)를 가려줌.
 *    - 왜 유용: 사용자는 `codex.startThread()` 한 줄만 외우면 됨. 내부 조립은 안 봐도 됨.
 *    - 적용 예: DB 클라이언트의 `client.collection()`, HTTP의 `client.request()` 등.
 *      "복잡한 조립을 가진 객체"는 직접 노출 말고 무조건 팩토리로.
 *
 * 2. **하나의 expensive 객체를 여러 자식 객체가 공유**
 *    - 어디에: Codex가 CodexExec 인스턴스 1개를 들고, 모든 Thread가 그것을 받아씀.
 *    - 왜 유용: CodexExec 안의 CLI 경로 탐색은 디스크 I/O — 매 Thread마다 반복하면 낭비.
 *    - 적용 예: HTTP 클라이언트의 connection pool, DB 커넥션 풀, 로거 인스턴스.
 *
 * 3. **Lazy initialization (게으른 초기화)**
 *    - 어디에: resumeThread는 객체만 만들고, 실제 CLI 호출은 첫 run()까지 미룸.
 *    - 왜 유용: 객체 생성이 동기적이고 빠름 → 사용자 코드가 단순. 또 검증을 단일 지점
 *      (run 내부)에서만 함 → 일관성 ↑.
 *    - 적용 예: ORM 모델 객체, 파일 핸들 wrapper, 외부 리소스를 가리키는 모든 객체.
 *
 * 4. **0-config 시작 (모든 옵션이 optional, 디폴트 `= {}`)**
 *    - 어디에: `constructor(options: CodexOptions = {})`, `startThread(options: ThreadOptions = {})`.
 *    - 왜 유용: README의 "Quickstart" 코드가 짧아짐 → 첫인상 좋아짐 → 채택률 ↑.
 *    - 적용 예: 모든 SDK/라이브러리. "필수 인자 0개"가 가능하도록 디폴트를 합리적으로 설정.
 *
 * 5. **ID는 발급자(CLI/서버)가 만들고 SDK는 운반만**
 *    - 어디에: thread_id는 CLI가 발급, SDK는 startThread에서 안 만들고 resumeThread에서 안 검증.
 *    - 왜 유용: SSOT(Single Source of Truth) 유지 → 동기화 버그 원천 차단.
 *    - 적용 예: 외부 API의 resource id, DB의 auto-increment PK 등 모든 외부 식별자.
 */
