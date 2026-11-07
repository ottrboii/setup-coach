// 11. Make it sound better: tune the EQ and de-esser to your mic and voice, check it offline, hear Before / After.
// Contract: app/API.md "Make it sound better". Plain filters only; numbers only under "Advanced".
import { api, on } from '../api.js';
import { esc, html, icons, toast, advanced, kvTable, fmt } from '../components.js';
import { state, selectedMic } from '../state.js';
import { SCRIPT, cleanAnswers } from './mictest.js';

const READ_SECS = 20;
const reduceMotion = () => window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
const sleep = (ms) => new Promise((r) => setTimeout(r, reduceMotion() ? Math.min(ms, 30) : ms));

const TONE_WORDS = { too_much: 'a bit much', on_target: 'about right', too_little: 'a bit low' };
const HEADLINE_KIND = { pass: 'good', close: 'warn', nothing_to_change: 'info', not_improved: 'info' };
const ICON_WARN = '<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 7v6M12 17h.01"/><circle cx="12" cy="12" r="9"/></svg>';
const ICON_HEADPHONES = '<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 15v-3a8 8 0 0116 0v3"/><rect x="3" y="14" width="4" height="7" rx="1.5"/><rect x="17" y="14" width="4" height="7" rx="1.5"/></svg>';

/** Rejections can be Errors, {code,message} objects (VoiceTuneError) or plain strings. */
function parseErr(e) {
  if (e && typeof e === 'object' && e.code) return { code: e.code, message: e.message || '' };
  if (e && e.message) return { code: 'other', message: e.message };
  return { code: 'other', message: typeof e === 'string' ? e : '' };
}

