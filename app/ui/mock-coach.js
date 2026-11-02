// Mock live-coaching sessions: realistic values that drift toward (and
// sometimes past) the target, changing hints, auto-adjust sessions that
// converge on their own, a stuck case and an out-of-app case.

const TICK_MS = 125;

/** Mock coaching plans per area (mirrors what the backend derives from findings). */
export function mockPlan(area) {
  if (area === 'camera') {
    return [
      { id: 'cam-1', finding_id: 'camera.backlit', task: 'light_face', title: 'Get light on your face', explain: "The window behind you is brighter than you, so the camera turns you into a silhouette. Let's put the light in front of you.", animation: 'light_move', auto: false, open_target: null, check: null },
      { id: 'cam-2', finding_id: 'camera.harsh_shadows', task: 'light_direction', title: 'Even out the shadows', explain: 'Your light comes from one side, so half your face is dark. A small move fixes it.', animation: 'light_move', auto: false, open_target: null, check: null },
      { id: 'cam-3', finding_id: 'camera.colour_cast', task: 'white_balance', title: 'Lock in your colours', explain: 'Hold up a white sheet of paper — we read the true colour of your light from it and lock it in for you.', animation: 'paper', auto: true, open_target: null, check: null },
      { id: 'cam-4', finding_id: 'camera.camera_low', task: 'camera_height', title: 'Raise the camera to eye level', explain: 'Your camera looks up at you. At eye level you look like you\'re talking to your viewers.', animation: 'camera_height', auto: false, open_target: null, check: null },
      { id: 'cam-5', finding_id: 'camera.framing', task: 'framing', title: 'Frame yourself nicely', explain: "You're a bit off to one side. Centred head-and-shoulders looks best.", animation: 'framing', auto: false, open_target: null, check: null },
      { id: 'cam-6', finding_id: 'camera.flicker_setting', task: 'flicker', title: 'Stop the flicker', explain: "Your camera's anti-flicker doesn't match your power grid. We're switching it for you — sit still a moment.", animation: 'flicker', auto: true, open_target: null, check: null },
    ];
  }
  return [
    { id: 'mic-1', finding_id: 'system.voice_clarity_on', task: 'check_setting', title: 'Turn off Windows "Voice Clarity"', explain: 'Windows is running an AI effect on your mic that can make you sound muffled. It has to be switched off in Windows Settings — we\'ll open the right page.', animation: 'settings', auto: false,
      open_target: { kind: 'windows_settings', uri_or_app: 'ms-settings:sound', label: 'Windows Sound settings', steps: ['Click your microphone under "Input".', 'Turn off "Audio enhancements" (it may be called "Voice Clarity").'] }, check: 'voice_clarity_off' },
    { id: 'mic-2', finding_id: 'mic.too_quiet', task: 'set_gain', title: 'Get your level into the green', explain: "Your mic is coming in quiet. We'll turn the gain knob on your interface together while you talk.", animation: 'knob', auto: false, open_target: null, check: null },
    { id: 'mic-3', finding_id: 'mic.too_far', task: 'find_distance', title: 'Bring the mic closer', explain: "You sound far away and a bit echoey. Closer is the single biggest upgrade — we'll show you exactly where.", animation: 'mic_distance', auto: false, open_target: null, check: null },
    { id: 'mic-4', finding_id: 'mic.plosives', task: 'angle_off_axis', title: 'Stop the pops', explain: 'Now that you\'re closer, P and B sounds puff into the mic. A small turn fixes it.', animation: 'mic_angle', auto: false, open_target: null, check: null },
    { id: 'mic-5', finding_id: 'mic.keyboard', task: 'keyboard_null', title: 'Make your keyboard quieter', explain: 'The mic hears your typing almost as loud as your voice. Pointing its back at the keyboard helps a lot.', animation: 'keyboard_null', auto: false, open_target: null, check: null },
    { id: 'mic-6', finding_id: null, task: 'quiet_room', title: 'Quiet the background', explain: "There's a steady whoosh behind you — probably your PC. Let's point the mic's deaf side at it.", animation: 'noise_null', auto: false, open_target: null, check: null },
  ];
}

