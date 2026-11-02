//! Capture endpoints (MMDevice API for ids/defaults, cpal for streaming).

use super::com_init;
use crate::platform::{parse_usb_ids, AudioBackend, AudioDevice, SampleSink, StreamHandle};
use coach_core::audio::AudioBuffer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc;
use windows::core::PWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};

pub struct WinAudio;

pub(crate) fn pwstr_take(p: PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

pub(crate) fn device_id(d: &IMMDevice) -> Option<String> {
    unsafe { d.GetId().ok().map(pwstr_take) }
}

pub(crate) fn friendly_name(d: &IMMDevice) -> Option<String> {
    unsafe {
        let store = d.OpenPropertyStore(STGM_READ).ok()?;
        let mut pv = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let s = PropVariantToStringAlloc(&pv).ok().map(pwstr_take);
        let _ = PropVariantClear(&mut pv);
        s
    }
}

/// (sample rate, channels) of the shared-mode mix format.
pub(crate) fn mix_format(d: &IMMDevice) -> Option<(u32, u16)> {
    unsafe {
        let client: IAudioClient = d.Activate(CLSCTX_ALL, None).ok()?;
        let fmt = client.GetMixFormat().ok()?;
        let r = ((*fmt).nSamplesPerSec, (*fmt).nChannels);
        CoTaskMemFree(Some(fmt as _));
        Some(r)
    }
}

pub(crate) fn enumerator() -> Option<IMMDeviceEnumerator> {
    com_init();
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() }
}

/// All active capture endpoints with their default roles.
pub(crate) fn capture_endpoints() -> Vec<(IMMDevice, AudioDevice)> {
    let Some(en) = enumerator() else { return Vec::new() };
    unsafe {
        let def = en.GetDefaultAudioEndpoint(eCapture, eConsole).ok().and_then(|d| device_id(&d));
        let comms = en.GetDefaultAudioEndpoint(eCapture, eCommunications).ok().and_then(|d| device_id(&d));
        let Ok(coll) = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return Vec::new() };
        let n = coll.GetCount().unwrap_or(0);
        let mut out = Vec::new();
        for i in 0..n {
            let Ok(d) = coll.Item(i) else { continue };
            let Some(id) = device_id(&d) else { continue };
            let name = friendly_name(&d).unwrap_or_else(|| "Microphone".into());
            let (sr, ch) = mix_format(&d).unwrap_or((48_000, 2));
            let usb = instance_usb_ids(&id);
            out.push((
                d,
                AudioDevice { is_default: def.as_deref() == Some(&id), is_default_comms: comms.as_deref() == Some(&id), id, name, channels: ch, sample_rate: sr, usb },
            ));
        }
        out
    }
}

/// USB VID/PID of an endpoint via its device-instance registry entry.
fn instance_usb_ids(endpoint_id: &str) -> Option<(u16, u16)> {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
    let guid = endpoint_id.rsplit('.').next()?;
    let k = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\{guid}\Properties"))
        .ok()?;
    for (name, _) in k.enum_values().flatten() {
        if let Ok(v) = k.get_value::<String, _>(&name) {
            if let Some(ids) = parse_usb_ids(&v) {
                return Some(ids);
            }
        }
    }
    None
}

impl AudioBackend for WinAudio {
    fn list_inputs(&self) -> Vec<AudioDevice> {
        capture_endpoints().into_iter().map(|(_, d)| d).collect()
    }