export async function render(root, { go }) {
  let alive = true;
  let unlisten = null;
  let timer = null;
  let playing = null; // 'tune_before' | 'tune_after' while a clip plays
  let result = null;  // VoiceTuneResult now shown
  let targetInfo = null;

  const page = html('<section class="stack-lg tune" aria-labelledby="tune-h"></section>');
  root.append(page);
  const mic = selectedMic();

  if (!state.env || !state.micId) {
    page.append(html('<div class="banner warn"><span>Pick your microphone first, so we know whose voice to tune.</span></div>'));
    const b = html('<button class="btn primary">Go to the mic test</button>');
    b.addEventListener('click', () => go('mic'));
    page.append(b);
    return;
  }

  if (state.profile === 'voiceover') {
    try { targetInfo = await api.getVoiceoverTarget(); } catch (_) { targetInfo = null; }
  }
  drawIntro();

  // ---------- helpers ----------
  function setView(inner) {
    stopClip();
    page.innerHTML = '';
    page.append(html(`<div class="stack" style="gap:6px"><span class="eyebrow">Microphone</span><h1 id="tune-h" tabindex="-1">Make it sound better</h1></div>`));
    page.append(inner);
  }
  function focusHeading() {
    const h = page.querySelector('#tune-h');
    if (h) h.focus({ preventScroll: true });
  }
  function teardown() {
    clearInterval(timer);
    if (unlisten) { unlisten(); unlisten = null; }
  }
  function stopClip() {
    if (playing) { playing = null; api.stopPreview().catch(() => {}); }
  }

  // ---------- 1. intro ----------
  function drawIntro(note) {
    const box = html(`<div class="stack-lg">
      <p class="lead">We listen to how your mic colours your voice, then adjust the equaliser and the harsh "s" sounds a little toward the targets for your setup. We check it on your own recording first, and you hear Before and After before anything is kept. Nothing changes until you press Keep it, and you can undo it any time.</p>
      <div id="note"></div>
      <div id="target-slot"></div>
      <div class="card stack">
        <div class="device"><div class="ico">${icons.mic}</div><div class="stack" style="gap:2px"><b>${esc(mic ? mic.name : 'Your microphone')}</b>
          <span class="subtle">${mic && mic.needs_mic_choice ? 'We are not sure which mic this is yet. You can still go ahead.' : 'Sit like you do when you talk or record.'}</span></div></div>
        <div class="row"><button class="btn primary lg" id="go">${icons.spark}<span>Listen to my voice</span></button>
          <button class="btn ghost" id="back">Back to my report</button></div>
      </div></div>`);
    if (note) box.querySelector('#note').append(html(`<div class="banner info"><span>${esc(note)}</span></div>`));
    if (targetInfo) box.querySelector('#target-slot').append(targetPicker(targetInfo, async (t) => { targetInfo = t; }));
    box.querySelector('#go').addEventListener('click', start);
    box.querySelector('#back').addEventListener('click', () => go(state.micReport ? 'mic-report' : 'mic'));
    setView(box);
    focusHeading();
  }

  // ---------- voiceover goal picker (only for the voiceover profile) ----------
  function targetPicker(info, onChanged) {
    const card = html(`<fieldset class="card stack tune-targets"><legend class="tune-legend">What are you making?</legend>
      <div class="tune-target-list" role="radiogroup" aria-label="Voiceover goal"></div>
      <p class="subtle tune-target-info" aria-live="polite"></p></fieldset>`);
    const list = card.querySelector('.tune-target-list');
    const infoLine = card.querySelector('.tune-target-info');
    const name = `tune-target-${Math.random().toString(36).slice(2, 7)}`;
    const note = (i, changed) => {
      infoLine.textContent = i.needs_rescan && result ? 'You changed the goal, so we will listen again before anything is applied.'
        : i.applies_now ? 'This sets the loudness of your voiceover settings right away.'
          : 'This goal is used when your setup is Voiceover.';
      if (changed && !(i.needs_rescan && result)) infoLine.textContent += ' Saved.';
    };
    for (const o of info.options) {
      const row = html(`<label class="tune-target"><input type="radio" name="${name}" value="${esc(o.id)}" ${o.id === info.selected ? 'checked' : ''}/>
        <span class="stack" style="gap:2px"><b>${esc(o.label)}</b><span class="subtle">${esc(o.summary)}</span></span></label>`);
      row.querySelector('input').addEventListener('change', async (ev) => {
        const inputs = list.querySelectorAll('input');
        inputs.forEach((i) => { i.disabled = true; });
        try {
          const next = await api.setVoiceoverTarget(ev.target.value);
          info = next;
          note(next, true);
          await onChanged(next);
          if (next.needs_rescan && result) { toast('Goal changed', 'Listening again with the new goal.', 'info'); await start(true); return; }
        } catch (e) {
          toast("Couldn't change the goal", parseErr(e).message || 'Nothing was changed.', 'bad');
          const prev = list.querySelector(`input[value="${info.selected}"]`);
          if (prev) prev.checked = true;
        }
        inputs.forEach((i) => { i.disabled = false; });
      });
      list.append(row);
    }
    note(info, false);
    return card;
  }

  // ---------- 2. start (and the Read step when needed) ----------
  async function start(again = false) {
    setView(workingView('Listening to your voice', 'This takes a second or two. Nothing is changed yet.'));
    focusHeading();
    try {
      result = await api.startVoiceTune(state.micId);
    } catch (e) {
      if (!alive) return;
      const err = parseErr(e);
      if (err.code === 'needs_read') return readStep();
      return drawError(err, () => start());
    }
    if (!alive) return;
    await reveal(result, again);
  }

  function workingView(title, text) {
    return html(`<div class="card stack" role="status" aria-live="polite"><div class="lf-run" style="border:0;padding:0;background:none">
      <div class="lf-wave" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i><i></i><i></i></div>
      <div class="stack" style="gap:2px;flex:1;min-width:200px"><b>${esc(title)}</b><span class="subtle">${esc(text)}</span></div></div></div>`);
  }

  async function readStep() {
    const view = html(`<div class="card stack-lg">
      <div class="spread" style="align-items:flex-start">
        <div class="stack" style="gap:8px;flex:1"><div class="prompt" id="rt">Get ready...</div>
          <p class="muted" id="rx">First we need to hear your voice. Read the lines below out loud, in your normal voice.</p></div>
        <div class="countdown" id="rc" aria-hidden="true"></div></div>
      <div class="script hidden" id="rs">${SCRIPT.replace(/\n/g, '<br/>')}</div>
      <div class="lf-run" id="rw" style="border:0;padding:0;background:none"><div class="lf-wave" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i><i></i><i></i></div>
        <span class="subtle" id="rh" aria-live="polite">Starting your mic...</span></div>
      <div class="row"><button class="btn ghost" id="cancel">Cancel</button></div></div>`);
    setView(view);
    focusHeading();
    const bars = [...view.querySelectorAll('.lf-wave i')];
    let cancelled = false;
    view.querySelector('#cancel').addEventListener('click', () => { cancelled = true; teardown(); drawIntro(); });
    try {
      state.micContext = await api.micContext(state.micId, cleanAnswers());
      unlisten = await on('mic-level', (l) => {
        view.querySelector('#rw').classList.add('live');
        const base = Math.max(0, Math.min(1, (l.rms_db + 60) / 50));
        bars.forEach((b, i) => { b.style.transform = `scaleY(${(0.15 + base * (0.55 + 0.45 * Math.abs(Math.sin(i * 1.7 + performance.now() / 160)))).toFixed(2)})`; });
      });
      await api.startMicTest(state.micId);
      const count = view.querySelector('#rc');
      count.textContent = '...';
      await sleep(1500);
      if (cancelled || !alive) return;
      view.querySelector('#rt').textContent = 'Read this out loud';
      view.querySelector('#rx').textContent = 'Use your normal voice, at the distance you normally sit.';
      view.querySelector('#rs').classList.remove('hidden');
      view.querySelector('#rh').textContent = 'Listening...';
      await api.markStep('read', 'start');
      await new Promise((resolve) => {
        let left = READ_SECS;
        count.textContent = left;
        timer = setInterval(() => {
          left -= 1;
          if (cancelled || left <= 0) { clearInterval(timer); resolve(); return; }
          count.textContent = left;
        }, reduceMotion() ? 120 : 1000);
      });
      if (cancelled || !alive) return;
      await api.markStep('read', 'end');
      teardown();
      view.querySelector('#rt').textContent = 'Thanks, got it';
      view.querySelector('#rx').textContent = 'Checking your recording...';
      view.querySelector('#rs').classList.add('hidden');
      count.textContent = '';
      state.micReport = await api.finishMicTest(state.micContext, state.micConstraints);
      state.micVerify = null;
      if (!alive) return;
      await start(); // the Read now counts, so this time it goes through
    } catch (e) {
      teardown();
      if (cancelled || !alive) return;
      drawError(parseErr(e), () => readStep());
    }
  }

  // ---------- 3. result ----------
  /** Facts one by one, then the rest. */
  async function reveal(res, again) {
    const view = html('<div class="stack-lg"></div>');
    const facts = html(`<div class="card stack tune-facts" aria-label="What we found"><ul class="tune-fact-list"></ul></div>`);
    const ul = facts.querySelector('ul');
    setView(view);
    view.append(facts);
    focusHeading();
    for (const key of ['voice', 'mic', 'goal']) {
      const f = res.facts && res.facts[key];
      if (!f) continue;
      const li = html(`<li class="tune-fact">${icons.check}<span>${esc(f)}</span></li>`);
      ul.append(li);
      await sleep(again ? 150 : 500);
      if (!alive) return;
    }
    await sleep(200);
    if (!alive) return;
    draw(res, view, facts);
  }

  function draw(res, view = null, factsCard = null) {
    result = res;
    const keep = !!res.can_apply;
    if (!view) {
      view = html('<div class="stack-lg"></div>');
      setView(view);
      factsCard = html(`<div class="card stack tune-facts"><ul class="tune-fact-list"></ul></div>`);
      for (const key of ['voice', 'mic', 'goal']) if (res.facts && res.facts[key]) factsCard.querySelector('ul').append(html(`<li class="tune-fact">${icons.check}<span>${esc(res.facts[key])}</span></li>`));
    }
    view.innerHTML = '';
    factsCard.classList.add('settled');
    const kind = HEADLINE_KIND[res.verdict] || 'info';
    const title = res.verdict === 'nothing_to_change' ? 'Your sound is already good here'
      : res.verdict === 'not_improved' ? "We couldn't improve it safely" : null;
    const head = html(`<div class="banner ${kind} tune-headline" role="status"><span class="tune-ico">${(kind === 'warn' || res.verdict === 'not_improved') ? ICON_WARN : icons.check}</span>
      <div class="stack" style="gap:4px;flex:1">${title ? `<b>${esc(title)}</b>` : ''}<span class="${title ? '' : 'tune-head-text'}">${esc(res.headline)}</span>
      ${res.applied ? `<span class="chip ok" style="align-self:flex-start">Kept${res.strength < 1 ? ' (gentler)' : ''}</span>` : ''}</div></div>`);
    view.append(head);
    view.append(factsCard);
    if (targetInfo && state.profile === 'voiceover') view.append(targetPicker(targetInfo, async (t) => { targetInfo = t; }));

    if (!keep) {
      view.append(html(`<p class="muted">${res.verdict === 'nothing_to_change'
        ? 'Your voice and mic already sit close to the targets, so we left your settings alone.'
        : 'The change we tried did not measure better on your own recording, so we did not touch anything. Your current settings stay as they are.'}</p>`));
      for (const n of res.notes || []) view.append(html(`<div class="banner info"><span>${esc(n)}</span></div>`));
      view.append(footerActions(res, false));
      view.append(advancedBlock(res));
      return;
    }

    if (res.changes?.length) {
      view.append(html(`<div class="card stack"><h3>What we would change</h3><ul class="plain-list">${res.changes.map((c) => `<li>${esc(c)}</li>`).join('')}</ul></div>`));
    }
    if (res.summary?.length) {
      const lines = res.summary.map((t) => `<li class="tone ${esc(t.after)}"><span class="tone-dot" aria-hidden="true"></span><span class="tone-text">${esc(t.text || `${t.label}: ${TONE_WORDS[t.before] || ''}`)}</span></li>`).join('');
      view.append(html(`<div class="card stack"><h3>How your voice sounds, before and after</h3><ul class="tone-list">${lines}</ul></div>`));
    }
    for (const n of res.notes || []) view.append(html(`<div class="banner info"><span>${esc(n)}</span></div>`));
    if (res.before_path && res.after_path) view.append(player(res));
    view.append(footerActions(res, true));
    view.append(advancedBlock(res));
  }

  // ---------- Before / After ----------
  function player(res) {
    let which = 'tune_before';
    const name = `tune-ab-${Math.random().toString(36).slice(2, 7)}`;
    const box = html(`<div class="card stack tune-player" aria-label="Before and after">
      <h3>Hear it</h3>
      <p class="subtle tune-hp">${ICON_HEADPHONES}<span>Headphones work best, so you hear your voice and not your speakers. Both clips are matched to the same loudness, so a louder one can't fool you.</span></p>
      <div class="tune-ab" role="radiogroup" aria-label="Which clip">
        <label class="tune-seg"><input type="radio" name="${name}" value="tune_before" checked/><span><b>Before</b><small>Your voice as it is now</small></span></label>
        <label class="tune-seg after"><input type="radio" name="${name}" value="tune_after"/><span><b>After</b><small>With the new settings</small></span></label>
      </div>
      <div class="row"><button class="btn primary lg" id="play" aria-pressed="false"><span class="ico">${icons.play}</span><span class="lbl">Play Before</span></button>
        <label class="toggle"><input type="checkbox" id="phone"/> <span>On a phone speaker</span></label></div>
      <p class="subtle" id="now" aria-live="polite"></p></div>`);
    const playBtn = box.querySelector('#play');
    const phone = box.querySelector('#phone');
    const now = box.querySelector('#now');
    const label = () => (which === 'tune_before' ? 'Before' : 'After');
    const paint = () => {
      playBtn.setAttribute('aria-pressed', playing ? 'true' : 'false');
      playBtn.querySelector('.ico').innerHTML = playing ? icons.stop : icons.play;
      playBtn.querySelector('.lbl').textContent = playing ? `Stop (playing ${label()})` : `Play ${label()}`;
      now.textContent = playing ? `Now playing: ${label()}. Switch above to compare at the same spot.` : '';
    };
    async function playNow() {
      try {
        await api.stopPreview();
        await api.playPreview(which, phone.checked);
        playing = which;
      } catch (e) {
        playing = null;
        toast("Couldn't play it", parseErr(e).message, 'bad');
      }
      paint();
    }
    playBtn.addEventListener('click', async () => {
      if (playing) { stopClip(); paint(); return; }
      await playNow();
    });
    box.querySelectorAll('input[type=radio]').forEach((r) => r.addEventListener('change', async () => {
      which = r.value;
      if (playing) await playNow(); else paint();
      if (!playing) paint();
    }));
    phone.addEventListener('change', () => { if (playing) playNow(); });
    paint();
    return box;
  }

  // ---------- Keep it / Gentler / Undo ----------
  function footerActions(res, canKeep) {
    const wrap = html('<div class="stack"><div id="msg" role="status" aria-live="polite"></div><div class="row tune-actions"></div></div>');
    const row = wrap.querySelector('.tune-actions');
    const msg = wrap.querySelector('#msg');
    const busy = (btn, text) => { row.querySelectorAll('button').forEach((b) => { b.disabled = true; }); btn.textContent = text; };

    if (canKeep && !res.applied) {
      const keep = html('<button class="btn primary lg" id="keep">Keep it</button>');
      keep.addEventListener('click', async () => {
        busy(keep, 'Saving...');
        try {
          const done = await api.applyVoiceTune(res.tune_id);
          const failed = done.applied?.failed || [];
          // Honest: only claim "kept" when the backend made a backup, or say what did not work.
          if (!done.backup_id || failed.length) {
            draw(res);
            wrapMsg(failed.length ? "Part of it couldn't be saved, so we stopped. Your old settings are still in place." : 'We could not make a safety copy of your settings first, so nothing was changed.', 'warn');
            return;
          }
          draw({ ...result, applied: true, backup_id: done.backup_id });
          wrapMsg(done.line || 'Done. You can undo this any time.', 'good', done.applied?.restart_obs_needed ? ' Restart OBS to load it.' : '');
        } catch (e) {
          failKeep(parseErr(e));
        }
      });
      row.append(keep);
    }
    if (canKeep && res.can_be_gentler) {
      const g = html('<button class="btn lg" id="gentler">Gentler</button>');
      g.title = 'Same idea, with smaller changes';
      g.addEventListener('click', async () => {
        busy(g, 'Making it gentler...');
        try {
          const next = await api.gentlerVoiceTune(res.tune_id);
          const keepBackup = res.backup_id; // the undo point stays the first one
          draw({ ...next, backup_id: next.backup_id || keepBackup });
          wrapMsg('Here is a gentler version. Press Play to hear it' + (keepBackup ? ', then Keep it to switch to it.' : '.'), 'info');
        } catch (e) {
          failKeep(parseErr(e));
        }
      });
      row.append(g);
    }
    if (res.backup_id) {
      const u = html('<button class="btn lg" id="undo">Undo</button>');
      u.addEventListener('click', async () => {
        busy(u, 'Putting it back...');
        try {
          const r = await api.undoVoiceTune(res.backup_id);
          const bad = [...(r?.failed || []), ...(r?.skipped || [])];
          if (bad.length) {
            draw(res);
            wrapMsg(`We couldn't put everything back. ${bad.map((x) => x.error).filter(Boolean)[0] || ''}`.trim(), 'warn');
            return;
          }
          draw({ ...result, applied: false, backup_id: null });
          wrapMsg('Put back. Your voice settings are how they were before.', 'info');
        } catch (e) {
          draw(res);
          wrapMsg(parseErr(e).message || "We couldn't put it back. Nothing else was changed.", 'warn');
        }
      });
      row.append(u);
    }
    if (!canKeep) {
      const again = html('<button class="btn lg" id="again">Listen again</button>');
      again.addEventListener('click', () => start());
      row.append(again);
    }
    const next = html(`<button class="btn ghost lg" id="done">${canKeep ? 'Done' : 'Back to my report'}</button>`);
    next.addEventListener('click', () => go(state.micReport ? 'mic-report' : 'home'));
    row.append(next);
    return wrap;
  }

  function wrapMsg(text, kind, extra = '') {
    const m = page.querySelector('#msg');
    if (m) m.innerHTML = `<div class="banner ${kind}"><span>${esc(text + extra)}</span></div>`;
  }

  function failKeep(err) {
    if (err.code === 'stale') { toast('Out of date', 'Something changed, so we are listening again.', 'info'); return start(true); }
    if (err.code === 'nothing_to_apply') { draw({ ...result, can_apply: false }); return wrapMsg('Nothing needed changing, so nothing was changed.', 'info'); }
    if (err.code === 'mic_missing') { draw(result); return wrapMsg('We cannot find your mic. Plug it back in, then try again.', 'warn'); }
    draw(result);
    wrapMsg(err.message || 'Something went wrong, and nothing was changed.', 'warn');
  }

  function drawError(err, retry) {
    const wording = {
      mic_missing: ['We cannot find your mic', 'Plug it back in (or switch it on), then try again. Nothing was changed.'],
      stale: ['That result is out of date', 'Something changed since we listened. Listen again to get a fresh one.'],
      nothing_to_apply: ['Nothing to change', 'Your settings are fine as they are.'],
    }[err.code] || ["That didn't work", err.message || 'Something went wrong, and nothing was changed.'];
    const box = html(`<div class="stack"><div class="banner bad" role="alert"><div class="stack" style="gap:4px"><b>${esc(wording[0])}</b><span>${esc(wording[1])}</span></div></div>
      <div class="row"><button class="btn primary lg" id="retry">Try again</button><button class="btn ghost lg" id="back">Back</button></div></div>`);
    box.querySelector('#retry').addEventListener('click', retry);
    box.querySelector('#back').addEventListener('click', () => drawIntro());
    setView(box);
    focusHeading();
  }

  // ---------- Advanced: all the numbers live here ----------
  function advancedBlock(res) {
    const a = res.advanced || {};
    const inner = html('<div class="stack"></div>');
    if (a.verification_note) inner.append(html(`<p class="muted">${esc(a.verification_note)}</p>`));
    const v = a.voice;
    const rows = [];
    if (v) {
      rows.push(['Voice pitch', `${fmt(v.f0_median_hz, 0, ' Hz')} (${v.pitch || 'n/a'})`], ['Tilt', `${fmt(v.tilt_db_per_oct, 1, ' dB/oct')} (${v.tilt || 'n/a'})`],
        ['"S" sounds', `${fmt(v.sib_level_db, 1, ' dB')} (${v.sibilance || 'n/a'})`], ['Pops per minute', fmt(v.plosives_per_min, 0)], ['Confidence', v.confidence || 'n/a']);
    }
    if (a.mic) rows.push(['Mic chart', `${a.mic.name || 'unknown'}${a.mic.has_curve ? (a.mic.verified ? ', verified' : ', maker chart (not verified)') : ', no chart'}`], ['Chart source', a.mic.source || 'n/a']);
    if (typeof a.loudness_match_db === 'number') rows.push(['After turned by (to match Before)', fmt(a.loudness_match_db, 1, ' dB')]);
    if (a.curves) rows.push(['Fit error', fmt(a.curves.fit_error_db, 2, ' dB')]);
    if (a.hpf) rows.push(['High-pass now / suggested', `${fmt(a.hpf.current_hz, 0, ' Hz')} / ${fmt(a.hpf.proposed_hz, 0, ' Hz')}`]);
    if (a.fallback) rows.push(['Voice not used because', typeof a.fallback === 'string' ? a.fallback : Object.keys(a.fallback)[0]]);
    if (a.tuning?.loudness) rows.push(['Loudness goal', `${a.tuning.loudness.value} ${a.tuning.loudness.measure === 'rms' ? 'dB RMS' : 'LUFS'}`], ['Limiter ceiling', fmt(a.tuning.limiter_ceiling_db, 1, ' dBFS')]);
    const ver = a.verification;
    if (ver) rows.push(['Checks', [['tone', ver.v1_tone], ['no harm', ver.v2_no_harm], ['level', ver.v3_level], ['cap', ver.v4_cap]].map(([k, ok]) => `${k}: ${ok ? 'pass' : 'no'}`).join(', ')], ['Dropped', (ver.dropped || []).join(', ') || 'nothing']);
    if (rows.length) inner.append(kvTable(rows));
    if (a.bands?.length) {
      inner.append(html(`<table class="kv"><thead><tr><th>EQ band</th><th>Freq</th><th>Gain</th><th>Q</th><th>From</th></tr></thead><tbody>${a.bands.map((b) => `<tr><td>${esc(b.band.kind)}</td><td>${fmt(b.band.freq_hz, 0, ' Hz')}</td><td>${fmt(b.band.gain_db, 1, ' dB')}</td><td>${fmt(b.band.q, 1)}</td><td>${esc(b.origin)}</td></tr>`).join('')}</tbody></table>`));
    }
    if (a.deesser) inner.append(html(`<pre class="json">${esc(JSON.stringify(a.deesser, null, 2))}</pre>`));
    if (a.curves?.freqs_hz?.length) inner.append(curveSvg(a.curves));
    return advanced('Advanced: the numbers', inner);
  }

  function curveSvg(c) {
    const W = 420; const H = 140; const L = 34; const B = 18;
    const n = c.freqs_hz.length;
    const x = (i) => L + ((Math.log(c.freqs_hz[i]) - Math.log(c.freqs_hz[0])) / (Math.log(c.freqs_hz[n - 1]) - Math.log(c.freqs_hz[0]))) * (W - L - 6);
    const all = [...c.mic_db, ...c.net_db, 0];
    const lo = Math.min(...all, -3) - 0.5; const hi = Math.max(...all, 3) + 0.5;
    const y = (d) => 6 + (1 - (d - lo) / (hi - lo)) * (H - B - 6);
    const path = (arr) => arr.map((d, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)} ${y(d).toFixed(1)}`).join(' ');
    return html(`<figure class="tune-curve"><svg viewBox="0 0 ${W} ${H}" role="img" aria-label="Curves: how your mic colours sound and the correction we would apply">
      <line x1="${L}" x2="${W - 6}" y1="${y(0)}" y2="${y(0)}" class="c-zero"/>
      <path d="${path(c.mic_db)}" class="c-mic"/><path d="${path(c.net_db)}" class="c-net"/>
      <text x="${L - 4}" y="${y(0) + 4}" text-anchor="end" class="c-t">0</text>
      <text x="${L}" y="${H - 3}" class="c-t">${c.freqs_hz[0]} Hz</text><text x="${W - 6}" y="${H - 3}" text-anchor="end" class="c-t">${c.freqs_hz[n - 1]} Hz</text></svg>
      <figcaption class="subtle"><span class="key mic"></span> your mic's colour &nbsp; <span class="key net"></span> correction applied</figcaption></figure>`);
  }

  return () => { alive = false; teardown(); stopClip(); };
}