// Per-task simulation config: start value, target band, hint kinds below/above
// the band, how fast the "user" corrects, and special behaviour.
const SIM = {
  set_gain: { start: -38, min: -18, max: -12, unit: 'dBFS', low: 'turn_up', high: 'turn_down', k: 0.035, overshoot: 0.45, readings: (v) => ({ speech_peak_dbfs: v, gain_knob_percent: Math.round(((v + 48) / 48) * 100) }) },
  find_distance: { start: 28, min: 4, max: 10, unit: 'cm', low: 'farther', high: 'closer', k: 0.03, overshoot: 0.5, readings: (v) => ({ estimated_distance_cm: v, room_echo_ratio: 0.2 + v / 60, proximity_bass_db: 8 - v / 5 }) },
  angle_off_axis: { start: 6, min: -30, max: -12, unit: 'dB', low: 'none', high: 'angle_away', k: 0.04, readings: (v) => ({ pop_strength_db: v, pops_heard: Math.max(0, Math.round((v + 12) / 4)) }) },
  quiet_room: { start: -46, min: -80, max: -62, unit: 'dBFS', low: 'none', high: 'be_quiet', k: 0.03, readings: (v) => ({ noise_floor_dbfs: v }) },
  keyboard_null: { start: -9, min: -45, max: -24, unit: 'dB', low: 'none', high: 'type_now', k: 0.0, stuck: true, readings: (v) => ({ typing_vs_voice_db: v }) },
  talk_into_logo: { start: -9, min: 0, max: 6, unit: 'dB', low: 'face_logo', high: 'none', k: 0.04, readings: (v) => ({ presence_vs_reference_db: v }) },
  light_face: { start: -2.2, min: 0.5, max: 1.5, unit: 'stops', low: 'add_light', high: 'none', k: 0.035, readings: (v) => ({ face_vs_background_stops: v, face_clipped_percent: 0 }) },
  light_direction: { start: 5.2, min: 1.2, max: 3, unit: ':1', low: 'none', high: 'move_light_left', k: 0.035, readings: (v) => ({ key_to_fill_ratio: v }) },
  white_balance: { start: 14, min: 0, max: 4, unit: '%', low: 'none', high: 'wait', k: 0.06, auto: true, readings: (v) => ({ paper_cast_percent: v, white_balance_kelvin: Math.round(3300 + (14 - v) * 120) }) },
  camera_height: { start: -18, min: -4, max: 4, unit: '°', low: 'raise_camera', high: 'lower_camera', k: 0.035, overshoot: 0.5, readings: (v) => ({ camera_pitch_deg: v }) },
  framing: { start: 0.25, min: 0.85, max: 1, unit: '', low: 'move_right', high: 'none', k: 0.04, readings: (v) => ({ framing_score: v }) },
  flicker: { start: 0.9, min: 0, max: 0.1, unit: '', low: 'none', high: 'wait', k: 0.07, auto: true, readings: (v) => ({ banding_strength: v, power_line_hz: 50 }) },
  check_setting: { start: 1, min: 0, max: 0.1, unit: '', low: 'none', high: 'wait', k: 0 },
};

