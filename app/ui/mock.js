// Built-in mock backend: realistic sample data for every command and event in
// API.md, so the UI can be designed and tested in a plain browser.

import { mockPlan, createCoachSim, drawCoachOverlay } from './mock-coach.js';

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

function finding(id, area, severity, title, why, fix, fix_kind, extra = {}) {
  return {
    id, area, severity, title, why, fix, fix_kind,
    can_decline: fix_kind === 'manual',
    evidence: {}, suggest_product: null, ...extra,
  };
}

const MIC_PROFILE = {
  id: 'generic_xlr_dynamic', brand: 'Generic', model: 'XLR dynamic mic', family: 'generic', aliases: [],
  generic_class: 'xlr_dynamic', connection: { type: 'xlr' }, capsule: 'dynamic',
};
const SM7B = { id: 'shure_sm7b', brand: 'Shure', model: 'SM7B', family: 'sm7', aliases: ['SM7B'], connection: { type: 'xlr' }, capsule: 'dynamic' };

// Mics for the 'What mic is plugged into this?' picker (shape of MicProfileOption in API.md).
const MIC_OPTIONS = [
  ['ev_re20', 'Electro-Voice', 'RE20', 'xlr', 'dynamic', ['RE-20']],
  ['shure_sm7b', 'Shure', 'SM7B', 'xlr', 'dynamic', ['SM7']],
  ['shure_sm58', 'Shure', 'SM58', 'xlr', 'dynamic', []],
  ['shure_mv7', 'Shure', 'MV7', 'usb_xlr', 'dynamic', []],
  ['rode_podmic', 'Rode', 'PodMic', 'xlr', 'dynamic', []],
  ['rode_nt1a', 'Rode', 'NT1-A', 'xlr', 'condenser', ['NT1A']],
  ['audio_technica_at2020', 'Audio-Technica', 'AT2020', 'xlr', 'condenser', []],
  ['blue_yeti', 'Blue', 'Yeti', 'usb', 'condenser', []],
  ['hyperx_quadcast', 'HyperX', 'QuadCast', 'usb', 'condenser', []],
].map(([id, brand, model, connection, capsule, aliases]) => ({
  id, brand, model, display_name: `${brand} ${model}`, family: id, connection, capsule, verified: false, aliases,
}));
// What the user said is behind each device id (the backend keeps this in setup.json).
const micChoices = {};
const MIC_FACTS = { ev_re20: { gain_needed_db: 55, distance_cm: [3, 15], address: 'end' }, shure_sm7b: { gain_needed_db: 60, distance_cm: [3, 10], address: 'end' } };

const ENVIRONMENT = {
  app_version: '0.1.0-mock',
  windows_build: '26200.8524',
  obs: { running: true, connected: true, version: '33.0.1', websocket_enabled: true, error: null },
  mics: [
    {
      id: '{0.0.1.00000000}.{goxlr-chat}', name: 'Chat Mic (TC-Helicon GoXLR Mini)', is_default: false,
      is_default_comms: false, channels: 2, sample_rate: 48000, needs_mic_choice: true, mic_choice: null,
      profile: {
        profile: MIC_PROFILE, confidence: 'generic', chosen: false,
        needs_questions: [
          { id: 'which_mic_on_interface', prompt: 'Which mic is plugged into your interface?', photo_hint: null, options: [] },
          { id: 'booster_in_chain', prompt: 'Is there a small box (like a Cloudlifter or FetHead) between the mic and the interface?', photo_hint: 'booster', options: ['Yes', 'No', "I'm not sure"] },
        ],
      },
    },
    {
      id: '{0.0.1.00000000}.{focusrite-in-1}', name: 'Analogue 1 + 2 (Focusrite USB Audio)', is_default: true,
      is_default_comms: false, channels: 2, sample_rate: 48000, needs_mic_choice: true, mic_choice: null,
      profile: {
        profile: MIC_PROFILE, confidence: 'generic',
        needs_questions: [
          { id: 'which_mic_on_interface', prompt: 'Which mic is plugged into your Focusrite?', photo_hint: null,
            options: ['Shure SM7B', 'Rode PodMic', 'Electro-Voice RE20', 'Shure SM58', 'Rode NT1-A', 'Something else'] },
          { id: 'booster_in_chain', prompt: 'Is there a small box (like a Cloudlifter or FetHead) between the mic and the interface?', photo_hint: 'booster',
            options: ['Yes', 'No', "I'm not sure"] },
        ],
      },
    },
    {
      id: '{0.0.1.00000000}.{c920-mic}', name: 'Microphone (HD Pro Webcam C920)', is_default: false,
      is_default_comms: true, channels: 2, sample_rate: 48000, needs_mic_choice: false, mic_choice: null,
      profile: { profile: { id: 'generic_webcam_mic', brand: 'Generic', model: 'Webcam mic', family: 'generic', aliases: [] }, confidence: 'generic', needs_questions: [] },
    },
  ],
  cameras: [
    { id: 'usb#vid_046d&pid_082d', name: 'HD Pro Webcam C920', obs_source: 'Webcam',
      profile: { profile: { id: 'logitech_c920', brand: 'Logitech', model: 'C920', family: 'c920', class: 'full_uvc', aliases: [] }, confidence: 'exact', needs_questions: [] } },
    { id: 'usb#vid_0fd9&pid_0066', name: 'Cam Link 4K', obs_source: null,
      profile: { profile: { id: 'elgato_cam_link_4k', brand: 'Elgato', model: 'Cam Link 4K', family: 'cam_link', class: 'capture_card', aliases: [] }, confidence: 'exact',
        needs_questions: [{ id: 'which_camera_body', prompt: 'Which camera is connected to your Cam Link?', photo_hint: null, options: ['Sony a6000', 'Sony ZV-E10', 'Canon M50', 'Something else'] }] } },
  ],
  saved_profile: null,
};

