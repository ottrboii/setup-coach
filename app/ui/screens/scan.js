// 2. Quick checks — the free scan of Windows + OBS settings.
import { api } from '../api.js';
import { esc, html, scoreRing, findingCard, loading, errorBanner, toast, advanced } from '../components.js';
import { state } from '../state.js';

const AREA_LABEL = { mic: 'Mic settings', system: 'Windows', camera: 'Camera', stream: 'Stream' };

export async function render(root, { go, refreshNav }) {
  root.append(html(`
    <section class="stack-lg">
      <div class="spread">
        <div class="stack" style="gap:6px"><span class="eyebrow">Free quick scan</span><h1>Your settings check-up</h1>
        <p class="lead">We looked through Windows and OBS for the settings mix-ups that trip up most streamers.</p></div>
        <button class="btn sm" id="rescan">Scan again</button>
      </div>
      <div id="body"></div>
    </section>`));
  const body = root.querySelector('#body');
  root.querySelector('#rescan').addEventListener('click', () => run(true));

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
    draw();
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
      body.append(html(`<div class="banner good" style="margin-top:16px"><div class="stack" style="gap:4px"><b>Done — fixed ${r.applied.length} thing${r.applied.length === 1 ? '' : 's'} for you.</b>
        ${r.failed.map((f) => `<span>• ${esc(f.error)}</span>`).join('')}</div></div>`));
      if (r.restart_obs_needed) body.append(html('<div class="banner warn" style="margin-top:12px"><span><b>Restart OBS</b> to finish applying the changes. We\'ll reconnect automatically.</span></div>'));
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
        state.applied = await api.applyActions([...selected]);
        toast('Settings updated', `${state.applied.applied.length} fixed`, 'good');
        draw();
      } catch (e) {
        toast("That didn't work", e.message, 'bad');
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