const HEADLINES = {
  turn_up: ['Turn the knob up a bit more…', 'Keep turning up…'], turn_down: ['A touch too far — turn it down a little', 'Back down a little…'],
  closer: ['Bring the mic closer…', 'A little closer…'], farther: ['Too close — back off a touch', 'Ease back a little…'],
  angle_away: ['Turn the mic slightly to the side…', 'Keep saying "pop" while you turn it…'],
  be_quiet: ['Stay quiet — turn the mic\'s back toward your PC', 'Keep turning the back toward the noise…'],
  type_now: ['Keep typing — turn the mic\'s back to the keyboard', 'Bring the mic closer to your mouth…'],
  face_logo: ['Turn the logo side toward your mouth…', 'Keep talking while you turn it…'],
  add_light: ['Add light in front of you…', 'Brighter on your face — or close the blinds a bit'],
  move_light_left: ['Move the light toward your left…', 'A little further left…'],
  raise_camera: ['Raise the camera…', 'A bit higher — towards your eye line'], lower_camera: ['A little lower…', 'Lower it a touch'],
  move_right: ['Shift a little to your right…', 'Keep moving right…'],
  wait: ['Locking it in for you…', 'Hold still a moment…'],
};
const DETAILS = {
  set_gain: 'Talk like you do on stream the whole time.', find_distance: 'About a fist away from your mouth is the sweet spot for this mic.',
  angle_off_axis: 'Say "pop, pop, Peter Piper" while you adjust.', quiet_room: 'Stay silent while you adjust — we\'re listening to the room.',
  keyboard_null: 'Type normally while you adjust, no talking.', talk_into_logo: 'Keep talking while you turn the mic.',
  light_face: 'Watch your face in the preview get brighter.', light_direction: 'Keep the light slightly above your eyes, pointing at your face.',
  white_balance: 'Keep the paper inside the dashed box.', camera_height: 'Look at your main screen like you do on stream.',
  framing: 'Head and shoulders, centred, with a little space above your head.', flicker: 'Sit still while we check the picture.',
};

