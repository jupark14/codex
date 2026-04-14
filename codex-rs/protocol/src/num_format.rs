//! 📄 이 모듈이 하는 일:
//!   숫자를 로케일 구분기호나 K/M/G 축약표기로 읽기 좋게 바꿔 준다.
//!   비유로 말하면 큰 금액을 `1,234`처럼 쉼표 찍어 읽기 쉽게 적거나 `1.2M`처럼 짧은 별명으로 줄여 적는 계산기다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/exec/src/event_processor_with_human_output.rs`
//!   - 토큰 사용량/통계 숫자 표시 코드
//!
//! 🧩 핵심 개념:
//!   - locale formatter = 나라별 숫자 표기 습관을 반영하는 숫자 도장
//!   - SI suffix = 큰 수를 K/M/G 꼬리표로 줄여 적는 별명표

use std::sync::OnceLock;

use icu_decimal::DecimalFormatter;
use icu_decimal::input::Decimal;
use icu_decimal::options::DecimalFormatterOptions;
use icu_locale_core::Locale;

/// 🍳 이 함수는 현재 시스템 로케일 기준 숫자 포매터를 만들 수 있으면 만든다.
fn make_local_formatter() -> Option<DecimalFormatter> {
    let loc: Locale = sys_locale::get_locale()?.parse().ok()?;
    DecimalFormatter::try_new(loc.into(), DecimalFormatterOptions::default()).ok()
}

/// 🍳 이 함수는 fallback용 `en-US` 포매터를 확실히 만든다.
fn make_en_us_formatter() -> DecimalFormatter {
    #![allow(clippy::expect_used)]
    let loc: Locale = "en-US".parse().expect("en-US wasn't a valid locale");
    DecimalFormatter::try_new(loc.into(), DecimalFormatterOptions::default())
        .expect("en-US wasn't a valid locale")
}

/// 🍳 이 함수는 포매터를 한 번만 만들고 계속 재사용하는 공용 창구다.
fn formatter() -> &'static DecimalFormatter {
    static FORMATTER: OnceLock<DecimalFormatter> = OnceLock::new();
    FORMATTER.get_or_init(|| make_local_formatter().unwrap_or_else(make_en_us_formatter))
}

/// Format an i64 with locale-aware digit separators (e.g. "12345" -> "12,345"
/// for en-US).
/// 🍳 이 함수는 정수를 로케일에 맞는 구분기호가 들어간 문자열로 바꾼다.
pub fn format_with_separators(n: i64) -> String {
    formatter().format(&Decimal::from(n)).to_string()
}

/// 🍳 이 함수는 주어진 formatter 하나를 써서 숫자 하나를 포맷한다.
fn format_with_separators_with_formatter(n: i64, formatter: &DecimalFormatter) -> String {
    formatter.format(&Decimal::from(n)).to_string()
}

/// 🍳 이 함수는 큰 숫자를 K/M/G 꼬리표가 붙은 짧은 문자열로 줄여 적는다.
fn format_si_suffix_with_formatter(n: i64, formatter: &DecimalFormatter) -> String {
    let n = n.max(0);
    if n < 1000 {
        return formatter.format(&Decimal::from(n)).to_string();
    }

    // 🧮 소수점 자리 수를 먼저 정한 뒤 스케일을 곱해 정수처럼 반올림하면,
    //    locale formatter를 그대로 쓰면서도 `1.23M` 같은 모양을 안정적으로 만들 수 있다.
    // Format `n / scale` with the requested number of fractional digits.
    let format_scaled = |n: i64, scale: i64, frac_digits: u32| -> String {
        let value = n as f64 / scale as f64;
        let scaled: i64 = (value * 10f64.powi(frac_digits as i32)).round() as i64;
        let mut dec = Decimal::from(scaled);
        dec.multiply_pow10(-(frac_digits as i16));
        formatter.format(&dec).to_string()
    };

    const UNITS: [(i64, &str); 3] = [(1_000, "K"), (1_000_000, "M"), (1_000_000_000, "G")];
    let f = n as f64;
    for &(scale, suffix) in &UNITS {
        if (100.0 * f / scale as f64).round() < 1000.0 {
            return format!("{}{}", format_scaled(n, scale, 2), suffix);
        } else if (10.0 * f / scale as f64).round() < 1000.0 {
            return format!("{}{}", format_scaled(n, scale, 1), suffix);
        } else if (f / scale as f64).round() < 1000.0 {
            return format!("{}{}", format_scaled(n, scale, 0), suffix);
        }
    }

    // Above 1000G, keep whole‑G precision.
    format!(
        "{}G",
        format_with_separators_with_formatter(((n as f64) / 1e9).round() as i64, formatter)
    )
}

/// Format token counts to 3 significant figures, using base-10 SI suffixes.
///
/// Examples (en-US):
///   - 999 -> "999"
///   - 1200 -> "1.20K"
///   - 123456789 -> "123M"
/// 🍳 이 함수는 바깥 호출자가 쓰는 대표 축약 포맷 함수다.
pub fn format_si_suffix(n: i64) -> String {
    format_si_suffix_with_formatter(n, formatter())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kmg() {
        let formatter = make_en_us_formatter();
        let fmt = |n: i64| format_si_suffix_with_formatter(n, &formatter);
        assert_eq!(fmt(0), "0");
        assert_eq!(fmt(999), "999");
        assert_eq!(fmt(1_000), "1.00K");
        assert_eq!(fmt(1_200), "1.20K");
        assert_eq!(fmt(10_000), "10.0K");
        assert_eq!(fmt(100_000), "100K");
        assert_eq!(fmt(999_500), "1.00M");
        assert_eq!(fmt(1_000_000), "1.00M");
        assert_eq!(fmt(1_234_000), "1.23M");
        assert_eq!(fmt(12_345_678), "12.3M");
        assert_eq!(fmt(999_950_000), "1.00G");
        assert_eq!(fmt(1_000_000_000), "1.00G");
        assert_eq!(fmt(1_234_000_000), "1.23G");
        // Above 1000G we keep whole‑G precision (no higher unit supported here).
        assert_eq!(fmt(1_234_000_000_000), "1,234G");
    }
}
