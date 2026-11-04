// 8. Home dashboard: scores, pre-stream re-check, save / re-apply setup.
import { api } from '../api.js';
import { esc, html, scoreRing, findingCard, toast, loading } from '../components.js';
import { state } from '../state.js';

export async function render(root, { go, refreshNav }) {
  const sys = state.quick?.score?.find((s) => s.area === 'system');
  const tiles = [
    { label: 'Mic', score: state.micVerify?.after?.score?.value ?? state.micReport?.score?.value, route: 'mic', cta: 'Test my mic' },
    { label: 'Camera', score: state.cameraReport?.score?.value, route: 'camera', cta: 'Check my camera' },
    { label: 'Stream', score: state.healthReport?.score?.value, route: 'health', cta: 'Watch a stream' },
    { label: 'Settings', score: sys?.value, route: 'scan', cta: 'Run quick scan' },
  ];
  const page = html(`
    <section class="stack-lg">
      <div class="spread"><div class="stack" style="gap:6px"><span class="eyebrow">Home</span><h1>Your setup</h1>
      <p class="lead">Everything in one place. Run a quick re-check before you go live — it takes a few seconds.</p></div>
      <button class="btn primary lg" id="recheck">Re-check before stream</button></div>
      <div class="grid-4" id="tiles"></div>
      <div id="recheck-box"></div>
      <div class="grid-2">
        <div class="card stack"><h3>Save this setup</h3><p class="muted">We'll remember your mic and camera settings and put them back automatically when Windows or a replug resets them.</p>
          <div class="row"><input id="name" class="btn" style="flex:1;min-width:160px;text-align:left;cursor:text" value="${esc(state.saved?.name || 'My stream setup')}" aria-label="Setup name"/><button class="btn primary" id="save">Save</button></div>
          <span class="subtle" id="saved">${state.saved || state.env?.saved_profile ? `Last saved: ${esc(new Date((state.saved || state.env.saved_profile).saved_at).toLocaleString())}` : 'Not saved yet.'}</span></div>
        <div class="card stack"><h3>Something sounds off again?</h3><p class="muted">Windows updates and other apps sometimes undo settings. Put your saved setup back in one click.</p>
          <div class="row"><button class="btn" id="reapply">Re-apply my saved setup</button><button class="btn ghost" id="backups">Undo changes</button></div></div>
      </div>
    </section>`);
  root.append(page);

  const grid = page.querySelector('#tiles');
  for (const t of tiles) {
    const card = html(`<div class="card stack" style="align-items:center;text-align:center;gap:10px"><b>${esc(t.label)}</b><div class="r"></div><button class="btn sm">${t.score == null ? esc(t.cta) : 'Open'}</button></div>`);
    if (t.score == null) card.querySelector('.r').append(html('<div class="score-ring sm" style="display:grid;place-items:center;border-radius:50%;border:6px solid var(--surface-3)"><span class="subtle">—</span></div>'));
    else card.querySelector('.r').append(scoreRing(t.score, { size: 'sm', label: '' }));
    card.querySelector('button').addEventListener('click', () => go(t.route));
    grid.append(card);
  }

  page.querySelector('#recheck').addEventListener('click', async () => {
    const box = page.querySelector('#recheck-box');
    box.innerHTML = '';
    box.append(loading('Re-checking Windows and OBS…'));
    try {
      state.quick = await api.runQuickChecks();
      state.applied = null;
      refreshNav();
      const open = state.quick.findings.filter((f) => f.severity !== 'info');
      box.innerHTML = '';
      if (!open.length) {
        box.append(html('<div class="banner good"><b>All good — you\'re ready to go live.</b></div>'));
      } else {
        box.append(html(`<div class="banner warn"><div class="stack" style="gap:4px"><b>${open.length} thing${open.length === 1 ? '' : 's'} changed since last time.</b><span>Fix them on the quick scan page.</span></div></div>`));
        const list = html('<div class="findings" style="margin-top:12px"></div>');
        open.slice(0, 3).forEach((f) => list.append(findingCard(f, { visual: false })));
        box.append(list);
        const b = html('<button class="btn primary" style="margin-top:12px">Go to quick scan</button>');
        b.addEventListener('click', () => go('scan'));
        box.append(b);
      }
    } catch (e) {
      box.innerHTML = '';
      toast("Couldn't re-check", e.message, 'bad');
    }
  });

  page.querySelector('#save').addEventListener('click', async () => {
    try {
      state.saved = await api.saveSetup(page.querySelector('#name').value.trim() || 'My stream setup');
      page.querySelector('#saved').textContent = `Last saved: ${new Date(state.saved.saved_at).toLocaleString()}`;
      toast('Setup saved', "We'll re-apply it automatically when something resets it.", 'good');
    } catch (e) {
      toast("Couldn't save", e.message, 'bad');
    }
  });

  page.querySelector('#backups').addEventListener('click', () => go('backups'));
  page.querySelector('#reapply').addEventListener('click', async (ev) => {
    const b = ev.currentTarget;
    b.disabled = true;
    try {
      const r = await api.reapplySavedSetup();
      if (r.failed?.length) toast('Some settings need a hand', r.failed.map((f) => f.error).join(' '), 'warn');
      else toast('Setup re-applied', `${r.applied.length} part${r.applied.length === 1 ? '' : 's'} restored.`, 'good');
    } catch (e) {
      toast("Couldn't re-apply", e.message, 'bad');
    } finally {
      b.disabled = false;
    }
  });
}