export function createCoachSim(emit, drawOverlay) {
  let timer = null;
  let opened = false;
  let openedAt = 0;

  function stop() { clearInterval(timer); timer = null; }

  function start(step) {
    stop();
    opened = false;
    const cfg = SIM[step.task] || SIM.check_setting;
    const auto = !!(step.auto || cfg.auto);
    const center = (cfg.min + cfg.max) / 2;
    const half = (cfg.max - cfg.min) / 2;
    const span0 = Math.abs(cfg.start - center) || 1;
    let v = cfg.start; let t = 0; let hold = 0; let overshot = false; let lastProgressT = 0; let best = 0; let done = false;
    const holdNeeded = step.task === 'check_setting' ? 0 : 2.0;

    timer = setInterval(() => {
      t += TICK_MS / 1000;
      const dt = TICK_MS / 1000;
      // --- value dynamics -------------------------------------------------
      if (step.task === 'check_setting') {
        if (opened && Date.now() - openedAt > 6000) v = 0;
      } else if (cfg.stuck) {
        v = cfg.start + Math.sin(t * 0.9) * 3 + (Math.random() - 0.5) * 1.5 - Math.min(5, t * 0.6);
      } else {
        // Simulated user: corrects toward the band, overshoots once, settles.
        let goal = center;
        if (cfg.overshoot && !overshot && t > 1.5) goal = center + (center - cfg.start) * cfg.overshoot;
        if (cfg.overshoot && !overshot && Math.abs(v - goal) < half * 0.6) overshot = true;
        const k = auto ? cfg.k * 1.4 : cfg.k;
        v += (goal - v) * k * (auto ? 1 : 0.6 + Math.random() * 0.8);
        if (!auto) v += (Math.random() - 0.5) * half * 0.25;
      }
      const inBand = v >= cfg.min && v <= cfg.max;
      const dist = inBand ? 0 : Math.min(Math.abs(v - cfg.min), Math.abs(v - cfg.max));
      const almost = !inBand && dist < Math.max(half, span0 * 0.15);
      hold = inBand ? hold + dt : 0;
      let progress = Math.max(0, Math.min(1, 1 - dist / span0));
      if (inBand) progress = Math.max(progress, 0.85 + 0.15 * (holdNeeded ? hold / holdNeeded : 1));
      if (progress > best + 0.02) { best = progress; lastProgressT = t; }

      let phase = inBand ? 'hold' : almost ? 'almost' : 'working';
      if (step.task === 'check_setting') phase = !opened ? 'waiting' : v === 0 ? 'done' : 'working';
      else if (inBand && hold >= holdNeeded) phase = 'done';
      if (t < 0.6 && step.task !== 'check_setting') phase = 'waiting';
      const stuckAfter = cfg.stuck ? 8 : 45;
      if (phase !== 'done' && t - lastProgressT > stuckAfter && t > stuckAfter) phase = 'stuck';

      // --- hint + text --------------------------------------------------------
      let kind = inBand ? 'stop' : v < cfg.min ? cfg.low : cfg.high;
      if (kind === 'none') kind = cfg.high !== 'none' ? cfg.high : cfg.low;
      if (phase === 'waiting') kind = step.task === 'check_setting' ? 'wait' : ({ set_gain: 'keep_talking', find_distance: 'keep_talking', angle_off_axis: 'say_pops', quiet_room: 'be_quiet', keyboard_null: 'type_now', talk_into_logo: 'keep_talking' }[step.task] || 'wait');
      const amount = Math.max(0, Math.min(1, dist / span0));
      const variants = HEADLINES[kind] || [''];
      let headline = variants[Math.floor(t / 2.5) % variants.length];
      if (auto && phase !== 'done') headline = step.task === 'set_gain' ? 'Setting your level… keep talking' : HEADLINES.wait[Math.floor(t / 2.5) % 2];
      if (phase === 'hold') headline = 'Perfect — hold it right there';
      if (phase === 'almost' && !auto) headline = `Almost… ${(HEADLINES[kind] || [''])[0].replace('…', '').toLowerCase()} just a touch`;
      if (phase === 'done') headline = step.task === 'check_setting' ? "Got it — that's switched off" : 'Nailed it!';
      if (phase === 'waiting' && step.task !== 'check_setting') headline = { set_gain: 'Start talking…', find_distance: 'Start talking…', angle_off_axis: 'Say "pop, pop, Peter Piper"…', quiet_room: 'Stay quiet for a moment…', keyboard_null: 'Start typing…', talk_into_logo: 'Start talking…' }[step.task] || 'Hold still…';
      if (phase === 'stuck') headline = "This one's being stubborn";
      if (step.task === 'check_setting' && phase === 'working') headline = 'Watching for the change…';

      const update = {
        task: step.task, phase, progress: phase === 'done' ? 1 : progress,
        value: step.task === 'check_setting' ? null : Math.round(v * 100) / 100,
        target: step.task === 'check_setting' ? null : { min: cfg.min, max: cfg.max, unit: cfg.unit },
        headline, detail: DETAILS[step.task] || (opened ? 'Do the steps in the other window — we\'ll bring you back.' : ''),
        hint: { kind: phase === 'done' ? 'none' : kind, amount, text: '' },
        hold_s: Math.min(hold, holdNeeded), hold_needed_s: holdNeeded,
        readings: cfg.readings ? cfg.readings(Math.round(v * 10) / 10) : { setting_on: v ? 1 : 0 },
        auto_adjusting: auto && phase !== 'done',
        stuck_alternative: cfg.stuck ? "Can't move the mic away from the keyboard? A desk mat or quieter keys help — or tap \"I can't do this\" and we'll quiet the gaps between your words instead." : 'Try a smaller change and wait a second — or tell us you can\'t, and we\'ll tune around it.',
      };
      emit('coach-update', update);
      if (drawOverlay && ['light_face', 'light_direction', 'white_balance', 'camera_height', 'framing', 'flicker'].includes(step.task) && Math.round(t * 8) % 2 === 0) {
        emit('coach-camera-overlay', drawOverlay(step.task, progress, v, phase));
      }
      if (phase === 'done' && !done) { done = true; setTimeout(stop, 600); }
    }, TICK_MS);
  }

  return {
    start,
    stop,
    open() { opened = true; openedAt = Date.now(); },
  };
}

