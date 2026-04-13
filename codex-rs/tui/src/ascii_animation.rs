//! 📄 이 파일이 하는 일:
//!   팝업이나 온보딩 화면에서 쓰는 ASCII 애니메이션의 현재 프레임과 다음 프레임 예약을 관리한다.
//!   비유로 말하면 여러 장의 손그림을 시간 맞춰 넘겨서 움직이는 것처럼 보이게 하는 플립북 감독이다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui`
//!   - 팝업/온보딩 ASCII 애니메이션 표시 코드
//!
//! 🧩 핵심 개념:
//!   - variant = 여러 애니메이션 세트 중 하나의 그림 묶음
//!   - frame tick = 다음 그림으로 넘길 시간 간격

use std::convert::TryFrom;
use std::time::Duration;
use std::time::Instant;

use rand::Rng as _;

use crate::frames::ALL_VARIANTS;
use crate::frames::FRAME_TICK_DEFAULT;
use crate::tui::FrameRequester;

/// Drives ASCII art animations shared across popups and onboarding widgets.
/// 🍳 이 구조체는 ASCII 그림장을 시간표에 맞춰 넘겨 주는 재생기다.
pub(crate) struct AsciiAnimation {
    request_frame: FrameRequester,
    variants: &'static [&'static [&'static str]],
    variant_idx: usize,
    frame_tick: Duration,
    start: Instant,
}

impl AsciiAnimation {
    /// 🍳 이 함수는 기본 그림 세트로 새 애니메이션 재생기를 만든다.
    pub(crate) fn new(request_frame: FrameRequester) -> Self {
        Self::with_variants(request_frame, ALL_VARIANTS, /*variant_idx*/ 0)
    }

    /// 🍳 이 함수는 그림 세트와 시작 variant를 직접 지정해 재생기를 만든다.
    pub(crate) fn with_variants(
        request_frame: FrameRequester,
        variants: &'static [&'static [&'static str]],
        variant_idx: usize,
    ) -> Self {
        assert!(
            !variants.is_empty(),
            "AsciiAnimation requires at least one animation variant",
        );
        let clamped_idx = variant_idx.min(variants.len() - 1);
        Self {
            request_frame,
            variants,
            variant_idx: clamped_idx,
            frame_tick: FRAME_TICK_DEFAULT,
            start: Instant::now(),
        }
    }

    /// 🍳 이 함수는 다음 프레임이 딱 tick 경계에 다시 그려지도록 예약한다.
    pub(crate) fn schedule_next_frame(&self) {
        let tick_ms = self.frame_tick.as_millis();
        if tick_ms == 0 {
            self.request_frame.schedule_frame();
            return;
        }
        let elapsed_ms = self.start.elapsed().as_millis();
        let rem_ms = elapsed_ms % tick_ms;
        let delay_ms = if rem_ms == 0 {
            tick_ms
        } else {
            tick_ms - rem_ms
        };
        if let Ok(delay_ms_u64) = u64::try_from(delay_ms) {
            self.request_frame
                .schedule_frame_in(Duration::from_millis(delay_ms_u64));
        } else {
            self.request_frame.schedule_frame();
        }
    }

    /// 🍳 이 함수는 지금 시간에 맞는 현재 그림 한 장을 골라 돌려준다.
    pub(crate) fn current_frame(&self) -> &'static str {
        let frames = self.frames();
        if frames.is_empty() {
            return "";
        }
        let tick_ms = self.frame_tick.as_millis();
        if tick_ms == 0 {
            return frames[0];
        }
        let elapsed_ms = self.start.elapsed().as_millis();
        let idx = ((elapsed_ms / tick_ms) % frames.len() as u128) as usize;
        frames[idx]
    }

    /// 🍳 이 함수는 현재 variant와 다른 랜덤 그림 세트를 골라 즉시 다시 그리게 한다.
    pub(crate) fn pick_random_variant(&mut self) -> bool {
        if self.variants.len() <= 1 {
            return false;
        }
        let mut rng = rand::rng();
        let mut next = self.variant_idx;
        while next == self.variant_idx {
            next = rng.random_range(0..self.variants.len());
        }
        self.variant_idx = next;
        self.request_frame.schedule_frame();
        true
    }

    #[allow(dead_code)]
    /// 🍳 이 함수는 지연 없이 바로 한 번 더 그려 달라고 요청한다.
    pub(crate) fn request_frame(&self) {
        self.request_frame.schedule_frame();
    }

    /// 🍳 이 함수는 현재 선택된 variant의 프레임 배열을 꺼낸다.
    fn frames(&self) -> &'static [&'static str] {
        self.variants[self.variant_idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_tick_must_be_nonzero() {
        assert!(FRAME_TICK_DEFAULT.as_millis() > 0);
    }
}
