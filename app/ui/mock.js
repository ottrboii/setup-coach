// Built-in mock backend: realistic sample data for every command and event in
// API.md, so the UI can be designed and tested in a plain browser.

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

const ENVIRONMENT = {
  app_version: '0.1.0-mock',
  windows_build: '26200.8524',
  obs: { running: true, connected: true, version: '33.0.1', websocket_enabled: true, error: null },
  mics: [
    {
      id: '{0.0.1.00000000}.{focusrite-in-1}', name: 'Analogue 1 + 2 (Focusrite USB Audio)', is_default: true,
      is_default_comms: false, channels: 2, sample_rate: 48000,
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
      is_default_comms: true, channels: 2, sample_rate: 48000,
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
  let saved = null;

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
    async get_environment() { await wait(600); return { ...ENVIRONMENT, saved_profile: saved }; },
    async connect_obs() { await wait(400); return ENVIRONMENT.obs; },
    async run_quick_checks() { await wait(900); return QUICK; },
    async apply_actions({ actionIds }) {
      await wait(900);
      const failed = actionIds.includes('a4') ? [{ id: 'a4', error: 'Windows needs you to confirm this one. We opened the Sound settings page for you.' }] : [];
      return { applied: actionIds.filter((a) => !failed.find((f) => f.id === a)), failed, restart_obs_needed: actionIds.includes('a1') };
    },
    async mic_context({ answers }) {
      await wait(200);
      const sm7 = answers && answers.mic_model_id === 'Shure SM7B';
      return { mic_name: sm7 ? 'Shure SM7B' : null, capsule: 'dynamic', address: 'end', connection: 'xlr', gain_needed_db: sm7 ? 60 : null,
        interface_max_gain_db: 56, booster_db: answers && answers.booster ? 25 : null, gain_at_max: answers ? answers.gain_at_max ?? null : null,
        onboard_auto_level: null, distance_cm: [3, 10] };
    },
    async start_mic_test() { micStep = null; startMicLevels(); await wait(150); return null; },
    async mark_step({ kind, edge }) { micStep = edge === 'start' ? kind : null; return null; },
    async finish_mic_test({ constraints: cs }) { clearInterval(micTimer); constraints = cs || []; await wait(1600); return micReport(constraints); },
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
    async finish_camera_test({ constraints: cs }) { clearInterval(camTimer); await wait(1200); return cameraReport(cs || [], lastCameraId); },
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
