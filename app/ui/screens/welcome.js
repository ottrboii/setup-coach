// 1. Welcome → consent → environment (OBS + devices + profile questions).
import { api } from '../api.js';
import { esc, html, icons, loading, errorBanner } from '../components.js';
import { state, selectedMic } from '../state.js';

const CONFIDENCE = {
  exact: ['ok', 'Recognised'],
  name_match: ['ok', 'Recognised by name'],
  generic: ['minor', 'Not sure which model'],
};

function deviceName(d) {
  const p = d.profile?.profile;
  if (p && d.profile.confidence !== 'generic') return `${p.brand} ${p.model}`;
  return d.name;
}

export async function render(root, { go, refreshNav }) {
  root.append(html(`
    <section class="stack-lg">
      <div class="card hero stack">
        <span class="eyebrow">Setup Coach</span>
        <h1>Let's make you sound and look great on stream</h1>
        <p class="lead">In about five minutes we'll check your mic, camera and OBS, explain anything that's off in plain words, and set things up properly. No AI filters — just your gear, set up right.</p>
      </div>
      <div id="body"></div>
    </section>`));
  const body = root.querySelector('#body');

  const consent = html(`
    <div class="card stack">
      <h2>First, a quick heads-up</h2>
      <p class="muted">To change settings in OBS for you, Setup Coach reads OBS's remote-control settings (the websocket password) from this PC. Your audio and video are analysed on this computer and never uploaded.</p>
      <div class="row"><button class="btn primary lg" id="go">Sounds good — let's start</button></div>
    </div>`);
  if (state.env) return showEnv(body, go, refreshNav);
  body.append(consent);
  consent.querySelector('#go').addEventListener('click', () => showEnv(body, go, refreshNav, true));
}

async function showEnv(body, go, refreshNav, fetch = false) {
  body.innerHTML = '';
  if (fetch || !state.env) {
    body.append(loading('Looking for OBS, your mic and your camera…'));
    try {
      state.env = await api.getEnvironment();
    } catch (e) {
      body.innerHTML = '';
      body.append(errorBanner(e, () => showEnv(body, go, refreshNav, true)));
      return;
    }
    body.innerHTML = '';
  }
  const env = state.env;
  state.micId ??= (env.mics.find((m) => m.is_default) || env.mics[0])?.id ?? null;
  state.cameraId ??= (env.cameras.find((c) => c.obs_source) || env.cameras[0])?.id ?? null;
  refreshNav();

  const wrap = html('<div class="stack-lg"></div>');
  body.append(wrap);
  wrap.append(obsCard(env, () => showEnv(body, go, refreshNav, true)));

  const devices = html(`<div class="grid-2"></div>`);
  devices.append(devicePicker('Microphone', icons.mic, env.mics, state.micId, (id) => { state.micId = id; renderQuestions(); }));
  devices.append(devicePicker('Camera', icons.cam, env.cameras, state.cameraId, (id) => { state.cameraId = id; }));
  wrap.append(devices);

  const qBox = html('<div class="stack"></div>');
  wrap.append(qBox);
  function renderQuestions() {
    qBox.innerHTML = '';
    const mic = selectedMic();
    // Real backend: ProfileMatch.questions (API.md). Browser mock: needs_questions.
    const qs = mic?.profile?.questions || mic?.profile?.needs_questions || [];
    const extra = mic && /focusrite|goxlr|scarlett|wave xlr|rodecaster|umc|interface|analogue/i.test(mic.name)
      ? [{ id: 'gain_at_max', prompt: 'Is the gain knob for your mic turned all (or almost all) the way up?', options: ['Yes', 'No', "I'm not sure"] }]
      : [];
    const all = [...qs, ...extra];
    if (!all.length) return;
    qBox.append(html(`<div><h2>A couple of quick questions</h2><p class="muted">So we can give advice for your exact gear. Tap an answer — "not sure" is fine.</p></div>`));
    for (const q of all) qBox.append(questionCard(q));
  }
  renderQuestions();

  const next = html(`<div class="row"><button class="btn primary lg">Run the free quick scan</button><span class="subtle">Checks Windows and OBS settings — takes a few seconds.</span></div>`);
  next.querySelector('button').addEventListener('click', () => {
    // A button must never fail silently: if going on throws, say so and log it.
    try {
      go('scan');
    } catch (e) {
      api.logUi('error', 'welcome.go_scan', e && e.stack ? e.stack : e);
      wrap.querySelector('.go-error')?.remove();
      wrap.append(errorBanner(new Error("We couldn't open the scan. Please try again."), () => go('scan')));
      wrap.lastElementChild.classList.add('go-error');
    }
  });
  wrap.append(next);
}

