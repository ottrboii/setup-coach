// 1. Welcome → consent → environment (OBS + devices + profile questions).
import { api } from '../api.js';
import { esc, html, icons, loading, errorBanner } from '../components.js';
import { state, selectedMic } from '../state.js';
import { openMicChoice, yourMicLine } from '../micpicker.js';

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
      <p class="muted">To change settings in OBS for you, Setup Coach reads OBS's remote-control settings (the password OBS made for this) from this PC. Your audio and video are analysed on this computer and never uploaded.</p>
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
  devices.append(devicePicker('Microphone', icons.mic, env.mics, state.micId, (id) => { state.micId = id; renderMicChoice(); renderQuestions(); }));
  devices.append(devicePicker('Camera', icons.cam, env.cameras, state.cameraId, (id) => { state.cameraId = id; }));
  wrap.append(devices);

  // What mic is plugged into this? Asked once for interface/GoXLR-type or generic-named devices.
  const choiceBox = html('<div class="stack" id="mic-choice"></div>');
  wrap.append(choiceBox);
  function renderMicChoice(attention = false) {
    choiceBox.innerHTML = '';
    const mic = selectedMic();
    if (!mic) return;
    const after = () => { renderMicChoice(); renderQuestions(); };
    if (mic.needs_mic_choice) {
      openMicChoice(choiceBox, mic, { required: true, onDone: after });
      if (attention) choiceBox.firstElementChild?.classList.add('needs-attention');
    } else if (mic.mic_choice) {
      const card = html('<div class="card flat stack"><div class="slot"></div></div>');
      card.querySelector('.slot').append(yourMicLine(mic, () => openMicChoice(choiceBox, mic, { onDone: after })));
      choiceBox.append(card);
    }
  }

  const qBox = html('<div class="stack"></div>');
  wrap.append(qBox);
  function renderQuestions() {
    qBox.innerHTML = '';
    const mic = selectedMic();
    // Real backend: ProfileMatch.questions (API.md). Browser mock: needs_questions.
    // The "which mic is it?" questions (no answer list) are the picker's job now.
    const qs = (mic?.profile?.questions || mic?.profile?.needs_questions || []).filter((q) => !(q.id === 'which_mic_on_interface' || (q.id === 'which_model' && !(q.options && q.options.length))));
    // A GoXLR has no gain knob (gain lives in the GoXLR app), so we never ask; the coach reads
    // or sets it itself. Interfaces with a real knob get a plain "look at it" question.
    const isGoxlr = mic && /goxlr/i.test(mic.name);
    const extra = mic && !isGoxlr && /focusrite|scarlett|wave xlr|rodecaster|umc|interface|analogue/i.test(mic.name)
      ? [{
        id: 'gain_at_max',
        prompt: 'Look at the round knob that controls how loud your mic is. Is it turned almost all the way round to the right?',
        options: ['Yes, almost all the way', 'No, it is lower than that', "I'm not sure"],
        help: {
          what: 'This knob is on the front of the box your mic plugs into. It is often labelled "Gain" or has a small microphone symbol.',
          where: 'Turn so the little line on the knob points to the right. If it points up or to the left, choose "No".',
        },
      }]
      : [];
    const all = [...qs, ...extra];
    if (!all.length) return;
    qBox.append(html(`<div><h2>A couple of quick questions</h2><p class="muted">So we can give advice for your exact gear. Tap an answer. If you are not sure, tap "I'm not sure": we will use safe, standard settings. You can skip these and carry on.</p></div>`));
    for (const q of all) qBox.append(questionCard(q));
  }
  renderMicChoice();
  renderQuestions();

  const next = html(`<div class="row"><button class="btn primary lg">Run the free quick scan</button><span class="subtle">Checks Windows and OBS settings — takes a few seconds.</span></div>`);
  next.querySelector('button').addEventListener('click', () => {
    // A button must never fail silently: if going on throws, say so and log it.
    try {
      // Confirm what is plugged in before we scan and test ("Not sure" counts).
      if (selectedMic()?.needs_mic_choice) {
        renderMicChoice(true);
        choiceBox.scrollIntoView?.({ block: 'center' });
        return;
      }
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
      ? '<ol class="coach-steps"><li>In OBS, open <b>Tools → WebSocket Server Settings</b> (this is OBS's remote-control switch).</li><li>Tick <b>Enable WebSocket server</b> and press OK.</li><li>Come back and press "Check again".</li></ol>'
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

const qKey = (q) => (q.key ? `${q.id}:${q.key}` : q.id);

function applyAnswer(q, value) {
  // value === null clears the answer (tap the chosen button again to change your mind).
  if (value === null) {
    delete state.answers[qKey(q)];
    if (q.id === 'gain_at_max') state.answers.gain_at_max = undefined;
    else if (q.id === 'onboard_processing') state.answers.onboard_auto_level = undefined;
    return;
  }
  state.answers[qKey(q)] = value;
  const yes = value === 'Yes';
  const unsure = /not sure/i.test(value);
  switch (q.id) {
    case 'which_mic_on_interface':
    case 'which_model':
    case 'which_headset_model':
      state.answers.mic_model_id = unsure || /something else/i.test(value) ? undefined : value;
      break;
    case 'booster_in_chain':
      state.answers.booster = unsure ? undefined : yes;
      break;
    case 'onboard_processing':
      state.answers.onboard_auto_level = unsure ? undefined : yes;
      break;
    case 'gain_at_max':
      state.answers.gain_at_max = unsure ? undefined : /^yes/i.test(value);
      break;
    default:
      break;
  }
}

// "What does this mean / where is it?" Plain words only; image and manual link optional.
function helpBlock(h) {
  if (!h || (!h.what && !h.where && !(h.positions || []).length)) return null;
  const safeUrl = /^https:\/\//.test(h.manual_url || '') ? h.manual_url : null;
  const safeImg = /^[\w./-]+$/.test(h.image || '') && !(h.image || '').includes('..') ? h.image : null;
  const pos = (h.positions || []).map((p) => `<li><b>${esc(p.label)}</b>: ${esc(p.meaning)}</li>`).join('');
  const d = html(`
    <details class="q-help">
      <summary>Where is it? What does this mean?</summary>
      <div class="stack" style="gap:8px;margin-top:8px">
        ${h.what ? `<p>${esc(h.what)}</p>` : ''}
        ${h.where ? `<p><b>Where to find it:</b> ${esc(h.where)}</p>` : ''}
        ${safeImg ? `<img src="${esc(safeImg)}" alt="" style="max-width:100%;border-radius:8px">` : ''}
        ${pos ? `<ul class="q-help-pos">${pos}</ul>` : ''}
        ${safeUrl ? `<p><a href="${esc(safeUrl)}" target="_blank" rel="noopener noreferrer" class="q-help-link">Open the maker's page for this mic</a></p>` : ''}
      </div>
    </details>`);
  return d;
}

function questionCard(q) {
  // Every closed question is exclusive and always has an "I'm not sure".
  let opts = q.options && q.options.length ? [...q.options] : ['Yes', 'No'];
  if (!opts.some((o) => /not sure/i.test(o))) opts.push("I'm not sure");
  const card = html(`<div class="card flat stack"><h3>${esc(q.prompt)}</h3><div class="choices" role="radiogroup"></div></div>`);
  const help = helpBlock(q.help);
  if (help) card.append(help);
  const box = card.querySelector('.choices');
  const note = html('<p class="subtle q-note" aria-live="polite"></p>');
  card.append(note);
  const draw = () => {
    box.innerHTML = '';
    const cur = state.answers[qKey(q)];
    for (const o of opts) {
      const on = cur === o;
      const b = html(`<button class="choice ${on ? 'selected' : ''}" role="radio" aria-checked="${on}">${esc(o)}</button>`);
      b.addEventListener('click', () => { applyAnswer(q, on ? null : o); draw(); });
      box.append(b);
    }
    note.textContent = !cur ? ''
      : /not sure/i.test(cur) ? "That's fine. We'll use safe, standard settings and check by listening."
      : 'Got it. Tap it again if you want to change your answer.';
  };
  draw();
  return card;
}