/** A device as the environment shows it, with the user's 'what mic is plugged in' answer applied. */
function withChoice(dev) {
  const c = micChoices[dev.id] || null;
  const asks = dev.id.includes('goxlr') || dev.id.includes('focusrite');
  const out = { ...dev, mic_choice: c, needs_mic_choice: asks && !c };
  if (c && c.status === 'chosen') {
    const p = MIC_OPTIONS.find((o) => o.id === c.profile_id);
    out.profile = {
      profile: { id: p.id, brand: p.brand, model: p.model, family: p.family, aliases: p.aliases }, confidence: 'exact', chosen: true,
      needs_questions: p.id === 'ev_re20' ? [{ id: 'switch_positions', prompt: 'How are the switches on your mic set?', photo_hint: null, options: ['Bass tilt (low-cut) switch', 'Flat'] }] : [],
    };
  }
  return out;
}

const QUICK = {
  findings: [
    finding('mic.left_channel_only', 'mic', 'critical', 'Viewers only hear you in their left ear',
      'Your interface sends input 1 as one side of a stereo signal, so the other side is silent.',
      "We'll switch your mic to mono in OBS so both ears get your voice.", 'auto'),
    finding('system.voice_clarity_on', 'system', 'major', 'Windows is changing your voice',
      "Windows 'Voice Clarity' is an AI effect that can make you sound muffled or warbly.",
      "We'll turn it off for your mic. Your voice stays natural.", 'auto'),
    finding('mic.ai_noise_suppression', 'mic', 'major', 'An AI noise filter is on your mic in OBS',
      'AI noise suppression can make you sound robotic or underwater when it works hard.',
      "We'll remove it. Your test will tell us if the room actually needs anything.", 'auto'),
    finding('system.sample_rate_mismatch', 'system', 'minor', 'Your mic and OBS run at different speeds',
      'Windows uses 44.1 kHz for your mic while OBS uses 48 kHz. That can cause crackles and drift.',
      'Set your mic to 48 kHz in Windows Sound settings. We\'ll open the right page for you.', 'guided'),
    finding('system.windows_ducking_on', 'system', 'minor', 'Windows turns your game down when Discord is open',
      "The 'Reduce other sounds by 80%' setting is on.", "We'll set it to 'Do nothing'.", 'auto'),
    finding('system.exclusive_mode_allowed', 'system', 'minor', 'Apps can take over your mic',
      '"Allow applications to take exclusive control" lets a game or app lock the mic so OBS or Discord suddenly hear nothing.',
      'Turn off exclusive control for this mic in Windows Sound settings.', 'guided'),
    finding('stream.on_wifi', 'stream', 'minor', "You're streaming over Wi-Fi",
      'Wi-Fi can drop out for a moment, which shows up as dropped frames for viewers.',
      'Use a network cable if you can.', 'manual'),
  ],
  actions: [
    { id: 'a1', label: 'Switch your OBS mic to mono', resolves: ['mic.left_channel_only'], needs_admin: false, needs_obs_closed: false, action: { type: 'set_obs_input_mono', input: 'Mic/Aux' } },
    { id: 'a2', label: "Turn off Windows 'Voice Clarity' for your mic", resolves: ['system.voice_clarity_on'], needs_admin: false, needs_obs_closed: false, action: { type: 'disable_voice_clarity', endpoint: '{0.0.1}' } },
    { id: 'a3', label: 'Remove the AI noise filter in OBS', resolves: ['mic.ai_noise_suppression'], needs_admin: false, needs_obs_closed: false, action: { type: 'remove_obs_filter', input: 'Mic/Aux', filter: 'Noise Suppression' } },
    { id: 'a4', label: "Set Windows ducking to 'Do nothing'", resolves: ['system.windows_ducking_on'], needs_admin: false, needs_obs_closed: false, action: { type: 'set_windows_ducking', preference: 'do_nothing' } },
  ],
  score: [
    { area: 'system', value: 76, issues: 3 },
    { area: 'mic', value: 63, issues: 2 },
    { area: 'stream', value: 95, issues: 1 },
  ],
};

