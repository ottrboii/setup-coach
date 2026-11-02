// 3. Guided mic test with live level meter.
import { api, on } from '../api.js';
import { esc, html, icons, errorBanner } from '../components.js';
import { state, selectedMic } from '../state.js';

const SCRIPT = `Hey chat! <mark>P</mark>eter <mark>p</mark>icked a <mark>b</mark>ig <mark>b</mark>ag of <mark>p</mark>opcorn for the <mark>s</mark>tream.
<mark>S</mark>he <mark>s</mark>ays <mark>s</mark>ix <mark>s</mark>illy <mark>s</mark>nakes <mark>s</mark>neaked past the <mark>b</mark>oss.
Don't forget to hit follow — let's go!`;

export const STEPS = [
  { kind: 'silence', secs: 5, title: 'Stay quiet for a moment', text: "We're listening to your room — fans, hum, background noise. Sit like you do on stream." },
  { kind: 'read', secs: 11, title: 'Read this out loud', text: 'Use your normal streaming voice.', script: true },
  { kind: 'loud', secs: 3, title: 'Now get loud!', text: 'Laugh, cheer or yell like you just won a clutch round.' },
  { kind: 'typing', secs: 4, title: 'Type on your keyboard', text: "Type like you're in chat — don't talk." },
  { kind: 'desk_tap', secs: 3, title: 'Tap your desk twice', text: 'Like you would bump it mid-game.' },
  { kind: 'cable_wiggle', secs: 4, title: 'Wiggle the mic cable', text: 'Keep talking while you gently wiggle the cable where it plugs in.' },
  { kind: 'moving', secs: 5, title: 'Lean back and look around', text: 'Keep talking while you lean back and turn your head, like mid-game.' },
];

// Level (dBFS) → meter position (0–100 %).
const pos = (db) => Math.max(0, Math.min(100, ((db + 60) / 60) * 100));