    fn start_capture(&self, device_id: &str, mut sink: SampleSink) -> Result<Box<dyn StreamHandle>, String> {
        let name = self.list_inputs().into_iter().find(|d| d.id == device_id).map(|d| d.name).ok_or("That microphone isn't connected any more.")?;
        // cpal streams aren't Send: own the stream on a dedicated thread.
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        std::thread::spawn(move || {
            let host = cpal::default_host();
            let dev = host.input_devices().ok().and_then(|mut it| it.find(|d| d.name().map(|n| n == name).unwrap_or(false)));
            let Some(dev) = dev else {
                let _ = ready_tx.send(Err("Couldn't open that microphone.".into()));
                return;
            };
            let cfg = match dev.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("Couldn't open that microphone ({e}).")));
                    return;
                }
            };
            let ch = cfg.channels();
            let sr = cfg.sample_rate().0;
            let err_fn = |e| eprintln!("capture error: {e}");
            let stream = match cfg.sample_format() {
                cpal::SampleFormat::F32 => dev.build_input_stream(&cfg.into(), move |d: &[f32], _: &_| sink(d, ch, sr), err_fn, None),
                cpal::SampleFormat::I16 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[i16], _: &_| {
                        let f: Vec<f32> = d.iter().map(|v| *v as f32 / 32768.0).collect();
                        sink(&f, ch, sr)
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::I32 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[i32], _: &_| {
                        let f: Vec<f32> = d.iter().map(|v| *v as f32 / 2147483648.0).collect();
                        sink(&f, ch, sr)
                    },
                    err_fn,
                    None,
                ),
                other => {
                    let _ = ready_tx.send(Err(format!("Unsupported microphone format {other:?}.")));
                    return;
                }
            };
            match stream.and_then(|s| s.play().map(|_| s).map_err(|e| cpal::BuildStreamError::BackendSpecific { err: cpal::BackendSpecificError { description: e.to_string() } })) {
                Ok(s) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stop_rx.recv();
                    drop(s);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("Couldn't start recording ({e}). Is another app using the mic exclusively?")));
                }
            }
        });
        ready_rx.recv().map_err(|_| "Couldn't start recording.".to_string())??;
        Ok(Box::new(ThreadHandle(Some(stop_tx))))
    }

    fn play(&self, buf: AudioBuffer) -> Result<Box<dyn StreamHandle>, String> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        std::thread::spawn(move || {
            let host = cpal::default_host();
            let Some(dev) = host.default_output_device() else {
                let _ = ready_tx.send(Err("No speakers or headphones found.".into()));
                return;
            };
            let Ok(cfg) = dev.default_output_config() else {
                let _ = ready_tx.send(Err("Couldn't open your speakers.".into()));
                return;
            };
            let out_ch = cfg.channels() as usize;
            let out_sr = cfg.sample_rate().0 as f64;
            // Linear resample + mono/stereo mapping.
            let src = &buf.channels;
            let ratio = buf.sample_rate as f64 / out_sr;
            let n_out = (buf.len() as f64 / ratio) as usize;
            let mut pos = 0usize;
            let data: Vec<Vec<f32>> = src.clone();
            let get = move |c: usize, i: f64| -> f32 {
                let ch = &data[c.min(data.len() - 1)];
                let a = i.floor() as usize;
                let t = (i - a as f64) as f32;
                let x0 = ch.get(a).copied().unwrap_or(0.0);
                let x1 = ch.get(a + 1).copied().unwrap_or(x0);
                x0 + (x1 - x0) * t
            };
            let stream = dev.build_output_stream(
                &cpal::StreamConfig { channels: cfg.channels(), sample_rate: cfg.sample_rate(), buffer_size: cpal::BufferSize::Default },
                move |out: &mut [f32], _: &_| {
                    for frame in out.chunks_mut(out_ch) {
                        let i = pos as f64 * ratio;
                        for (c, s) in frame.iter_mut().enumerate() {
                            *s = if pos < n_out { get(c, i) } else { 0.0 };
                        }
                        pos += 1;
                    }
                },
                |e| eprintln!("playback error: {e}"),
                None,
            );
            match stream {
                Ok(s) if s.play().is_ok() => {
                    let _ = ready_tx.send(Ok(()));
                    let secs = n_out as f64 / out_sr + 0.3;
                    let _ = stop_rx.recv_timeout(std::time::Duration::from_secs_f64(secs));
                    drop(s);
                }
                _ => {
                    let _ = ready_tx.send(Err("Couldn't play the preview.".into()));
                }
            }
        });
        ready_rx.recv().map_err(|_| "Couldn't play the preview.".to_string())??;
        Ok(Box::new(ThreadHandle(Some(stop_tx))))
    }
}

struct ThreadHandle(Option<mpsc::Sender<()>>);

impl StreamHandle for ThreadHandle {
    fn stop(mut self: Box<Self>) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ThreadHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}