const CONSTRAINT_TEXT = {
  cant_move_mic_closer: ['mic.too_far', 'Got it — we\'ve tuned for the distance: gentler compression (so the room isn\'t pulled up), a tighter expander and a little clarity boost.'],
  cant_add_clean_gain: ['mic.not_enough_gain', "Got it — we're using your interface's max clean gain plus digital gain, with an expander to hide hiss between sentences."],
  cant_fix_plosives: ['mic.plosives', 'Got it — we\'ve raised the low-cut filter a little to soften pops.'],
  cant_remove_noise_source: ['mic.background_noise', 'Got it — we\'ve set a stronger expander and low-cut so the noise drops out between sentences.'],
};

function micFindings(constraints, after = false) {
  let list = after
    ? [
        finding('mic.too_far', 'mic', 'major', 'You sound far away',
          "The mic hears a lot of room echo compared to your voice, which happens when it's too far from your mouth.",
          'Bring the mic to 3–10 cm (about a fist away) and point it at your mouth.', 'manual', { suggest_product: 'boom_arm', evidence: { reverb_decay_s: 0.52 } }),
      ]
    : [
        finding('mic.not_enough_gain', 'mic', 'major', 'Shure SM7B needs more power than your interface can give',
          'This mic needs about 60 dB of clean gain; your setup is about 4 dB short. Turning it up all the way adds hiss.',
          'Talk closer (3–10 cm). For the full fix, add an inline mic booster (about +25 dB) or use an interface with more gain.', 'manual',
          { suggest_product: 'inline_booster', evidence: { gain_shortfall_db: 4 } }),
        finding('mic.too_far', 'mic', 'major', 'You sound far away',
          "The mic hears a lot of room echo compared to your voice, which happens when it's too far from your mouth.",
          'Bring the mic to 3–10 cm (about a fist away) and point it at your mouth. This is the single biggest improvement for most setups.', 'manual',
          { suggest_product: 'boom_arm', evidence: { reverb_decay_s: 0.58 } }),
        finding('mic.muddy', 'mic', 'minor', 'Your voice sounds muddy', 'There\'s a build-up in the low-mids that blurs your words.',
          "Back off slightly from the mic if you're very close. We'll gently cut the muddy range.", 'auto', { evidence: { mud_db: 4.6 } }),
        finding('mic.plosives', 'mic', 'minor', "Your P's and B's pop", "Puffs of air from 'p' and 'b' hit the mic directly.",
          "Angle the mic slightly to the side of your mouth (20–45°) or add a pop filter. We'll add a low-cut filter too.", 'manual',
          { suggest_product: 'pop_filter', evidence: { events: 3 } }),
        finding('mic.keyboard', 'mic', 'minor', 'Your keyboard is too loud', 'The mic hears your typing almost as loud as your voice.',
          "Move the mic closer to your mouth and point its back at the keyboard. A boom arm helps. We'll quiet the gaps.", 'manual',
          { suggest_product: 'boom_arm', evidence: { typing_rel_db: -16 } }),
      ];
  for (const c of constraints) {
    const t = CONSTRAINT_TEXT[c];
    if (!t) continue;
    list = list.map((f) => (f.id === t[0] ? { ...f, severity: 'info', fix: t[1], fix_kind: 'auto', can_decline: false } : f));
  }
  const rank = { critical: 3, major: 2, minor: 1, info: 0 };
  list.sort((a, b) => rank[b.severity] - rank[a.severity]);
  return list;
}

function scoreOf(area, findings) {
  const w = { critical: 25, major: 12, minor: 5, info: 0 };
  const ws = findings.filter((f) => f.area === area).map((f) => w[f.severity]).filter((x) => x > 0).sort((a, b) => b - a);
  let p = 0;
  ws.forEach((x, i) => { p += x * Math.pow(0.85, i); });
  return { area, value: Math.round(Math.max(5, 100 - p)), issues: ws.length };
}

function micMetrics(after) {
  return after
    ? { speech_peak_dbfs: -6.1, noise_floor_dbfs: -74.2, snr_db: 52.1, speech_lufs: -16.4, reverb_decay_s: 0.52, f0_hz: 118, loud_peak_dbfs: -1.6, clipped_fraction: 0, dead_channel: null, upper_bandwidth_hz: 10000 }
    : { speech_peak_dbfs: -27.3, noise_floor_dbfs: -61.8, snr_db: 30.4, speech_lufs: -36.9, reverb_decay_s: 0.58, f0_hz: 118, loud_peak_dbfs: -14.2, clipped_fraction: 0, dead_channel: 1, upper_bandwidth_hz: 10000 };
}

