// 6. Camera test (paper-metered) → report → apply / coach-only steps.
import { api, on } from '../api.js';
import { esc, html, icons, scoreRing, scoreWord, findingCard, upgradeCard, toast, errorBanner, advanced, kvTable, fmt } from '../components.js';
import { state, selectedCamera, CAMERA_CONSTRAINT } from '../state.js';
import { delta } from './micreport.js';

const STEPS = [
  { step: 'neutral', secs: 4, title: 'Sit like you do on stream', text: 'Look at your main screen and relax.' },
  { step: 'turn_head', secs: 5, title: 'Turn your head slowly', text: 'Left, right, then back to the middle.' },
  { step: 'hold_paper', secs: 4, title: 'Hold up a white sheet of paper', text: 'Fill the dashed box with it, next to your face. Any plain printer paper works.' },
  { step: 'lean_in_out', secs: 5, title: 'Lean in, then lean back', text: 'Like you would to read chat, then sit back.' },
];

const CLASS_TEXT = {
  full_uvc: 'Webcam — we can set it for you',
  limited_uvc: 'Webcam with a few settings — we set what we can',
  capture_card: "Camera through a capture card — we'll walk you through your camera's menus",
  phone_or_virtual: "Phone or virtual camera — settings live in the phone app; we'll guide you",
  phone: "Phone as webcam — we'll guide you through the app",
  virtual: "Virtual camera — we'll guide you",
  laptop: 'Laptop camera — we set what we can',
};

