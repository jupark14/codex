//! 📄 이 파일이 하는 일:
//!   realtime 대화에 쓸 마이크/스피커 장치를 찾고 적절한 오디오 설정을 고른다.
//!   비유로 말하면 방송부에서 "어떤 마이크를 쓸지"와 "어떤 음질 설정이 가장 무난한지"를 골라 주는 장비 담당자다.
//!
//! 🔗 누가 이걸 쓰나:
//!   - `codex-rs/tui`
//!   - realtime audio 장치 선택 UI 및 세션 시작 코드
//!
//! 🧩 핵심 개념:
//!   - configured device = 사용자가 미리 이름을 지정한 장치
//!   - fallback default = 지정 장치가 없으면 시스템 기본 장치로 물러나는 안전장치

use crate::legacy_core::config::Config;
use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use tracing::warn;

use crate::app_event::RealtimeAudioDeviceKind;

const PREFERRED_INPUT_SAMPLE_RATE: u32 = 24_000;
const PREFERRED_INPUT_CHANNELS: u16 = 1;

pub(crate) fn list_realtime_audio_device_names(
    kind: RealtimeAudioDeviceKind,
) -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let mut device_names = Vec::new();
    for device in devices(&host, kind)? {
        let Ok(name) = device.name() else {
            continue;
        };
        if !device_names.contains(&name) {
            device_names.push(name);
        }
    }
    Ok(device_names)
}

/// 🍳 이 함수는 설정에 맞는 마이크 장치와 입력 스트림 설정을 함께 고른다.
pub(crate) fn select_configured_input_device_and_config(
    config: &Config,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    select_device_and_config(RealtimeAudioDeviceKind::Microphone, config)
}

/// 🍳 이 함수는 설정에 맞는 스피커 장치와 출력 스트림 설정을 함께 고른다.
pub(crate) fn select_configured_output_device_and_config(
    config: &Config,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    select_device_and_config(RealtimeAudioDeviceKind::Speaker, config)
}

/// 🍳 이 함수는 입력 장치가 지원하는 후보들 중
///   24kHz/모노에 가장 가까운 설정을 골라 준다.
pub(crate) fn preferred_input_config(
    device: &cpal::Device,
) -> Result<cpal::SupportedStreamConfig, String> {
    let supported_configs = device
        .supported_input_configs()
        .map_err(|err| format!("failed to enumerate input audio configs: {err}"))?;

    supported_configs
        .filter_map(|range| {
            let sample_format_rank = match range.sample_format() {
                cpal::SampleFormat::I16 => 0u8,
                cpal::SampleFormat::U16 => 1u8,
                cpal::SampleFormat::F32 => 2u8,
                _ => return None,
            };
            let sample_rate = preferred_input_sample_rate(&range);
            let sample_rate_penalty = sample_rate.0.abs_diff(PREFERRED_INPUT_SAMPLE_RATE);
            let channel_penalty = range.channels().abs_diff(PREFERRED_INPUT_CHANNELS);
            Some((
                (sample_rate_penalty, channel_penalty, sample_format_rank),
                range.with_sample_rate(sample_rate),
            ))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, config)| config)
        .or_else(|| device.default_input_config().ok())
        .ok_or_else(|| "failed to get default input config".to_string())
}

/// 🍳 이 함수는 장치 종류와 설정 이름을 보고 최종 장치+스트림 설정을 고른다.
fn select_device_and_config(
    kind: RealtimeAudioDeviceKind,
    config: &Config,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let host = cpal::default_host();
    let configured_name = configured_name(kind, config);
    let selected = configured_name
        .and_then(|name| find_device_by_name(&host, kind, name))
        .or_else(|| {
            let default_device = default_device(&host, kind);
            if let Some(name) = configured_name && default_device.is_some() {
                warn!(
                    "configured {} audio device `{name}` was unavailable; falling back to system default",
                    kind.noun()
                );
            }
            default_device
        })
        .ok_or_else(|| missing_device_error(kind, configured_name))?;

    let stream_config = match kind {
        RealtimeAudioDeviceKind::Microphone => preferred_input_config(&selected)?,
        RealtimeAudioDeviceKind::Speaker => default_config(&selected, kind)?,
    };
    Ok((selected, stream_config))
}