function targets(after) {
  const m = micMetrics(after);
  const st = (ok, close) => (ok ? 'met' : close ? 'close' : 'missed');
  return [
    { id: 'speech_peaks', label: 'Normal talking level', value: m.speech_peak_dbfs, target: '−18 to −12 dBFS', status: after ? 'met' : 'missed' },
    { id: 'noise_floor', label: 'Background noise', value: m.noise_floor_dbfs, target: '≤ −60 dBFS', status: st(m.noise_floor_dbfs <= -60, true) },
    { id: 'snr', label: 'Voice above noise', value: m.snr_db, target: '≥ 50 dB', status: st(m.snr_db >= 50, m.snr_db >= 40) },
    { id: 'one_ear', label: 'Both ears', value: after ? 0 : 1, target: 'both channels', status: after ? 'met' : 'missed' },
    { id: 'room', label: 'Room echo', value: m.reverb_decay_s, target: '≤ 0.35 s', status: 'close' },
    { id: 'voice_loudness', label: 'Voice loudness', value: m.speech_lufs, target: '≈ −16 LUFS', status: after ? 'met' : 'missed' },
  ];
}

function chain(constraints) {
  const far = constraints.includes('cant_move_mic_closer');
  return {
    mono_downmix: true,
    hardware_gain_change_db: constraints.includes('cant_add_clean_gain') ? 0 : 6.5,
    input_gain_db: constraints.includes('cant_add_clean_gain') ? 9.0 : 0,
    highpass_hz: constraints.includes('cant_fix_plosives') ? 100 : 90,
    highpass_slope_db_oct: 12,
    eq: [
      { kind: 'peak', freq_hz: 315, gain_db: -3.2, q: 1, reason: 'less mud' },
      ...(far ? [{ kind: 'peak', freq_hz: 3150, gain_db: 2.5, q: 0.8, reason: 'more clarity' }] : []),
    ],
    expander: { threshold_db: -52, ratio: far ? 4 : 2.5, attack_ms: 2, release_ms: 150 },
    compressor: { threshold_db: -30.5, ratio: far ? 2.5 : 3, attack_ms: 3, release_ms: 100, makeup_db: 14.2 },
    deesser: null,
    limiter: { ceiling_db: -1.5, release_ms: 60 },
    target_lufs: -16,
    notes: far ? ['Mic stays where it is: no bass boost (it would boost room mud), small clarity lift instead.'] : [],
  };
}

function micReport(constraints, after = false) {
  const findings = micFindings(constraints, after);
  return { metrics: micMetrics(after), findings, chain: chain(constraints), score: scoreOf('mic', findings), targets: targets(after) };
}

function cameraReport(constraints, cameraId) {
  const capture = cameraId && cameraId.includes('0fd9');
  let findings = [
    finding('camera.backlit', 'camera', 'major', "You're darker than the window behind you",
      'A bright window or lamp behind you makes the camera darken the whole picture, including your face.',
      'Turn so the window is beside or in front of you, or close the blinds. A light in front of you helps most.', 'manual', { suggest_product: 'key_light' }),
    finding('camera.auto_exposure_on', 'camera', 'minor', 'Your brightness changes when you move',
      "Auto exposure keeps re-adjusting, so your face 'breathes' brighter and darker.", "We'll lock the exposure for your room.", 'auto'),
    finding('camera.colour_cast', 'camera', 'minor', 'Your picture looks a bit orange',
      'Warm bulbs make everything orange, and auto white balance drifts as the scene changes.', "We'll lock white balance using your white paper.", 'auto'),
    finding('camera.camera_low', 'camera', 'minor', 'Your camera looks up at you',
      'A camera below eye level shows more chin and ceiling.', 'Raise the camera to eye level — a stack of books under it works.', 'manual'),
  ];
  const map = { cant_move_backlight: 'camera.backlit', cant_move_camera: 'camera.camera_low' };
  for (const c of constraints) {
    if (map[c]) findings = findings.map((f) => (f.id === map[c] ? { ...f, severity: 'info', fix: "Got it — we've adjusted the settings to work around this.", fix_kind: 'auto', can_decline: false } : f));
  }
  const settings = capture
    ? { class: 'capture_card', uvc: null, format: { width: 1920, height: 1080, fps: 30, format: 'unknown' }, light_limited: false,
        coach_steps: [
          'Set the camera to movie mode (M) so you control exposure.',
          'Shutter speed: 1/50 (you are on 50 Hz power) to avoid flicker.',
          'ISO: as low as possible — start at 400 and only go up if needed.',
          'White balance: Kelvin 5000 K (or set it from your white paper).',
          'Menu → Setup → HDMI Settings → HDMI Info. Display: Off (clean picture).',
          'Auto Power Off Temp: High, and use a dummy battery so it doesn\'t shut off mid-stream.',
          'Focus: AF-C with face detection on.',
        ], notes: [] }
    : { class: 'full_uvc',
        uvc: { auto_exposure: false, ae_priority: false, exposure_log2: -5.64, target_shutter_s: 0.02, shutter_exact: true, gain: 32, auto_wb: false, wb_kelvin: 4300,
               auto_focus: false, focus: 30, sharpness: 110, saturation: 120, backlight_comp: constraints.includes('cant_move_backlight') ? 1 : 0, powerline_hz: 50, hdr: null },
        format: { width: 1920, height: 1080, fps: 30, format: 'mjpeg' }, light_limited: constraints.includes('cant_add_light'),
        coach_steps: ['Put your main light in front of you, slightly above eye level.', 'Close the blinds behind you, or turn your desk so the window is to the side.'],
        notes: constraints.includes('cant_add_light') ? ['Your room is dim, so we chose 30 fps and a bit more gain for a brighter picture.'] : [] };
  return {
    metrics: { frames: 120, face_found: true, paper_found: true, face_vs_background_stops: capture ? 0.6 : -1.4, paper_cast_pct: 9.5, noise_sigma: 3.1, face_sharpness: 182, delivered_fps: 30 },
    findings, settings, score: scoreOf('camera', findings), class: capture ? 'capture_card' : 'full_uvc',
  };
}