function obsCard(env, retry) {
  const o = env.obs;
  let led = 'good';
  let title = `Connected to OBS ${esc(o.version || '')}`;
  let text = "We can check and fix your OBS settings for you.";
  let fix = '';
  if (!o.running) {
    led = 'bad'; title = "OBS isn't running"; text = 'Open OBS Studio, then press "Check again".';
  } else if (!o.connected) {
    led = 'warn'; title = "OBS is open, but we can't connect yet";
    text = o.websocket_enabled === false
      ? 'OBS\'s remote control is switched off.'
      : (o.error || 'We couldn\'t log in to OBS remote control.');
    fix = o.websocket_enabled === false
      ? '<ol class="coach-steps"><li>In OBS, open <b>Tools → WebSocket Server Settings</b>.</li><li>Tick <b>Enable WebSocket server</b> and press OK.</li><li>Come back and press "Check again".</li></ol>'
      : '';
  }
  const card = html(`
    <div class="card stack">
      <div class="spread">
        <div class="row"><span class="led ${led}" style="width:12px;height:12px"></span><h3>${title}</h3></div>
        ${o.connected ? '' : '<button class="btn sm" id="retry">Check again</button>'}
      </div>
      <p class="muted">${esc(text)}</p>
      ${fix}
    </div>`);
  card.querySelector('#retry')?.addEventListener('click', retry);
  return card;
}

function devicePicker(label, icon, list, selected, onPick) {
  const card = html(`<div class="card stack"><div class="eyebrow">${esc(label)}</div><div class="list stack" style="gap:10px"></div></div>`);
  const listEl = card.querySelector('.list');
  if (!list.length) {
    listEl.append(html(`<p class="muted">We couldn't find a ${esc(label.toLowerCase())}. Plug it in and press "Check again".</p>`));
    return card;
  }
  const draw = (sel) => {
    listEl.innerHTML = '';
    for (const d of list) {
      const conf = CONFIDENCE[d.profile?.confidence] || CONFIDENCE.generic;
      const b = html(`
        <button class="choice ${d.id === sel ? 'selected' : ''}" aria-pressed="${d.id === sel}">
          <div class="device">
            <div class="ico">${icon}</div>
            <div class="stack" style="gap:4px;min-width:0">
              <span>${esc(deviceName(d))}</span>
              <span class="row" style="gap:6px"><span class="chip ${conf[0]}">${conf[1]}</span>${d.is_default ? '<span class="chip">Windows default</span>' : ''}${d.obs_source ? `<span class="chip">In OBS: ${esc(d.obs_source)}</span>` : ''}</span>
              <span class="subtle" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(d.name)}</span>
            </div>
          </div>
        </button>`);
      b.addEventListener('click', () => { onPick(d.id); draw(d.id); });
      listEl.append(b);
    }
  };
  draw(selected);
  return card;
}

function applyAnswer(q, value) {
  state.answers[q.id] = value;
  const yes = value === 'Yes';
  const unsure = /not sure/i.test(value);
  switch (q.id) {
    case 'which_mic_on_interface':
    case 'which_model':
    case 'which_headset_model':
      state.answers.mic_model_id = /something else/i.test(value) ? undefined : value;
      break;
    case 'booster_in_chain':
      state.answers.booster = unsure ? undefined : yes;
      break;
    case 'onboard_processing':
      state.answers.onboard_auto_level = unsure ? undefined : yes;
      break;
    case 'gain_at_max':
      state.answers.gain_at_max = unsure ? undefined : yes;
      break;
    default:
      break;
  }
}

function questionCard(q) {
  const opts = q.options && q.options.length ? q.options : ['Yes', 'No', "I'm not sure"];
  const card = html(`<div class="card flat stack"><h3>${esc(q.prompt)}</h3><div class="choices"></div></div>`);
  const box = card.querySelector('.choices');
  const draw = () => {
    box.innerHTML = '';
    for (const o of opts) {
      const b = html(`<button class="choice ${state.answers[q.id] === o ? 'selected' : ''}">${esc(o)}</button>`);
      b.addEventListener('click', () => { applyAnswer(q, o); draw(); });
      box.append(b);
    }
  };
  draw();
  return card;
}
