// 2. Quick checks — the free scan of Windows + OBS settings.
import { api } from '../api.js';
import { esc, html, scoreRing, findingCard, loading, errorBanner, toast, advanced } from '../components.js';
import { state, selectedMic } from '../state.js';
import { openMicChoice, yourMicLine } from '../micpicker.js';
import { guidedFailures, plainFailures, guidedFixCard } from '../guided.js';

const AREA_LABEL = { mic: 'Mic settings', system: 'Windows', camera: 'Camera', stream: 'Stream' };
// Findings fixed by hand in Windows Sound settings when we lack the rights to do it.
const SOUND_SETTINGS_FOR = new Set([
  'system.exclusive_mode_allowed', 'system.listen_to_device_on', 'system.audio_enhancements_on', 'system.voice_clarity_on', 'system.sample_rate_mismatch', 'system.mic_boost_on',
]);

export async function render(root, { go, refreshNav }) {
  const mic = selectedMic();
  root.append(html(`
    <section class="stack-lg">
      <div class="spread">
        <div class="stack" style="gap:6px"><span class="eyebrow">Free quick scan</span><h1>Your settings check-up</h1>
        <p class="lead">We looked through Windows and OBS for the settings mix-ups that trip up most streamers.</p>
        <div id="miclinebox"></div></div>
        <button class="btn sm" id="rescan">Scan again</button>
      </div>
      <div id="choice-slot"></div>
      <div id="body"></div>
    </section>`));
  const body = root.querySelector('#body');
  root.querySelector('#rescan').addEventListener('click', () => run(true));
  const micSlot = root.querySelector('#choice-slot');
  function drawMicLine() {
    const box = root.querySelector('#miclinebox');
    box.innerHTML = '';
    box.append(yourMicLine(mic, () => openMicChoice(micSlot, mic, { onDone: drawMicLine })));
  }
  drawMicLine();

  async function run(force) {
    body.innerHTML = '';
    if (!state.quick || force) {
      body.append(loading('Checking Windows sound settings, OBS filters and your stream settings…'));
      try {
        state.quick = await api.runQuickChecks();
        state.applied = null;
      } catch (e) {
        body.innerHTML = '';
        body.append(errorBanner(e, () => run(true)));
        return;
      }
      refreshNav();
    }
    try {
      draw();
    } catch (e) {
      // Unexpected data from the backend: say so plainly, log it, offer a retry.
      api.logUi('error', 'scan.draw', e && e.stack ? e.stack : e);
      body.innerHTML = '';
      body.append(errorBanner(new Error("We got the scan results but couldn't show them."), () => { state.quick = null; run(true); }));
    }
  }

  function draw() {
    body.innerHTML = '';
    const q = state.quick;
    const fixed = new Set(state.applied ? q.actions.filter((a) => state.applied.applied.includes(a.id)).flatMap((a) => a.resolves) : []);
    const open = q.findings.filter((f) => !fixed.has(f.id) && f.severity !== 'info');
    const pendingActions = q.actions.filter((a) => !state.applied?.applied.includes(a.id));
    const autoOpen = open.filter((f) => pendingActions.some((a) => a.resolves.includes(f.id))).length;
    const total = q.score.length ? Math.round(q.score.reduce((s, x) => s + x.value, 0) / q.score.length) : 100;

    const summary = html(`
      <div class="card hero">
        <div class="row" style="gap:28px;align-items:center">
          <div class="ring"></div>
          <div class="stack" style="gap:8px;flex:1;min-width:240px">
            <h2>${open.length ? `We found ${open.length} thing${open.length === 1 ? '' : 's'} to fix` : 'Everything looks good'}</h2>
            <p class="muted">${open.length ? (autoOpen ? `${autoOpen} of them we can fix for you right now${autoOpen < open.length ? '. The rest take a quick tweak by you.' : '.'}` : 'Each takes a quick tweak by you — the steps are below.') : 'Your Windows and OBS settings are in great shape.'}</p>
            <div class="row areas" style="gap:14px"></div>
          </div>
        </div>
      </div>`);
    summary.querySelector('.ring').append(scoreRing(total, { label: 'settings' }));
    const areas = summary.querySelector('.areas');
    for (const s of q.score) {
      const a = html(`<div class="row" style="gap:8px"></div>`);
      a.append(scoreRing(s.value, { size: 'sm', label: '' }));
      a.append(html(`<div class="stack" style="gap:0"><b>${esc(AREA_LABEL[s.area] || s.area)}</b><span class="subtle">${s.issues} to fix</span></div>`));
      areas.append(a);
    }
    body.append(summary);

    if (state.applied) {
      const r = state.applied;
      const labelOf = (id) => q.actions.find((a) => a.id === id)?.label || 'That fix';
      const n = r.applied.length;
      const guided = guidedFailures(r);
      const fails = plainFailures(r).map((f) => `<li><b>${esc(labelOf(f.id))}</b> — we couldn't do this automatically. ${esc(f.error)}</li>`).join('');
      const head = n && !r.failed.length ? `Done — fixed ${n} thing${n === 1 ? '' : 's'} for you.`
        : n ? `Fixed ${n} thing${n === 1 ? '' : 's'}. The rest need a hand from you.`
          : guided.length === r.failed.length ? 'Windows wants you to switch this one yourself.'
            : "We couldn't change anything automatically.";
      body.append(html(`<div class="banner ${r.failed.length ? 'warn' : 'good'}" style="margin-top:16px"><div class="stack" style="gap:4px"><b>${esc(head)}</b>
        ${fails ? `<ul class="plain">${fails}</ul><span class="subtle">They stay on your list below. Use "Open Sound settings" where it's offered, or press the fix button to try again.</span>` : ''}</div></div>`));
      // Windows said no, but we can walk them through it: one big button each.
      for (const f of guided) body.append(guidedFixCard(f, { from: 'scan', go, label: labelOf(f.id) }));
      if (r.restart_obs_needed && n) body.append(html('<div class="banner warn" style="margin-top:12px"><span><b>Restart OBS</b> to finish applying the changes. We\'ll reconnect automatically.</span></div>'));
    }

    const selected = new Set(q.actions.filter((a) => !state.applied?.applied.includes(a.id)).map((a) => a.id));
    const list = html('<div class="findings" style="margin-top:20px"></div>');
    const ranked = [...q.findings].sort((a, b) => Number(fixed.has(a.id)) - Number(fixed.has(b.id)));
    for (const f of ranked) {
      const acts = q.actions.filter((a) => a.resolves.includes(f.id));
      const isFixed = fixed.has(f.id);
      list.append(findingCard(f, {
        fixed: isFixed,
        selectable: acts.length > 0 && !isFixed,
        checked: acts.every((a) => selected.has(a.id)),
        onToggle: (on) => acts.forEach((a) => (on ? selected.add(a.id) : selected.delete(a.id))) || updateBtn(),
      }));
      // Steps the user does in Windows: give them a one-click way to the right page.
      const help = !acts.length && !isFixed && f.severity !== 'info' && SOUND_SETTINGS_FOR.has(f.id);
      if (help) {
        const card = list.lastElementChild;
        const b = html('<button class="btn sm primary">Open Sound settings</button>');
        b.addEventListener('click', async () => {
          try {
            await api.openForUser({ kind: 'windows_settings', uri_or_app: 'ms-settings:sound', label: 'Sound settings', steps: [] });
          } catch (e) {
            toast("Couldn't open Sound settings", 'Open the Windows Start menu, type "Sound settings" and press Enter.', 'bad');
          }
        });
        const row = html('<div class="actions"></div>');
        row.append(b);
        card.querySelector('.body').append(row);
      }
    }

    const bar = html(`<div class="card stack" style="margin-top:20px">
      <div class="spread"><div class="stack" style="gap:2px"><b>Let us handle the settings</b><span class="subtle">Nothing physical — just switches in Windows and OBS. You can undo any of it.</span></div>
      <div class="row"><button class="btn primary lg" id="fix"></button></div></div></div>`);
    const fixBtn = bar.querySelector('#fix');
    function updateBtn() {
      fixBtn.textContent = selected.size ? `Fix ${selected.size} thing${selected.size === 1 ? '' : 's'} for me` : 'Nothing selected';
      fixBtn.disabled = !selected.size;
    }
    updateBtn();
    fixBtn.addEventListener('click', async () => {
      fixBtn.disabled = true;
      fixBtn.innerHTML = '<span class="spinner" style="width:18px;height:18px;border-width:2px"></span> Fixing…';
      try {
        const res = await api.applyActions([...selected]);
        state.applied = res;
        // Exactly one toast, and never a success toast when nothing was fixed.
        if (res.applied.length && !res.failed.length) toast('All done', 'Your settings are updated. You can undo any of it from Backups.', 'good');
        else if (res.applied.length) toast('Partly done', 'Some fixes need a hand from you. See the list.', '');
        else toast("Nothing was changed", "We couldn't do these automatically. The list shows what to do instead.", 'bad');
        draw();
      } catch (e) {
        toast("That didn't work", `${e.message} Nothing was changed. Press the button to try again.`, 'bad');
        updateBtn();
      }
    });
    if (q.actions.length && !(state.applied && selected.size === 0)) body.append(bar);
    body.append(list);

    const next = html(`<div class="card hero stack" style="margin-top:20px"><h2>Next: listen to your mic</h2>
      <p class="muted">Settings are only half of it. A 45-second guided test tells us how your mic actually sounds — and what to change.</p>
      <div class="row"><button class="btn primary lg">Start the mic test</button></div></div>`);
    next.querySelector('button').addEventListener('click', () => go('mic'));
    body.append(next);
    body.append(html('<div style="height:12px"></div>'));
    body.append(advanced('Advanced: what each fix changes', html(`<pre class="json">${esc(JSON.stringify(q.actions.map((a) => ({ label: a.label, ...a.action })), null, 2))}</pre>`)));
  }

  run(false);
}