export async function render(root, { go, refreshNav }) {
  let unlisten = null;
  let cancelled = false;
  const cam = selectedCamera();
  const page = html('<section class="stack-lg"></section>');
  root.append(page);

  if (!state.env) {
    page.append(html('<div class="banner warn"><span>Start from the Welcome page so we know which camera to test.</span></div>'));
    const b = html('<button class="btn primary">Go to Welcome</button>');
    b.addEventListener('click', () => go('welcome'));
    page.append(b);
    return;
  }

  if (state.autoStart === 'camera' && cam) { state.autoStart = null; state.cameraReport = null; runTest(); }
  else if (state.cameraReport) drawReport();
  else drawIntro();

  function header(title, lead) {
    return html(`<div class="stack" style="gap:6px"><span class="eyebrow">Camera</span><h1>${esc(title)}</h1><p class="lead">${esc(lead)}</p></div>`);
  }

  function drawIntro() {
    page.innerHTML = '';
    page.append(header('Let\'s look at your camera', 'A 20-second check of your light, colour, focus and framing. Grab a plain white sheet of paper first — it tells us exactly how bright and what colour your light is.'));
    const cls = cam?.profile?.profile?.class;
    const card = html(`
      <div class="card stack">
        <div class="device"><div class="ico">${icons.cam}</div><div class="stack" style="gap:2px"><b>${esc(cam ? cam.name : 'No camera selected')}</b><span class="subtle">${esc(CLASS_TEXT[cls] || 'Camera')}</span></div></div>
        <ul class="explain" style="margin:0;padding:0">${STEPS.map((s) => `<li><span class="ico">${icons.check}</span><span><b>${esc(s.title)}</b> · ${s.secs}s</span></li>`).join('')}</ul>
        <div class="row"><button class="btn primary lg" id="start" ${cam ? '' : 'disabled'}>I have my paper — start</button></div>
      </div>`);
    card.querySelector('#start').addEventListener('click', runTest);
    page.append(card);
  }

  async function runTest() {
    page.innerHTML = '';
    page.append(header('Camera check', 'Follow the prompts — we\'re only measuring, nothing is recorded or uploaded.'));
    const view = html(`
      <div class="grid-2" style="grid-template-columns:3fr 2fr;align-items:start">
        <div class="preview"><img id="img" alt="Live camera preview"/><span class="badge" id="badge">Live</span><div class="guide hidden" id="guide"><span>Fill this box with white paper</span></div></div>
        <div class="card stack">
          <div class="steps">${STEPS.map(() => '<div class="step-dot"></div>').join('')}</div>
          <div class="prompt" id="title">Starting your camera…</div>
          <p class="muted" id="text"></p>
          <div class="countdown" id="count" style="text-align:left"></div>
          <button class="btn ghost" id="cancel">Cancel</button>
        </div>
      </div>`);
    page.append(view);
    const img = view.querySelector('#img');
    view.querySelector('#cancel').addEventListener('click', () => { cancelled = true; teardown(); drawIntro(); cancelled = false; });
    try {
      unlisten = await on('camera-preview', (src) => { img.src = src; });
      await api.startCameraTest(state.cameraId);
      const dots = [...view.querySelectorAll('.step-dot')];
      for (let i = 0; i < STEPS.length; i++) {
        if (cancelled) return;
        const s = STEPS[i];
        dots.forEach((d, j) => { d.className = `step-dot ${j < i ? 'done' : j === i ? 'now' : ''}`; });
        view.querySelector('#title').textContent = s.title;
        view.querySelector('#text').textContent = s.text;
        view.querySelector('#guide').classList.toggle('hidden', s.step !== 'hold_paper');
        view.querySelector('#count').textContent = `${s.secs}s`;
        const res = await api.captureCameraStep(s.step, s.secs);
        if (cancelled) return;
        if (s.step !== 'hold_paper' && res.faces_found === 0) toast("We couldn't see your face", 'Make sure you are in frame and the lens cap is off.', 'warn');
      }
      dots.forEach((d) => { d.className = 'step-dot done'; });
      view.querySelector('#title').textContent = 'Looking at the results…';
      view.querySelector('#text').textContent = 'Checking light, colour, flicker, focus and framing.';
      view.querySelector('#count').innerHTML = '<div class="spinner"></div>';
      state.cameraReport = await api.finishCameraTest(state.cameraConstraints);
      teardown();
      drawReport();
    } catch (e) {
      teardown();
      page.append(errorBanner(e, runTest));
    }
  }

  function drawReport() {
    const r = state.cameraReport;
    page.innerHTML = '';
    const open = r.findings.filter((f) => f.severity !== 'info');
    const handled = r.findings.filter((f) => f.severity === 'info');
    const coachOnly = !r.settings.uvc;

    const head = html(`<div class="card hero"><div class="row" style="gap:28px"><div class="ring"></div>
      <div class="stack" style="gap:8px;flex:1;min-width:260px"><span class="eyebrow">Your camera report</span><div class="row" style="gap:10px"><h1>${esc(scoreWord(r.score.value).replace('Sounding', 'Looking'))}</h1>${delta(state.scoreBefore?.camera, r.score.value)}</div>
      <p class="muted">${open.length ? `${open.length} thing${open.length === 1 ? '' : 's'} to improve. Light matters more than any setting, so start there.` : 'Your picture looks good. We\'ll lock the settings so it stays that way.'}</p></div></div></div>`);
    head.querySelector('.ring').append(scoreRing(r.score.value, { label: 'camera' }));
    page.append(head);

    if (open.some((f) => f.fix_kind !== 'auto')) {
      const coachCard = html(`<div class="card coach-cta"><div class="stack" style="gap:4px;flex:1;min-width:240px"><h2>Let's fix your light together</h2>
        <p class="muted">Live preview with arrows: move the lamp, raise the camera, hold up the paper — we'll tell you the moment it looks right.</p></div>
        <button class="btn primary lg">Coach me through it</button></div>`);
      coachCard.querySelector('button').addEventListener('click', () => { state.coach = { area: 'camera', steps: null, index: 0, status: {} }; go('coach'); });
      page.append(coachCard);
    }
    if (open.length) {
      const list = html('<div class="findings"></div>');
      for (const f of open) {
        list.append(findingCard(f, { onDone: () => { state.cameraReport = null; drawIntro(); }, onCant: CAMERA_CONSTRAINT[f.id] ? (fd, btn) => cant(fd, btn) : undefined }));
        if (f.suggest_product && f.fix_kind !== 'auto' && !(list.dataset.upgrades || '').split(',').includes(f.suggest_product)) { list.dataset.upgrades = (list.dataset.upgrades || '') + ',' + f.suggest_product; const u = upgradeCard(f.suggest_product); if (u) list.append(u); }
      }
      page.append(list);
    }
    if (handled.length) {
      page.append(html('<h2>Handled for you</h2>'));
      const list = html('<div class="findings"></div>');
      handled.forEach((f) => list.append(findingCard(f, { visual: false })));
      page.append(list);
    }

    const s = r.settings;
    if (coachOnly) {
      page.append(html(`<div class="card stack"><h2>Set these on your camera</h2><p class="muted">Your camera connects through a capture card or app, so these live in its own menus. Do them once — they stick.</p>
        <ol class="coach-steps">${s.coach_steps.map((t) => `<li>${esc(t)}</li>`).join('')}</ol></div>`));
    } else {
      const u = s.uvc;
      const lines = [
        ['Brightness locked for your room', "No more 'breathing' when you move or a bright scene flashes."],
        u.wb_kelvin ? [`Colour locked to your light (${Math.round(u.wb_kelvin)} K)`, 'Measured from your white paper, so skin tones stay natural.'] : null,
        !u.auto_focus ? ['Focus locked on you', 'No hunting when you lean in or move your hands.'] : null,
        u.powerline_hz ? [`Anti-flicker set to ${u.powerline_hz} Hz`, 'Matches your power grid, so lights don\'t cause rolling bands.'] : null,
        [`${s.format.height}p at ${Math.round(s.format.fps)} fps${s.format.format === 'mjpeg' ? ' (compressed, smooth)' : ''}`, 'The best mode your camera and USB port can do smoothly.'],
      ].filter(Boolean);
      page.append(html(`<div class="card explain"><h2 style="margin-bottom:6px">What we'll set</h2><ul>${lines.map(([t, d]) => `<li><span class="ico">${icons.check}</span><div><b>${esc(t)}</b><div class="muted">${esc(d)}</div></div></li>`).join('')}</ul></div>`));
      if (s.coach_steps?.length) page.append(html(`<div class="card stack"><h3>And a couple of things for you</h3><ol class="coach-steps">${s.coach_steps.map((t) => `<li>${esc(t)}</li>`).join('')}</ol></div>`));
    }
    if (s.light_limited || s.notes?.length) {
      page.append(html(`<div class="banner info"><div class="stack" style="gap:4px">${s.light_limited ? '<b>Your room is on the dim side.</b><span>More light in front of you is the biggest upgrade for your picture.</span>' : ''}${(s.notes || []).map((n) => `<span>${esc(n)}</span>`).join('')}</div></div>`));
    }

    const actions = html(`<div class="row">${coachOnly ? '<button class="btn primary lg" id="done">I\'ve set these</button>' : '<button class="btn primary lg" id="apply">Apply camera settings</button>'}<button class="btn lg" id="again">Retest camera</button></div>`);
    actions.querySelector('#again').addEventListener('click', () => { state.cameraReport = null; drawIntro(); });
    const applyBtn = actions.querySelector('#apply');
    if (applyBtn) applyBtn.addEventListener('click', async () => {
      applyBtn.disabled = true;
      applyBtn.innerHTML = '<span class="spinner" style="width:18px;height:18px;border-width:2px"></span> Applying…';
      try {
        const res = await api.applyCameraSettings(state.cameraId, r.settings);
        state.cameraApplied = true; refreshNav();
        if (res.failed?.length) toast('Some settings need a hand', res.failed.map((f) => f.error).join(' '), 'warn');
        else toast('Camera set up', "We'll re-apply these every time your camera reconnects.", 'good');
        applyBtn.textContent = 'Applied ✓';
        nextBtn();
      } catch (e) {
        toast("Couldn't apply", e.message, 'bad');
        applyBtn.disabled = false;
        applyBtn.textContent = 'Apply camera settings';
      }
    });
    const doneBtn = actions.querySelector('#done');
    if (doneBtn) doneBtn.addEventListener('click', () => { state.cameraApplied = true; refreshNav(); nextBtn(); doneBtn.disabled = true; });
    page.append(actions);

    function nextBtn() {
      if (page.querySelector('#next')) return;
      const n = html('<div class="card hero stack" id="next"><h2>Next: stream health</h2><p class="muted">Watch for dropped frames and lag while you stream — in plain words.</p><div class="row"><button class="btn primary lg">Open stream health</button></div></div>');
      n.querySelector('button').addEventListener('click', () => go('health'));
      page.append(n);
    }

    const m = r.metrics || {};
    page.append(advanced('Advanced: the numbers', kvTable([
      ['Face vs background', fmt(m.face_vs_background_stops, 1, ' stops')],
      ['Colour cast on paper', fmt(m.paper_cast_pct, 1, ' %')],
      ['Noise', fmt(m.noise_sigma, 1)],
      ['Face sharpness', fmt(m.face_sharpness, 0)],
      ['Delivered fps', fmt(m.delivered_fps, 1)],
      ['Settings', JSON.stringify(r.settings.uvc || r.settings.format)],
    ])));
  }

  async function cant(f, btn) {
    const c = CAMERA_CONSTRAINT[f.id];
    if (!state.cameraConstraints.includes(c)) state.cameraConstraints.push(c);
    btn.disabled = true;
    btn.innerHTML = '<span class="spinner" style="width:16px;height:16px;border-width:2px"></span> Re-tuning…';
    try {
      state.cameraReport = await api.finishCameraTest(state.cameraConstraints);
      toast('Got it', "We've adjusted the camera settings around that.", 'good');
      drawReport();
    } catch (e) {
      toast("Couldn't re-tune", e.message, 'bad');
      btn.disabled = false;
      btn.textContent = "I can't do this";
    }
  }

  function teardown() {
    if (unlisten) { unlisten(); unlisten = null; }
  }
  return () => { cancelled = true; teardown(); };
}
