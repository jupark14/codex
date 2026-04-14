//! 📄 이 파일이 하는 일:
//!   stdin으로 받은 Markdown을 pulldown-cmark 이벤트 흐름으로 펼쳐서 디버그 출력한다.
//!   비유로 말하면 긴 글을 문장/제목/목록 같은 조각 이벤트로 잘라 보여 주는 분해 현미경이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - 개발자 수동 디버깅
//!   - Markdown parser 동작 확인용 작은 보조 바이너리
//!
//! 🧩 핵심 개념:
//!   - stdin = 바깥에서 흘려보낸 원문 글
//!   - parser event = 파서가 읽으며 만들어 낸 중간 조각

use std::io::Read;
use std::io::{self};

/// 🍳 이 함수는 stdin Markdown을 읽고 이벤트를 한 줄씩 출력하는 작은 검사기다.
fn main() {
    let mut input = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut input) {
        eprintln!("failed to read stdin: {err}");
        std::process::exit(1);
    }

    let parser = pulldown_cmark::Parser::new(&input);
    for event in parser {
        println!("{event:?}");
    }
}