export async function render(root, { go }) {
  let unlisten = null;
  let timer = null;
  let cancelled = false;
  const mic = selectedMic();

  root.append(html(`
    <section class="stack-lg">
      <div class="stack" style="gap:6px"><span class="eyebrow">Microphone</span><h1>Let's hear your mic</h1>
      <p class="lead">A short guided test — about 45 seconds. Sit exactly like you do when you stream, mic where it normally is.</p></div>
      <div id="body"></div>
    </section>`));
  const body = root.querySelector('#body');

  if (!state.env) {
    body.append(html('<div class="banner warn"><span>Start from the Welcome page so we know which mic to test.</span></div>'));
    const b = html('<button class="btn primary" style="margin-top:12px">Go to Welcome</button>');
    b.addEventListener('click', () => go('welcome'));
    body.append(b);
    return;
  }

  const intro = html(`
    <div class="card stack">
      <div class="device"><div class="ico">${icons.mic}</div><div class="stack" style="gap:2px"><b>${esc(mic ? mic.name : 'No mic selected')}</b><span class="subtle">Change it on the Welcome page.</span></div></div>
      <ul class="explain" style="margin:0;padding:0">${STEPS.map((s) => `<li><span class="ico">${icons.check}</span><span><b>${esc(s.title)}</b> · ${s.secs}s</span></li>`).join('')}</ul>
      ${state.micConstraints.length ? `<div class="banner info"><span>We'll keep your answers in mind (${state.micConstraints.length} thing${state.micConstraints.length === 1 ? '' : 's'} you can't change).</span></div>` : ''}
      <div class="row"><button class="btn primary lg" id="start" ${mic ? '' : 'disabled'}>Start the test</button></div>
    </div>`);
  body.append(intro);
  intro.querySelector('#start').addEventListener('click', start);

  async function start() {
    body.innerHTML = '';
    const view = html(`
      <div class="card stack-lg">
        <div class="steps">${STEPS.map(() => '<div class="step-dot"></div>').join('')}</div>
        <div class="spread" style="align-items:flex-start">
          <div class="stack" style="gap:8px;flex:1"><div class="prompt" id="title">Get ready…</div><p class="muted" id="text">Starting your mic.</p></div>
          <div class="countdown" id="count"></div>
        </div>
        <div class="script hidden" id="script">${SCRIPT.replace(/\n/g, '<br/>')}</div>
        <div class="stack" style="gap:6px">
          <div class="meter" aria-hidden="true"><div class="zone" style="left:${pos(-18)}%;width:${pos(-10) - pos(-18)}%"></div><div class="fill" id="fill"></div><div class="peak" id="peak"></div></div>
          <div class="meter-labels"><span>quiet</span><span>green zone = great talking level</span><span>too loud</span></div>
          <div id="hint" class="muted" aria-live="polite">&nbsp;</div>
        </div>
        <div class="row"><button class="btn ghost" id="cancel">Cancel</button></div>
      </div>`);
    body.append(view);
    const fill = view.querySelector('#fill');
    const peakEl = view.querySelector('#peak');
    const hint = view.querySelector('#hint');
    let currentKind = null;
    let peakHold = -90;
    let lastHintAt = 0;
    view.querySelector('#cancel').addEventListener('click', () => { cancelled = true; teardown(); go('mic'); });

    try {
      state.micContext = await api.micContext(state.micId, cleanAnswers());
      unlisten = await on('mic-level', (l) => {
        // The bar tracks speech peaks (≈ RMS + 12 dB) and is coloured by zone.
        const lvl = l.rms_db + 12;
        fill.style.width = `${pos(lvl)}%`;
        fill.dataset.zone = l.clipped ? 'clip' : lvl > -10 ? 'hot' : lvl >= -18 ? 'good' : 'low';
        peakHold = Math.max(l.peak_db, peakHold - 1.2);
        peakEl.style.left = `calc(${pos(peakHold)}% - 2px)`;
        const now = performance.now();
        if (l.clipped) {
          hint.innerHTML = '<span class="clip-warn">Too loud — that distorted. Turn the gain down a little.</span>';
          lastHintAt = now;
        } else if (now - lastHintAt > 900 && (currentKind === 'read' || currentKind === 'moving')) {
          hint.textContent = l.peak_db < -30 ? "You're quiet — move a bit closer or turn the gain up a little."
            : l.peak_db > -6 ? 'A bit hot — your laughs might distort.' : 'Great level — keep going!';
          lastHintAt = now;
        }
      });
      await api.startMicTest(state.micId);
    } catch (e) {
      teardown();
      body.innerHTML = '';
      body.append(errorBanner(e, () => { body.innerHTML = ''; start(); }));
      return;
    }

    const dots = [...view.querySelectorAll('.step-dot')];
    for (let i = 0; i < STEPS.length; i++) {
      if (cancelled) return;
      const s = STEPS[i];
      currentKind = s.kind;
      dots.forEach((d, j) => { d.className = `step-dot ${j < i ? 'done' : j === i ? 'now' : ''}`; });
      view.querySelector('#title').textContent = s.title;
      view.querySelector('#text').textContent = s.text;
      view.querySelector('#script').classList.toggle('hidden', !s.script);
      hint.innerHTML = '&nbsp;';
      await countdown(view.querySelector('#count'), 2, true);
      if (cancelled) return;
      await api.markStep(s.kind, 'start');
      await countdown(view.querySelector('#count'), s.secs);
      await api.markStep(s.kind, 'end');
    }
    dots.forEach((d) => { d.className = 'step-dot done'; });
    teardown();
    view.querySelector('#title').textContent = 'Nice! Crunching the numbers…';
    view.querySelector('#text').textContent = 'Comparing your voice with a natural reference and checking for noise, echo and pops.';
    view.querySelector('#script').classList.add('hidden');
    view.querySelector('#count').innerHTML = '<div class="spinner"></div>';
    try {
      state.micReport = await api.finishMicTest(state.micContext, state.micConstraints);
      state.micVerify = null;
      go('mic-report');
    } catch (e) {
      body.innerHTML = '';
      body.append(errorBanner(e, () => { body.innerHTML = ''; start(); }));
    }
  }

  function countdown(el, secs, ready = false) {
    return new Promise((resolve) => {
      let left = secs;
      el.textContent = ready ? '' : left;
      el.style.color = ready ? 'var(--text-3)' : '';
      if (ready) el.textContent = '…';
      clearInterval(timer);
      timer = setInterval(() => {
        left -= 1;
        if (cancelled) { clearInterval(timer); resolve(); return; }
        if (left <= 0) { clearInterval(timer); resolve(); return; }
        el.textContent = ready ? '…' : left;
      }, 1000);
    });
  }

  function teardown() {
    clearInterval(timer);
    if (unlisten) { unlisten(); unlisten = null; }
  }

  return () => { cancelled = true; teardown(); };
}

function cleanAnswers() {
  const a = state.answers;
  const out = {};
  for (const k of ['mic_model_id', 'booster', 'gain_at_max', 'onboard_auto_level']) if (a[k] !== undefined) out[k] = a[k];
  return out;
}

