// 2. Quick checks — the free scan of Windows + OBS settings.
import { api } from '../api.js';
import { esc, html, scoreRing, findingCard, loading, errorBanner, toast, advanced } from '../components.js';
import { state, selectedMic } from '../state.js';
import { openMicChoice, yourMicLine } from '../micpicker.js';
import { guidedFixCard, runtimeWarningBanners } from '../guided.js';

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
        state.quick = await api.runQuickChecks(state.micId);
        state.applied = null;
        state.fixReport = null;
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
    // `q` is always the latest scan (after a fix it is the re-scan the backend took to verify it),
    // so nothing here is remembered from an earlier round: what is listed is what is true now.
    const open = q.findings.filter((f) => f.severity !== 'info');
    const autoOpen = open.filter((f) => q.actions.some((a) => a.resolves.includes(f.id))).length;
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
    for (const b of runtimeWarningBanners(q)) body.append(b);

    if (state.fixReport) {
      const r = state.fixReport;
      const by = (s) => r.items.filter((i) => i.status === s);
      const fixedN = by('fixed').length;
      const needN = by('needs_you').length;
      const badN = by('failed').length + by('not_confirmed').length;
      const head = !r.items.length ? 'Nothing was selected.'
        : fixedN === r.items.length ? `Done — fixed ${fixedN} thing${fixedN === 1 ? '' : 's'}, and the re-check agrees.`
          : fixedN ? `Fixed ${fixedN} of ${r.items.length}. The rest are listed below with the reason.`
            : "Nothing could be fixed automatically. Each reason is below.";
      const chip = { fixed: ['good', 'Fixed'], needs_you: ['warn', 'Needs you'], failed: ['bad', "Couldn't fix"], not_confirmed: ['bad', "Couldn't confirm"] };
      const rows = r.items.map((i) => {
        const [tone, text] = chip[i.status] || ['bad', 'Not done'];
        return `<li class="fix-row"><span class="fix-chip ${tone}">${esc(text)}</span> <b>${esc(i.label)}</b><br><span class="subtle">${esc(i.message)}</span></li>`;
      }).join('');
      const tone = badN || needN ? 'warn' : 'good';
      body.append(html(`<div class="banner ${tone}" role="status" style="margin-top:16px"><div class="stack" style="gap:6px"><b>${esc(head)}</b>
        <ul class="plain fix-list">${rows}</ul>
        <span class="subtle">${fixedN ? 'We scanned again after the changes, so this list is what Windows and OBS show now. You can undo any of it from Backups. ' : ''}${badN || needN ? 'Anything not fixed stays in the list below.' : ''}</span></div></div>`));
      // Windows said no, but we can walk them through it: one big button each.
      for (const i of by('needs_you')) body.append(guidedFixCard({ error: i.message, guided: i.guided }, { from: 'scan', go, label: i.label }));
      if (r.restart_obs_needed && fixedN) body.append(html('<div class="banner warn" style="margin-top:12px"><span><b>Restart OBS</b> to finish applying the changes. We\'ll reconnect automatically.</span></div>'));
    }

    const selected = new Set(q.actions.map((a) => a.id));
    const list = html('<div class="findings" style="margin-top:20px"></div>');
    for (const f of q.findings) {
      const acts = q.actions.filter((a) => a.resolves.includes(f.id));
      list.append(findingCard(f, {
        fixed: false,
        selectable: acts.length > 0,
        checked: acts.every((a) => selected.has(a.id)),
        onToggle: (on) => acts.forEach((a) => (on ? selected.add(a.id) : selected.delete(a.id))) || updateBtn(),
      }));
      // Steps the user does in Windows: give them a one-click way to the right page.
      const help = !acts.length && f.severity !== 'info' && SOUND_SETTINGS_FOR.has(f.id);
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
      <div class="spread"><div class="stack" style="gap:2px"><b>Let us handle the settings</b><span class="subtle">Nothing physical — just switches in Windows and OBS. We scan again afterwards to check, and you can undo any of it.</span></div>
      <div class="row"><button class="btn primary lg" id="fix"></button></div></div>
      <div id="fix-live" class="stack" style="gap:6px"></div></div>`);
    const fixBtn = bar.querySelector('#fix');
    function updateBtn() {
      fixBtn.textContent = selected.size ? `Fix ${selected.size} thing${selected.size === 1 ? '' : 's'} for me` : 'Nothing selected';
      fixBtn.disabled = !selected.size;
    }
    updateBtn();
    fixBtn.addEventListener('click', async () => {
      const ids = [...selected];
      const todo = q.actions.filter((a) => ids.includes(a.id));
      fixBtn.disabled = true;
      fixBtn.innerHTML = '<span class="spinner" style="width:18px;height:18px;border-width:2px"></span> Fixing…';
      // Live status while the backend works: every item, what it is doing, and a heads-up for Windows' permission prompt.
      const live = bar.querySelector('#fix-live');
      live.innerHTML = '';
      live.append(html(`<ul class="plain fix-list">${todo.map((a) => `<li class="fix-row"><span class="fix-chip"><span class="spinner" style="width:12px;height:12px;border-width:2px"></span> Working…</span> <b>${esc(a.label)}</b></li>`).join('')}</ul>`));
      if (todo.some((a) => a.needs_admin)) live.append(html('<span class="subtle">Windows may ask for permission — choose Yes. It only changes the one setting.</span>'));
      live.append(html('<span class="subtle">Then we scan again to check each one actually changed.</span>'));
      try {
        const rep = await api.fixAndVerify(ids, state.micId);
        // The re-scan IS the new truth: replace the old scan, keep the per-item outcomes on screen.
        state.quick = rep.quick;
        // Earlier rounds stay on screen, and are re-judged by this fresh scan: if one came back, say so.
        const earlier = (state.fixReport?.items || []).filter((i) => i.status === 'fixed').map((i) => {
          const back = rep.quick.findings.find((f) => i.resolves.includes(f.id) && f.severity !== 'info');
          return back ? { ...i, status: 'not_confirmed', message: `It came back: the re-check shows "${back.title}" again.` }
            : { ...i, message: 'Fixed earlier; the re-check still shows it gone.' };
        });
        state.fixReport = { ...rep, items: [...rep.items, ...earlier] };
        state.applied = null;
        refreshNav();
        const f = rep.items.filter((i) => i.status === 'fixed').length;
        // Exactly one toast, and never a success toast when nothing was fixed.
        if (f && f === rep.items.length) toast('All done', 'Fixed, and the re-check agrees. You can undo any of it from Backups.', 'good');
        else if (f) toast('Partly done', `${f} of ${rep.items.length} fixed. The list says why for the rest.`, '');
        else toast("Nothing was changed", 'Each item says why, and what to do instead.', 'bad');
        draw();
      } catch (e) {
        live.innerHTML = '';
        toast("That didn't work", `${e.message || e} Press the button to try again, or scan again to see where things stand.`, 'bad');
        updateBtn();
      }
    });
    if (q.actions.length) body.append(bar);
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