/// 🍳 이 함수는 config에서 해당 장치 종류의 사용자 지정 이름을 꺼낸다.
fn configured_name(kind: RealtimeAudioDeviceKind, config: &Config) -> Option<&str> {
    match kind {
        RealtimeAudioDeviceKind::Microphone => config.realtime_audio.microphone.as_deref(),
        RealtimeAudioDeviceKind::Speaker => config.realtime_audio.speaker.as_deref(),
    }
}

/// 🍳 이 함수는 장치 목록에서 이름이 정확히 맞는 장치를 찾는다.
fn find_device_by_name(
    host: &cpal::Host,
    kind: RealtimeAudioDeviceKind,
    name: &str,
) -> Option<cpal::Device> {
    let devices = devices(host, kind).ok()?;
    devices
        .into_iter()
        .find(|device| device.name().ok().as_deref() == Some(name))
}

/// 🍳 이 함수는 마이크/스피커 종류에 따라 전체 장치 목록을 읽어 온다.
fn devices(host: &cpal::Host, kind: RealtimeAudioDeviceKind) -> Result<Vec<cpal::Device>, String> {
    match kind {
        RealtimeAudioDeviceKind::Microphone => host
            .input_devices()
            .map(|devices| devices.collect())
            .map_err(|err| format!("failed to enumerate input audio devices: {err}")),
        RealtimeAudioDeviceKind::Speaker => host
            .output_devices()
            .map(|devices| devices.collect())
            .map_err(|err| format!("failed to enumerate output audio devices: {err}")),
    }
}

/// 🍳 이 함수는 시스템 기본 장치를 꺼낸다.
fn default_device(host: &cpal::Host, kind: RealtimeAudioDeviceKind) -> Option<cpal::Device> {
    match kind {
        RealtimeAudioDeviceKind::Microphone => host.default_input_device(),
        RealtimeAudioDeviceKind::Speaker => host.default_output_device(),
    }
}

/// 🍳 이 함수는 선택된 장치의 기본 스트림 설정을 읽는다.
fn default_config(
    device: &cpal::Device,
    kind: RealtimeAudioDeviceKind,
) -> Result<cpal::SupportedStreamConfig, String> {
    match kind {
        RealtimeAudioDeviceKind::Microphone => device
            .default_input_config()
            .map_err(|err| format!("failed to get default input config: {err}")),
        RealtimeAudioDeviceKind::Speaker => device
            .default_output_config()
            .map_err(|err| format!("failed to get default output config: {err}")),
    }
}

/// 🍳 이 함수는 원하는 24kHz가 범위 안이면 그걸 쓰고,
///   아니면 범위 끝값 중 더 가까운 쪽으로 맞춘다.
fn preferred_input_sample_rate(range: &cpal::SupportedStreamConfigRange) -> cpal::SampleRate {
    let min = range.min_sample_rate().0;
    let max = range.max_sample_rate().0;
    if (min..=max).contains(&PREFERRED_INPUT_SAMPLE_RATE) {
        cpal::SampleRate(PREFERRED_INPUT_SAMPLE_RATE)
    } else if PREFERRED_INPUT_SAMPLE_RATE < min {
        cpal::SampleRate(min)
    } else {
        cpal::SampleRate(max)
    }
}

/// 🍳 이 함수는 장치를 못 찾았을 때 사용자에게 보여 줄 이유 문장을 만든다.
fn missing_device_error(kind: RealtimeAudioDeviceKind, configured_name: Option<&str>) -> String {
    match (kind, configured_name) {
        (RealtimeAudioDeviceKind::Microphone, Some(name)) => {
            format!(
                "configured microphone `{name}` was unavailable and no default input audio device was found"
            )
        }
        (RealtimeAudioDeviceKind::Speaker, Some(name)) => {
            format!(
                "configured speaker `{name}` was unavailable and no default output audio device was found"
            )
        }
        (RealtimeAudioDeviceKind::Microphone, None) => {
            "no input audio device available".to_string()
        }
        (RealtimeAudioDeviceKind::Speaker, None) => "no output audio device available".to_string(),
    }
}