/** Draw a mock camera frame + overlay data for a camera coaching task. */
export function drawCoachOverlay(task, progress, value, phase) {
  const W = 480, H = 270;
  const c = document.createElement('canvas');
  c.width = W; c.height = H;
  const g = c.getContext('2d');
  const p = Math.max(0, Math.min(1, progress));
  // Room + window.
  g.fillStyle = '#2f2a25'; g.fillRect(0, 0, W, H);
  const winBright = task === 'light_face' ? 1 - 0.45 * p : 0.75;
  g.fillStyle = `rgba(243,240,230,${winBright})`; g.fillRect(310, 28, 130, 140);
  g.fillStyle = '#4a4036'; g.fillRect(372, 28, 6, 140); g.fillRect(310, 96, 130, 6);
  // Person placement.
  let fx = 240, fy = 140, sc = 1;
  if (task === 'camera_height') fy = 175 - 45 * p;
  if (task === 'framing') { fx = 150 + 90 * p; sc = 1; }
  const faceLum = task === 'light_face' ? 0.45 + 0.55 * p : 0.95;
  g.save(); g.translate(fx, fy); g.scale(sc, sc);
  g.fillStyle = '#1d2333'; g.beginPath(); g.ellipse(0, 120, 120, 70, 0, 0, Math.PI * 2); g.fill();
  const skin = (k) => `rgb(${Math.round(185 * k)},${Math.round(138 * k)},${Math.round(108 * k)})`;
  g.fillStyle = skin(faceLum); g.beginPath(); g.ellipse(0, 0, 46, 58, 0, 0, Math.PI * 2); g.fill();
  if (task === 'light_direction') { // shadow on the right half fades as the light moves
    g.fillStyle = `rgba(0,0,0,${0.55 * (1 - p)})`; g.beginPath(); g.ellipse(0, 0, 46, 58, 0, -Math.PI / 2, Math.PI / 2); g.fill();
  }
  g.fillStyle = '#2b1f18'; g.beginPath(); g.ellipse(0, -42, 50, 28, 0, Math.PI, Math.PI * 2); g.fill();
  g.restore();
  // Paper with a warm cast that neutralises.
  let paper = null;
  if (task === 'white_balance') {
    const cast = Math.max(0, Math.min(1, value / 14));
    g.fillStyle = `rgb(${Math.round(244)},${Math.round(236 - 18 * cast)},${Math.round(226 - 60 * cast)})`;
    g.fillRect(186, 88, 108, 92);
    paper = { x: 180, y: 82, w: 120, h: 104 };
  }
  // Flicker bands.
  if (task === 'flicker') {
    const t = Date.now() / 1000;
    g.fillStyle = `rgba(0,0,0,${0.35 * Math.max(0, Math.min(1, value))})`;
    for (let i = 0; i < 4; i++) g.fillRect(0, ((t * 90 + i * 70) % (H + 40)) - 30, W, 24);
  }
  // Noise speckle.
  g.fillStyle = 'rgba(255,255,255,0.05)';
  for (let i = 0; i < 250; i++) g.fillRect(Math.random() * W, Math.random() * H, 1, 1);
  const face = { x: Math.round(fx - 50), y: Math.round(fy - 64), w: 100, h: 126 };
  // Clipped (blown-out) areas: the window, while it is too bright.
  let mask = null;
  if (task === 'light_face' && winBright > 0.7) {
    const m = document.createElement('canvas'); m.width = W; m.height = H;
    const mg = m.getContext('2d');
    mg.fillStyle = 'rgba(255,40,40,1)';
    for (let y = 28; y < 168; y += 8) for (let x = 310; x < 440; x += 8) if ((x + y) % 16 === 0) mg.fillRect(x, y, 6, 6);
    mask = m.toDataURL('image/png');
  }
  // Arrows in normalised frame coordinates (0..1).
  const arrows = [];
  if (phase !== 'done' && phase !== 'hold') {
    if (task === 'light_face') arrows.push({ x: 0.12, y: 0.3, dx: 0.14, dy: 0.08 });
    if (task === 'light_direction') arrows.push({ x: 0.22, y: 0.22, dx: -0.1, dy: 0 });
    if (task === 'camera_height') arrows.push({ x: 0.9, y: 0.75, dx: 0, dy: -0.25 });
    if (task === 'framing') arrows.push({ x: fx / W + 0.12, y: 0.35, dx: 0.12, dy: 0 });
  }
  return { frame: c.toDataURL('image/jpeg', 0.8), face, face_ok: phase === 'hold' || phase === 'done', paper, clipped_mask: mask, arrows };
}