function healthReport() {
  const findings = [
    finding('stream.network_drops', 'stream', 'minor', 'Your internet dropped some frames',
      'For a short moment your upload couldn\'t keep up with your bitrate.', 'Use a network cable, or lower your bitrate a little.', 'manual'),
  ];
  return {
    duration_s: 5420, network: { lost: 312, total: 325200, percent: 0.096 }, encoding: { lost: 0, total: 325200, percent: 0 },
    rendering: { lost: 41, total: 325200, percent: 0.013 }, avg_bitrate_kbps: 5890, min_bitrate_kbps: 2310, bitrate_variation: 0.08,
    reconnect_events: 0, avg_fps: 59.9, peak_cpu_percent: 61, avg_cpu_percent: 34,
    worst_moments: [{ offset_ms: 2_412_000, kind: 'Network', loss_percent: 2.4 }],
    findings, score: scoreOf('stream', findings),
  };
}

export function createMock() {
  const listeners = new Map();
  const emit = (ev, payload) => (listeners.get(ev) || []).forEach((h) => h(payload));
  let micStep = null;
  let micTimer = null;
  let camTimer = null;
  let camStep = 'neutral';
  let healthTimer = null;
  let healthT = 0;
  let healthBytes = 0;
  let lastCameraId = null;
  let constraints = [];
  let obsPolls = 0;          // obs_running says true for the first polls, then the 'user' has closed OBS
  let obsRunning = true;
  let mockAbError = null;
  try { mockAbError = new URLSearchParams(location.search).get('mock_ab_error'); } catch { /* not in a browser */ }
  let abFinal = null;
  const ago = (h) => new Date(Date.now() - h * 3600e3).toISOString();
  const bi = (kind, label) => ({ kind, label });
  const backups = [
    { id: 'original', created_at: ago(72), label: 'How it was before Setup Coach', items: [bi('obs_filters', 'OBS: filters on "Mic/Aux"'), bi('windows_setting', 'Windows: Voice Clarity'), bi('windows_setting', 'Windows: audio enhancements'), bi('camera', 'Camera: HD Pro Webcam C920 settings'), bi('obs_scene_file', 'OBS: settings file Stream.json')] },
    { id: '20261104T081500Z-000', created_at: ago(2), label: 'Before setting up your Yeti Stereo Microphone', items: [bi('obs_filters', 'OBS: filters on "Mic/Aux"'), bi('voice_chain', 'Setup Coach voice settings')] },
    { id: '20261104T080200Z-000', created_at: ago(2.3), label: 'Before: Turn off Windows audio enhancements', items: [bi('windows_setting', 'Windows: audio enhancements'), bi('windows_setting', 'Windows: Voice Clarity')] },
    { id: '20261103T190000Z-000', created_at: ago(20), label: 'Before changing your HD Pro Webcam C920 settings', items: [bi('camera', 'Camera: HD Pro Webcam C920 settings')] },
  ];
  const abInfo = () => ({
    original: { path: 'original.wav', lufs: -36.9, score: 38 },
    final: abFinal,
    matched_gain_db: abFinal ? 20.5 : 0,
    note: null,
    findings: abFinal ? [
      finding('mic.roomy', 'mic', 'minor', 'Your room adds some echo', 'Hard walls bounce your voice back to the mic.', 'Hang something soft behind you, or sit closer to the mic.', 'manual'),
      finding('mic.plosives', 'mic', 'minor', 'Some p and b sounds pop', 'Bursts of air from p and b hit the mic.', 'Speak slightly across the mic, or add a pop filter.', 'manual'),
    ] : [],
  });
  const abErr = (code, message) => Object.assign(new Error(message), { code });
  let saved = null;
  const coached = { mic: false, camera: false };
  const coachSim = createCoachSim((ev, payload) => emit(ev, payload), drawCoachOverlay);

  function startMicLevels() {
    clearInterval(micTimer);
    let t = 0;
    micTimer = setInterval(() => {
      t += 0.05;
      let rms = -75 + Math.random() * 3;
      let peak = rms + 4;
      const k = micStep;
      if (k === 'read' || k === 'cable_wiggle' || k === 'moving') {
        const syl = Math.max(0, Math.sin(t * 9) * Math.sin(t * 2.3 + 1));
        rms = -48 + syl * 26 + Math.random() * 3 - (k === 'moving' ? Math.max(0, Math.sin(t)) * 8 : 0);
        peak = rms + 10 + Math.random() * 3;
      } else if (k === 'loud') {
        rms = -30 + Math.abs(Math.sin(t * 6)) * 20;
        peak = rms + 9;
      } else if (k === 'typing') {
        const click = Math.random() < 0.25;
        rms = click ? -48 : -72;
        peak = rms + (click ? 14 : 4);
      } else if (k === 'desk_tap') {
        const tap = Math.random() < 0.08;
        rms = tap ? -36 : -74;
        peak = rms + 12;
      }
      emit('mic-level', { rms_db: rms, peak_db: Math.min(0, peak), clipped: peak >= -0.1 });
    }, 50);
  }

  function frame() {
    const c = document.createElement('canvas');
    c.width = 480; c.height = 270;
    const g = c.getContext('2d');
    const backlit = !constraints.includes('cant_move_backlight');
    // Room
    const grad = g.createLinearGradient(0, 0, 480, 0);
    grad.addColorStop(0, '#3a3530'); grad.addColorStop(1, '#2a2622');
    g.fillStyle = grad; g.fillRect(0, 0, 480, 270);
    // Window behind
    g.fillStyle = backlit ? '#f3f0e6' : '#9fb3c8';
    g.fillRect(300, 30, 140, 150);
    g.fillStyle = '#4a4036'; g.fillRect(366, 30, 6, 150); g.fillRect(300, 102, 140, 6);
    // Person
    const sway = camStep === 'turn_head' ? Math.sin(Date.now() / 400) * 18 : 0;
    const scale = camStep === 'lean_in_out' ? 1 + Math.sin(Date.now() / 500) * 0.12 : 1;
    g.save(); g.translate(240 + sway, 160); g.scale(scale, scale);
    g.fillStyle = '#1d2333'; g.beginPath(); g.ellipse(0, 110, 120, 70, 0, 0, Math.PI * 2); g.fill();
    g.fillStyle = backlit ? '#7a5a46' : '#b98a6c'; g.beginPath(); g.ellipse(0, -10, 46, 58, 0, 0, Math.PI * 2); g.fill();
    g.fillStyle = '#2b1f18'; g.beginPath(); g.ellipse(0, -52, 50, 28, 0, Math.PI, Math.PI * 2); g.fill();
    g.restore();
    if (camStep === 'hold_paper') {
      g.fillStyle = backlit ? '#d9d4c4' : '#f4f1ea';
      g.fillRect(180, 92, 120, 92);
    }
    g.fillStyle = 'rgba(255,255,255,0.05)';
    for (let i = 0; i < 300; i++) g.fillRect(Math.random() * 480, Math.random() * 270, 1, 1);
    return c.toDataURL('image/jpeg', 0.8);
  }

  const handlers = {
    async get_environment() {
      await wait(600);
      return { ...ENVIRONMENT, mics: ENVIRONMENT.mics.map(withChoice), saved_profile: saved };
    },
    async list_mic_profiles() { await wait(150); return MIC_OPTIONS; },
    async get_mic_behind({ deviceId }) { return micChoices[deviceId] || null; },
    async set_mic_behind({ deviceId, choice }) {
      await wait(250);
      const dev = ENVIRONMENT.mics.find((m) => m.id === deviceId);
      if (!dev) throw 'That microphone isn\'t connected any more.';
      // ?fail=mic simulates the save failing, to check the picker's error + retry in a browser.
      if (/[?&]fail=mic\b/.test(location.search) && !window.__micRetried) { window.__micRetried = true; throw "We couldn't save a backup of your current settings, so nothing was changed."; }
      const base = { device_name: dev.name, chosen_at: new Date().toISOString() };
      if (choice.kind === 'clear') delete micChoices[deviceId];
      else if (choice.kind === 'profile') {
        const p = MIC_OPTIONS.find((o) => o.id === choice.profile_id);
        if (!p) throw 'We don\'t know that microphone. Pick another one from the list, or choose "Other".';
        micChoices[deviceId] = { status: 'chosen', profile_id: p.id, display_name: p.display_name, ...base };
      } else micChoices[deviceId] = { status: choice.kind === 'other' ? 'other' : 'skipped', profile_id: null, display_name: null, ...base };
      const m = withChoice(dev);
      return { mic_choice: m.mic_choice, profile: m.profile, needs_mic_choice: m.needs_mic_choice };
    },
    async connect_obs() { await wait(400); return ENVIRONMENT.obs; },
    async run_quick_checks() {
      await wait(900);
      // ?fail=scan simulates a backend error so the error banner + retry can be checked in a browser.
      if (/[?&]fail=scan\b/.test(location.search) && !window.__scanRetried) { window.__scanRetried = true; throw 'The scan hit an unexpected problem. Please try again.'; }
      return QUICK;
    },
    async log_ui(entry) { (window.__uiLog ||= []).push(entry); return null; },
    async apply_actions({ actionIds }) {
      await wait(900);
      // ?fail=apply simulates every fix failing (0 fixed).
      if (/[?&]fail=apply\b/.test(location.search)) {
        return { applied: [], failed: actionIds.map((id) => ({ id, error: "Windows didn't allow that change." })), restart_obs_needed: false, skipped: [] };
      }
      const failed = actionIds.includes('a4') ? [{ id: 'a4', error: 'Windows needs you to confirm this one. We opened the Sound settings page for you.' }] : [];
      return { applied: actionIds.filter((a) => !failed.find((f) => f.id === a)), failed, restart_obs_needed: actionIds.includes('a1') };
    },
    async mic_context({ deviceId, answers }) {
      await wait(200);
      // The named mic (picker) drives the context, like the real backend; a question answer wins.
      const picked = micChoices[deviceId]?.status === 'chosen' ? micChoices[deviceId] : null;
      const sm7 = answers && answers.mic_model_id === 'Shure SM7B';
      const name = sm7 ? 'Shure SM7B' : picked ? picked.display_name : null;
      const facts = (sm7 && MIC_FACTS.shure_sm7b) || (picked && MIC_FACTS[picked.profile_id]) || null;
      return { mic_name: name, capsule: 'dynamic', address: facts?.address || 'end', connection: 'xlr', gain_needed_db: facts ? facts.gain_needed_db : null,
        interface_max_gain_db: 56, booster_db: answers && answers.booster ? 25 : null, gain_at_max: answers ? answers.gain_at_max ?? null : null,
        onboard_auto_level: null, distance_cm: facts ? facts.distance_cm : [3, 10] };
    },
    async start_mic_test() { micStep = null; startMicLevels(); await wait(150); return null; },
    async mark_step({ kind, edge }) { micStep = edge === 'start' ? kind : null; return null; },
    async finish_mic_test({ constraints: cs }) { clearInterval(micTimer); constraints = cs || []; await wait(1600); return micReport(constraints, coached.mic); },
    async rescan_mic({ constraints: cs }) { constraints = cs || []; await wait(700); return micReport(constraints); },
    async apply_mic_chain() { await wait(1100); return { applied: ['vst3_config', 'obs_filters'], failed: [], restart_obs_needed: false }; },
    async verify_mic() { await wait(1400); return { before: micReport(constraints), after: micReport(constraints, true) }; },
    async play_preview() { await wait(100); return null; },
    async stop_preview() { return null; },
    async camera_state() { await wait(200); return { auto_exposure: true, ae_priority: true, auto_wb: true, auto_focus: true, requested_fps: 30, requested_width: 1920, requested_height: 1080, format: 'mjpeg' }; },
    async start_camera_test({ cameraId }) {
      lastCameraId = cameraId; camStep = 'neutral';
      clearInterval(camTimer);
      camTimer = setInterval(() => emit('camera-preview', frame()), 250);
      emit('camera-preview', frame());
      await wait(200);
      return null;
    },
    async capture_camera_step({ step, seconds }) { camStep = step; await wait(Math.min(seconds, 4) * 1000); return { frames: Math.round(seconds * 4), faces_found: Math.round(seconds * 4) - 1 }; },
    async finish_camera_test({ constraints: cs }) {
      clearInterval(camTimer); await wait(1200);
      const r = cameraReport(cs || [], lastCameraId);
      if (coached.camera) { r.findings = r.findings.filter((f) => f.fix_kind === 'auto'); r.score = scoreOf('camera', r.findings); }
      return r;
    },
    async apply_camera_settings() { await wait(800); return { applied: ['uvc'], failed: [], restart_obs_needed: false }; },
    async start_health_monitor() {
      healthT = 0;
      healthBytes = 0;
      clearInterval(healthTimer);
      healthTimer = setInterval(() => {
        healthT += 1000;
        const dropping = healthT >= 3000 && healthT < 9000;
        const kbps = dropping ? 3100 + Math.random() * 400 : 5900 + Math.random() * 200;
        healthBytes += Math.round((kbps * 1000) / 8);
        emit('health-sample', { t_ms: healthT, cpu_percent: 30 + Math.random() * 10, active_fps: 60, avg_frame_render_ms: 4 + Math.random(),
          render_skipped: 0, render_total: healthT * 0.06, output_skipped: 0, output_total: healthT * 0.06,
          network_dropped: dropping ? Math.round(healthT * 0.0005) : 0, network_total: healthT * 0.06, congestion: dropping ? 0.4 : 0.02,
          bytes_sent: healthBytes, reconnecting: false });
        if (healthT === 3000) emit('health-alert', { kind: 'network', raised: true, t_ms: healthT, title: 'Your internet is dropping frames', cause: "Your upload can't keep up with your bitrate right now.", fix: 'Lower your bitrate a little, or pause big downloads on your network.', value: 2.1 });
        if (healthT === 10000) emit('health-alert', { kind: 'network', raised: false, t_ms: healthT, title: 'Internet is back to normal', cause: '', fix: '', value: 0.1 });
      }, 1000);
      await wait(200);
      return null;
    },
    async stop_health_monitor() { clearInterval(healthTimer); await wait(500); return healthReport(); },
    async save_setup({ name }) { await wait(300); saved = { name, saved_at: new Date().toISOString(), mic_chain: chain(constraints), camera_settings: null, camera_id: lastCameraId, obs_input: 'Mic/Aux' }; return saved; },
    async coach_plan({ area }) { await wait(500); coached[area] = true; return mockPlan(area); },
    async start_coach({ step }) { await wait(120); coachSim.start(step); return null; },
    async stop_coach() { coachSim.stop(); return null; },
    async open_for_user() { await wait(300); coachSim.open(); return null; },
    // Settings vault and real A/B recordings (API.md "Settings vault & A/B")
    async list_backups() { await wait(250); return backups.map((b) => ({ ...b })); },
    async restore_backup({ id }) {
      await wait(1100);
      const b = backups.find((x) => x.id === id);
      if (!b) throw new Error("We couldn't find that backup any more.");
      const needsFiles = b.items.some((i) => i.kind === 'obs_scene_file');
      if (needsFiles && obsRunning) {
        obsRunning = false; // as if the user closed OBS now
        return { applied: ['windows:voice_clarity:mic'], failed: [{ id: 'obs_file:basic.ini', error: 'Close OBS, then press Restore again so we can put back its files.' }], restart_obs_needed: false, skipped: [{ id: 'goxlr:S123', error: 'GoXLR Utility is not running, so the GoXLR was left as it is.' }] };
      }
      return { applied: b.items.map((_, i) => 'item' + i), failed: [], restart_obs_needed: needsFiles, skipped: id === 'original' ? [{ id: 'camera:cam1', error: 'Your webcam is not plugged in, so it was left as it is.' }] : [] };
    },
    async obs_running() {
      await wait(150);
      obsPolls += 1;
      if (obsPolls > 3) { obsRunning = false; }
      return obsRunning;
    },
    async preview_restore({ id }) {
      await wait(350);
      const b = backups.find((x) => x.id === id);
      if (!b) throw new Error("We couldn't find that backup any more.");
      const items = b.items.map((it, i) => {
        const key = `${it.kind}:${i}`;
        if (it.kind === 'obs_scene_file') return { key, kind: it.kind, label: it.label, action: obsRunning ? 'needs_obs_closed' : 'change', note: obsRunning ? 'OBS has to be closed before this can be put back.' : null };
        if (it.kind === 'camera' && id === 'original') return { key, kind: it.kind, label: it.label, action: 'skip', note: "Your webcam isn't connected, so its settings will be left alone." };
        return { key, kind: it.kind, label: it.label, action: 'change', note: null };
      });
      return { id, label: b.label, items, obs_running: obsRunning };
    },
    async record_final() {
      await wait(1500);
      abFinal = { path: 'final.wav', lufs: -16.4, score: 91 };
      return abInfo();
    },
    async get_ab() {
      await wait(250);
      if (mockAbError) throw abErr(mockAbError, 'Mock error (' + mockAbError + ').');
      return abInfo();
    },
    async export_ab() { await wait(300); if (!abFinal) throw abErr('no_final_take', 'Record your final take first.'); return 'C:\\Users\\you\\Desktop\\Setup Coach A-B - Yeti Stereo Microphone'; },
    async reapply_saved_setup() { await wait(700); return { applied: ['mic_chain', 'camera'], failed: [], restart_obs_needed: false }; },
  };

  return {
    async invoke(cmd, args) {
      const h = handlers[cmd];
      if (!h) throw new Error(`Mock has no command ${cmd}`);
      return h(args || {});
    },
    listen(ev, h) {
      if (!listeners.has(ev)) listeners.set(ev, []);
      listeners.get(ev).push(h);
      return () => listeners.set(ev, (listeners.get(ev) || []).filter((x) => x !== h));
    },
  };
}
