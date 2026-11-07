// Live coaching: step-by-step, animated, measured as you go. We confirm
// success ourselves (no "did you do it?"), auto-advance, and retest at the end.
import { api, on } from '../api.js';
import { esc, html, icons, toast, errorBanner, advanced, loading } from '../components.js';
import { state, MIC_CONSTRAINT, CAMERA_CONSTRAINT } from '../state.js';
import { TASKS, HINT_TEXT, PHASE_TEXT, TASK_AREA, readingLabel } from '../coach/copy.js';
import { mountIllustration, domainFor, reducedMotion } from '../coach/anim.js';
import { gauge, holdRing, cameraOverlay } from '../coach/widgets.js';
import { levelFixPanel, canFixLevel } from '../levelfix.js';

const AREA_TEXT = { mic: 'Microphone', camera: 'Camera' };
const SUCCESS_MS = 1200;

export async function render(root, { go, refreshNav }) {
  const c = state.coach;
  const page = html('<section class="coach stack-lg"></section>');
  root.append(page);
  if (new URLSearchParams(location.search).get('mini') === '1') document.body.classList.add('mini');

  if (!c) {
    page.append(html('<div class="banner warn"><span>Start coaching from your mic or camera report.</span></div>'));
    const b = html('<button class="btn primary">Go to the mic report</button>');
    b.addEventListener('click', () => go('mic-report'));
    page.append(b);
    return;
  }

  let session = 0;          // bumps on every step change; stale events are ignored
  let unlisten = [];
  let illo = null;
  let overlay = null;
  let advanceTimer = null;
  let destroyed = false;

  if (!c.steps) {
    page.append(loading('Building your coaching plan…'));
    try {
      c.steps = await api.coachPlan(c.area);
      if (c.only) c.steps = c.steps.filter((s) => c.only.includes(s.task)); // quick re-check after a profile switch
      c.index = 0;
      c.status = {};
    } catch (e) {
      page.innerHTML = '';
      page.append(errorBanner(e, () => go('coach')));
      return () => {};
    }
  }
  if (!c.steps.length) { finish(); return cleanup; }
  showStep();
  return cleanup;

  // ------------------------------------------------------------- lifecycle
  async function stopLive() {
    session++;
    clearTimeout(advanceTimer);
    unlisten.forEach((u) => { try { u(); } catch (_) { /* ignore */ } });
    unlisten = [];
    if (illo) { illo.destroy(); illo = null; }
    if (overlay) { overlay.destroy(); overlay = null; }
    try { await api.stopCoach(); } catch (_) { /* nothing running */ }
  }
  function cleanup() {
    destroyed = true;
    document.body.classList.remove('coach-done');
    stopLive();
    // Give a camera we freed from OBS back now that the live coaching is over.
    if (state.cameraFreed) { state.cameraFreed = false; state.keepCameraFree = false; api.restoreFreedCamera().catch(() => {}); }
  }

  // --------------------------------------------------------------- one step
  async function showStep() {
    await stopLive();
    if (destroyed) return;
    const my = session;
    const step = c.steps[c.index];
    const copy = TASKS[step.task] || TASKS.check_setting;
    const anim = step.animation || copy.animation;
    const isCamera = TASK_AREA[step.task] === 'camera';
    const todo = step.auto ? (copy.autoTodo || copy.todo) : copy.todo;
    const outOfApp = !!step.open_target;

    page.innerHTML = '';
    window.scrollTo({ top: 0, behavior: reducedMotion() ? 'auto' : 'smooth' });
    page.append(html(`
      <div class="coach-head">
        <div class="stack" style="gap:6px">
          <span class="eyebrow">Coaching · ${esc(AREA_TEXT[c.area] || '')} · step ${c.index + 1} of ${c.steps.length}</span>
          <h1>${esc(step.title || copy.title)}</h1>
          <p class="lead">${esc(step.explain || copy.why)}</p>
        </div>
      </div>`));

    const grid = html('<div class="coach-grid"></div>');
    page.append(grid);

    // Main live card.
    const card = html(`
      <div class="card coach-card" data-phase="waiting" aria-live="polite">
        <div class="coach-top">
          <span class="chip phase-chip">${esc(PHASE_TEXT.waiting)}</span>
          <span class="chip auto auto-badge hidden"><span class="spinner tiny"></span> Setting it for you</span>
        </div>
        <div class="coach-stage ${isCamera ? 'camera' : ''}">
          <div class="coach-visual"></div>
          <div class="coach-live">
            <div class="coach-headline">Getting ready…</div>
            <p class="coach-detail muted"></p>
            <div class="coach-gauge"></div>
            <div class="coach-meta">
              <div class="coach-progress"><div class="bar"></div></div>
              <div class="coach-hold"></div>
            </div>
          </div>
        </div>
        <div class="coach-success" aria-hidden="true"><div class="burst"></div><div class="tick">${icons.check}</div><b>Nailed it!</b></div>
        <div class="coach-stuck hidden"></div>
        <div class="coach-out hidden"></div>
        <div class="coach-actions row">
          <button class="btn ghost" data-act="back" ${c.index === 0 ? 'disabled' : ''}>Back</button>
          <span style="flex:1"></span>
          <button class="btn" data-act="cant">I can't do this</button>
          <button class="btn" data-act="skip">Skip this step</button>
        </div>
      </div>`);
    grid.append(card);

    // Step list: a compact stepper above the card, so the live view gets the full width.
    const list = html(`<nav class="coach-stepper" aria-label="Coaching steps"><ol>${c.steps.map((s, i) => {
      const st = c.status[s.id] || '';
      return `<li class="${esc(st)} ${i === c.index ? 'now' : ''}" title="${esc(s.title || TASKS[s.task]?.title || s.task)}"><span class="mark">${st === 'done' ? '✓' : st === 'skipped' ? '–' : i + 1}</span><span class="t">${esc(s.title || TASKS[s.task]?.title || s.task)}</span></li>`;
    }).join('')}</ol><button class="btn ghost sm" data-act="exit">Leave coaching</button></nav>`);
    grid.before(list);

    // How-to + what good is like.
    const how = html(`
      <div class="grid-2 coach-how">
        <div class="card stack" style="gap:10px"><h3>What to do</h3>${todo.length ? `<ol class="coach-steps">${todo.map((t) => `<li>${esc(t)}</li>`).join('')}</ol>` : '<p class="muted">Follow the steps in the box above.</p>'}</div>
        <div class="card stack" style="gap:10px"><h3>${isCamera ? 'What good looks like' : 'What good sounds like'}</h3><p class="muted">${esc(sentence(copy.good.replace(/^Good (sounds|looks) like: /, '')))}</p></div>
      </div>`);
    page.append(how);
    if (step.task === 'set_gain' && canFixLevel()) {
      const lf = html('<div class="card"></div>');
      lf.append(levelFixPanel({ deviceId: state.micId, reason: 'Or let us set the level for you', beforeStart: stopLive, onFixed: () => showStep(), onUndone: () => {} }));
      how.before(lf);
    }
    const readingsBox = html('<table class="kv"><tbody></tbody></table>');
    page.append(advanced('Advanced: live numbers', readingsBox));

    // Wire actions.
    card.querySelector('[data-act=back]').addEventListener('click', () => { if (c.index > 0) { c.index--; showStep(); } });
    card.querySelector('[data-act=skip]').addEventListener('click', () => { c.status[step.id] = 'skipped'; next(); });
    card.querySelector('[data-act=cant]').addEventListener('click', (e) => cant(step, e.currentTarget));
    list.querySelector('[data-act=exit]').addEventListener('click', async () => { await stopLive(); if (c.returnTo) { const back = c.returnTo; state.coach = null; go(back); } else go(c.area === 'camera' ? 'camera' : 'mic-report'); });

    // Visual: animated illustration; camera tasks also get the live preview.
    const visual = card.querySelector('.coach-visual');
    if (isCamera) {
      overlay = cameraOverlay();
      visual.append(overlay.node);
      const small = html('<div class="coach-illo small"></div>');
      visual.append(small);
      illo = mountIllustration(anim, small);
    } else {
      const host = html('<div class="coach-illo"></div>');
      visual.append(host);
      illo = mountIllustration(anim, host);
    }
    const g = gauge(copy.lowLabel, copy.highLabel);
    card.querySelector('.coach-gauge').append(g.node);
    if (step.task === 'check_setting') {
      g.node.classList.add('hidden');
      card.querySelector('.coach-meta').classList.add('hidden');
    }
    // Short name of the setting for the animated mini-window, e.g. "Voice Clarity".
    const settingLabel = (step.title || '').replace(/^Turn (off|on) /i, '').replace(/^Windows /i, '').replace(/["“”]/g, '') || 'The setting';
    const ring = holdRing();
    card.querySelector('.coach-hold').append(ring.node);

    const $ = (s) => card.querySelector(s);
    let lastHeadline = '';
    let lastKey = '';
    let opened = !outOfApp;

    // Out-of-app card: explain, open it for them, then watch.
    if (outOfApp) drawOutOfApp();

    function drawOutOfApp(waiting = false) {
      const t = step.open_target;
      const box = $('.coach-out');
      box.classList.remove('hidden');
      card.classList.add('has-out');
      box.innerHTML = '';
      box.append(html(`
        <div class="out-card ${waiting ? 'waiting' : ''}">
          <div class="stack" style="gap:8px">
            <b>${waiting ? `Do this in ${esc(t.label)} — we're watching` : `We'll open ${esc(t.label)} right next to this window`}</b>
            <ol class="coach-steps">${(t.steps || []).map((s) => `<li>${esc(s)}</li>`).join('')}</ol>
            <span class="muted">${waiting ? "No need to come back and tell us — we'll notice the change and bring you back here." : `Just ${t.steps?.length === 1 ? 'one click' : `${t.steps?.length || 'a couple of'} clicks`}. We stay on screen and notice the moment it's done.`}</span>
          </div>
          ${waiting ? '<div class="watching"><span class="pulse"></span>Watching for the change…</div>' : `<button class="btn primary lg" data-act="open">Open ${esc(t.label)} for me</button>`}
        </div>`));
      const b = box.querySelector('[data-act=open]');
      if (b) b.addEventListener('click', async () => {
        b.disabled = true;
        try {
          await api.openForUser(t);
          opened = true;
          drawOutOfApp(true);
        } catch (e) {
          toast("Couldn't open it", e.message, 'bad');
          b.disabled = false;
        }
      });
    }

    function onUpdate(u) {
      if (my !== session || u.task !== step.task) return;
      const dom = domainFor(u, copy.domain);
      card.dataset.phase = u.phase;
      $('.phase-chip').textContent = step.task === 'check_setting' && u.phase === 'working' ? 'Watching' : (PHASE_TEXT[u.phase] || u.phase);
      $('.auto-badge').classList.toggle('hidden', !u.auto_adjusting);
      const hintText = u.hint?.text || HINT_TEXT[u.hint?.kind] || '';
      let headline = u.headline || hintText || PHASE_TEXT[u.phase];
      if (outOfApp && !opened && u.phase !== 'done') headline = 'Ready when you are';
      if (headline !== lastHeadline) {
        const h = $('.coach-headline');
        h.textContent = headline;
        // Animate only when the situation changes (phase/hint), not on every wording tweak.
        const key = `${u.phase}|${u.hint?.kind}`;
        if (!reducedMotion() && key !== lastKey) { h.classList.remove('pop'); void h.offsetWidth; h.classList.add('pop'); }
        lastKey = key;
        lastHeadline = headline;
      }
      $('.coach-detail').textContent = u.detail || '';
      $('.coach-progress .bar').style.width = `${Math.round(Math.max(0, Math.min(1, u.progress || 0)) * 100)}%`;
      g.update(u, dom);
      ring.update(u);
      if (illo) illo.update(anim === 'settings' ? { ...u, detail_setting: settingLabel } : u, dom);
      // Readings for the curious.
      const rows = Object.entries(u.readings || {}).filter(([, v]) => typeof v === 'number');
      readingsBox.querySelector('tbody').innerHTML = [
        ['Live value', u.value, u.target?.unit],
        ['Target', u.target ? `${fmtN(u.target.min)} … ${fmtN(u.target.max)}` : null, u.target?.unit],
        ...rows.map(([k, v]) => [readingLabel(k), v, '']),
      ].filter((r) => r[1] !== null && r[1] !== undefined)
        .map(([k, v, unit]) => `<tr><th>${esc(k)}</th><td>${esc(typeof v === 'number' ? fmtN(v) : v)} ${esc(unit || '')}</td></tr>`).join('');

      const stuck = $('.coach-stuck');
      if (u.phase === 'stuck') {
        if (stuck.classList.contains('hidden')) {
          stuck.classList.remove('hidden');
          stuck.innerHTML = '';
          stuck.append(html(`<div class="stuck-card"><div class="stack" style="gap:6px"><b>Tricky one? Here's another way</b><span>${esc(u.stuck_alternative || "Some setups just can't change this — that's fine. Tell us and we'll tune your settings around it.")}</span></div>
            <div class="row"><button class="btn primary" data-act="retry">Try another way</button><button class="btn" data-act="cant2">I can't do this</button></div></div>`));
          stuck.querySelector('[data-act=retry]').addEventListener('click', () => showStep());
          stuck.querySelector('[data-act=cant2]').addEventListener('click', (e) => cant(step, e.currentTarget));
        }
      } else stuck.classList.add('hidden');

      if (u.phase === 'done') success();
    }

    function success() {
      if (card.classList.contains('success')) return;
      card.classList.add('success');
      c.status[step.id] = 'done';
      const li = list.querySelectorAll('li')[c.index];
      if (li) { li.classList.add('done'); li.querySelector('.mark').textContent = '✓'; }
      advanceTimer = setTimeout(() => { if (my === session) next(); }, SUCCESS_MS);
    }

    unlisten.push(await on('coach-update', onUpdate));
    if (isCamera) unlisten.push(await on('coach-camera-overlay', (d) => { if (my === session && overlay) overlay.update(d); }));
    if (isCamera) {
      unlisten.push(await on('camera-picture', (p) => {
        if (my !== session || p.live) return;
        // Never coach from a black picture: say so instead.
        $('.coach-headline').textContent = 'We lost sight of your camera';
        $('.coach-detail').textContent = "Another app may have grabbed it. Go back to the camera check and we'll sort it out.";
      }));
    }
    if (my !== session || destroyed) return;
    try {
      await api.startCoach(step);
    } catch (e) {
      if (my !== session) return;
      $('.coach-headline').textContent = "We couldn't start this step";
      $('.coach-detail').textContent = e.message;
      return;
    }
    // Came from a "Show me how" button: the user already asked, so open the page now.
    if (c.autoOpen && outOfApp && !opened && my === session) {
      try {
        await api.openForUser(step.open_target);
        opened = true;
        if (my === session) drawOutOfApp(true);
      } catch (e) {
        if (my === session) toast("Couldn't open it", e.message, 'bad');
      }
    }
  }

  function next() {
    if (c.index < c.steps.length - 1) { c.index++; showStep(); } else finish();
  }

  async function cant(step, btn) {
    const map = c.area === 'camera' ? CAMERA_CONSTRAINT : MIC_CONSTRAINT;
    const constraint = step.finding_id ? map[step.finding_id] : null;
    btn.disabled = true;
    c.status[step.id] = 'skipped';
    if (c.returnTo) {
      // A Windows setting we couldn't change: recalibrate (fresh checks, and the mic advice if there is one) and go back.
      await stopLive();
      try {
        if (state.micReport) state.micReport = await api.rescanMic(state.micContext, state.micConstraints);
      } catch (_) { /* the advice just stays as it was */ }
      toast('No problem', "We'll leave that one and tune around it.", 'good');
      return leaveToStart(false);
    }
    if (constraint) {
      try {
        if (c.area === 'camera') {
          if (!state.cameraConstraints.includes(constraint)) state.cameraConstraints.push(constraint);
          state.cameraReport = await api.finishCameraTest(state.cameraConstraints);
        } else {
          if (!state.micConstraints.includes(constraint)) state.micConstraints.push(constraint);
          state.micReport = await api.rescanMic(state.micContext, state.micConstraints);
        }
        toast('No problem', "We've re-tuned your settings to work around that.", 'good');
      } catch (e) {
        toast("Couldn't re-tune", e.message, 'bad');
      }
    } else {
      toast('No problem', "We'll skip this one — your settings will still help.", 'good');
    }
    next();
  }

  // Back to the screen whose "Show me how" button started this: re-run the
  // check so a fixed setting drops off the list on its own.
  async function leaveToStart(fixed) {
    const back = c.returnTo;
    state.coach = null;
    if (fixed) {
      page.innerHTML = '';
      page.append(html(`<div class="card hero coach-finish"><div class="tick big">${icons.check}</div><h1>That's fixed</h1><p class="lead">We saw the change in Windows. Re-checking your settings…</p></div>`));
    }
    try {
      state.quick = await api.runQuickChecks(state.micId);
      state.applied = null;
      refreshNav();
    } catch (e) {
      toast("Couldn't re-check", e.message, 'bad');
    }
    go(back);
  }

  // ------------------------------------------------------------ all done
  async function finish() {
    await stopLive();
    if (destroyed) return;
    if (c.backTo) {
      const back = c.backTo;
      const n = c.steps.length;
      state.coach = null;
      toast(n ? 'Level and distance checked' : 'Nothing to adjust', n ? 'Your mic is set for this profile.' : 'Your level and distance already fit this profile.', 'good');
      return go(back);
    }
    if (c.returnTo) {
      const fixedIt = c.steps.every((s) => c.status[s.id] === 'done');
      return fixedIt ? leaveToStart(true) : leaveToStart(false);
    }
    const done = c.steps.filter((s) => c.status[s.id] === 'done').length;
    const area = c.area;
    page.innerHTML = '';
    const card = html(`
      <div class="card hero coach-finish">
        <div class="tick big">${icons.check}</div>
        <h1>${done ? 'Great work!' : 'All set'}</h1>
        <p class="lead">${done ? `You fixed ${done} thing${done === 1 ? '' : 's'} by hand — those matter more than any setting.` : "We'll tune your settings around your setup."} Let's ${area === 'camera' ? 'look at your camera' : 'listen'} again to see the difference.</p>
        <ul class="coach-summary">${c.steps.map((s) => `<li class="${esc(c.status[s.id] || '')}"><span class="mark">${c.status[s.id] === 'done' ? '✓' : '–'}</span>${esc(s.title || TASKS[s.task]?.title || s.task)}</li>`).join('')}</ul>
        <div class="row"><button class="btn primary lg" data-act="go">Check again now</button><button class="btn ghost" data-act="later">Not now</button></div>
        <span class="subtle" data-countdown>Starting in 4…</span>
      </div>`);
    page.append(card);
    const before = area === 'camera' ? state.cameraReport?.score?.value : state.micReport?.score?.value;
    const startRetest = () => {
      clearInterval(timer);
      state.scoreBefore = { ...(state.scoreBefore || {}), [area]: before ?? null };
      state.autoStart = area;
      if (area === 'camera') state.cameraReport = null;
      state.coach = null;
      go(area === 'camera' ? 'camera' : 'mic');
    };
    let n = 4;
    const cd = card.querySelector('[data-countdown]');
    const timer = setInterval(() => { n--; if (n <= 0) startRetest(); else cd.textContent = `Starting in ${n}…`; }, 1000);
    card.querySelector('[data-act=go]').addEventListener('click', startRetest);
    card.querySelector('[data-act=later]').addEventListener('click', () => { clearInterval(timer); state.coach = null; go(area === 'camera' ? 'camera' : 'mic-report'); });
    unlisten.push(() => clearInterval(timer));
    refreshNav();
  }
}

function fmtN(v) {
  if (typeof v !== 'number' || !Number.isFinite(v)) return String(v ?? '—');
  return Math.abs(v) >= 100 ? v.toFixed(0) : v.toFixed(1);
}

function sentence(t) {
  return t ? t.charAt(0).toUpperCase() + t.slice(1) : t;
}
