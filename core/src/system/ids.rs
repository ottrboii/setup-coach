//! Stable finding ids produced by the system audit. Shared with
//! [`super::actions`] so every auto-fix maps back to the finding it resolves.

// Windows
pub const MIC_PRIVACY: &str = "system.mic_privacy_blocked";
pub const ENHANCEMENTS: &str = "system.audio_enhancements_on";
pub const VOICE_CLARITY: &str = "system.voice_clarity_on";
pub const SAMPLE_RATE_MISMATCH: &str = "system.sample_rate_mismatch";
pub const MIC_BOOST: &str = "system.mic_boost_on";
pub const ENDPOINT_VOLUME: &str = "system.mic_volume_below_100";
pub const EXCLUSIVE_MODE: &str = "system.exclusive_mode_allowed";
pub const DUCKING: &str = "system.windows_ducking_on";
pub const LISTEN_TO_DEVICE: &str = "system.listen_to_device_on";
pub const USB_SUSPEND: &str = "system.usb_selective_suspend_on";
pub const BLUETOOTH_HANDSFREE: &str = "system.bluetooth_handsfree";
pub const LEVEL_HIJACKED: &str = "system.mic_level_changed_by_app";
pub const WEBSOCKET_OFF: &str = "system.obs_websocket_off";
pub const OBS_OLD: &str = "system.obs_outdated";

// OBS mic source
pub const OBS_DEFAULT_DEVICE: &str = "mic.obs_uses_default_device";
pub const MONITORING_DOUBLED: &str = "mic.monitoring_doubled";
pub const MONITORING_DELAYED: &str = "mic.monitoring_delayed";
pub const AI_SUPPRESSION: &str = "mic.ai_noise_suppression";
pub const SPEEX_SUPPRESSION: &str = "mic.speex_noise_suppression";
pub const FILTER_ORDER: &str = "mic.filter_order_wrong";
pub const GATE_CHOPS: &str = "mic.gate_cuts_words";
pub const GATE_SLOW: &str = "mic.gate_slow_or_short";
pub const LOW_SNR_FOR_GATE: &str = "mic.noise_too_close_to_voice";
pub const EXPANDER_CHOPS: &str = "mic.expander_too_high";
pub const COMPRESSOR_EXTREME: &str = "mic.compressor_extreme";
pub const LIMITER_MISSING: &str = "mic.limiter_missing";
pub const LIMITER_LEVEL: &str = "mic.limiter_level_wrong";
pub const GAIN_STACKED: &str = "mic.gain_stacked";
pub const NOT_MONO: &str = "mic.left_channel_only";
pub const DOUBLE_CAPTURE_APP: &str = "mic.app_audio_captured_twice";
pub const DOUBLE_CAPTURE_MIX: &str = "mic.mic_captured_twice";
pub const VENDOR_DSP_STACKED: &str = "mic.processing_in_two_places";
pub const AI_APP_PROCESSING: &str = "mic.ai_voice_app_active";
pub const ONBOARD_DSP_FIGHTS: &str = "mic.onboard_auto_level_fights_compressor";
pub const MIC_MUTED: &str = "mic.obs_mic_muted";

// Camera pipeline
pub const CAM_PRIVACY: &str = "camera.privacy_blocked";
pub const CAM_FRAME_SERVER: &str = "camera.frame_server_stopped";
pub const CAM_STUDIO_EFFECTS: &str = "camera.studio_effects_on";
pub const CAM_MULTI_APP: &str = "camera.multi_app_mode_on";
pub const CAM_VENDOR_APPS: &str = "camera.vendor_app_overrides";
pub const CAM_DEVICE_DEFAULT: &str = "camera.obs_device_default_mode";
pub const CAM_YUY2_HIGH_RES: &str = "camera.uncompressed_high_res";
pub const CAM_COLOR_RANGE: &str = "camera.color_range_mismatch";

// Encoder / stream config
pub const ENC_CPU_WITH_GPU: &str = "stream.cpu_encoder_with_gpu_available";
pub const ENC_NOT_CBR: &str = "stream.rate_control_not_cbr";
pub const ENC_KEYFRAME: &str = "stream.keyframe_interval";
pub const ENC_OVER_PLATFORM: &str = "stream.bitrate_over_platform_limit";
pub const ENC_LOW_FOR_RES: &str = "stream.bitrate_low_for_resolution";
pub const ENC_OVER_UPLOAD: &str = "stream.bitrate_over_upload";
pub const ENC_AUDIO_LOW: &str = "stream.audio_bitrate_low";
pub const REC_MP4: &str = "stream.recording_plain_mp4";
pub const NET_WIFI: &str = "stream.on_wifi";

// Snapshot diff (something changed since last launch)
pub const CHANGED_DEFAULT_MIC: &str = "system.changed.default_mic";
pub const CHANGED_OBS_MIC_MISSING: &str = "system.changed.obs_mic_missing";
pub const CHANGED_SAMPLE_RATE: &str = "system.changed.sample_rate";
pub const CHANGED_ENHANCEMENTS: &str = "system.changed.enhancements_back_on";
pub const CHANGED_LEVEL: &str = "system.changed.mic_level";
pub const CHANGED_DUCKING: &str = "system.changed.ducking";
pub const CHANGED_OBS_FILTERS: &str = "system.changed.obs_mic_filters";
pub const CHANGED_STUDIO_EFFECTS: &str = "system.changed.studio_effects_back_on";
