// 9. Backups: everything Setup Coach changed was saved first. Put it back, plain and safe.
import { api } from '../api.js';
import { esc, html, icons, toast, loading, errorBanner } from '../components.js';

function when(iso) {
  const t = new Date(iso).getTime();
  if (!Number.isFinite(t)) return '';
  const mins = Math.max(0, Math.round((Date.now() - t) / 60000));
  if (mins < 2) return 'Just now';
  if (mins < 60) return `${mins} minutes ago`;
  const h = Math.round(mins / 60);
  if (h < 24) return `${h} hour${h === 1 ? '' : 's'} ago`;
  return new Date(t).toLocaleString([], { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
}

export async function render(root, { go }) {
  const page = html('<section class="stack-lg"></section>');
  root.append(page);
  page.append(html(`<div class="stack" style="gap:6px"><span class="eyebrow">Backups &amp; undo</span><h1>Nothing is permanent</h1>
    <p class="lead">Before Setup Coach changes anything, it saves how things were. Pick a moment below to go back to it. Your voice recordings are never touched.</p></div>`));
  const body = html('<div class="stack-lg" id="body"></div>');
  page.append(body);
  const ab = html(`<div class="card stack"><h3>Hear the difference</h3>
    <p class="muted">Compare your first test with your new sound, side by side, at the same volume.</p>
    <div class="row"><button class="btn primary" id="ab">Compare before and after</button></div></div>`);
  ab.querySelector('#ab').addEventListener('click', () => go('ab'));
  page.append(ab);

  let openId = null;
  await load();

  async function load() {
    body.innerHTML = '';
    body.append(loading('Looking for your backups…'));
    let list;
    try {
      list = await api.listBackups();
    } catch (e) {
      body.innerHTML = '';
      body.append(errorBanner(e, load));
      return;
    }
    body.innerHTML = '';
    if (!list.length) {
      body.append(html(`<div class="banner info"><div class="stack" style="gap:4px"><b>No backups yet</b>
        <span>That means Setup Coach hasn't changed anything on this PC. When it does, the old settings are saved here first.</span></div></div>`));
      return;
    }
    const orig = list.find((b) => b.id === 'original');
    if (orig) body.append(card(orig, true));
    const rest = list.filter((b) => b.id !== 'original');
    if (rest.length) {
      body.append(html('<h2>Step by step</h2>'));
      const wrap = html('<div class="findings"></div>');
      rest.forEach((b) => wrap.append(card(b, false)));
      body.append(wrap);
    }
  }

  function card(b, isOriginal) {
    const el = html(`<div class="card stack ${isOriginal ? 'hero' : ''}" data-id="${esc(b.id)}">
      <div class="spread" style="align-items:flex-start;gap:12px">
        <div class="stack" style="gap:4px;flex:1;min-width:200px"><b>${esc(b.label)}</b>
          <span class="subtle">${isOriginal ? 'Saved the first time Setup Coach changed anything' : esc(when(b.created_at))}</span></div>
        <button class="btn ${isOriginal ? 'primary' : ''}" data-act="ask">${isOriginal ? 'Undo everything' : 'Go back to this'}</button>
      </div>
      <ul class="explain" style="margin:0;padding:0">${b.items.map((i) => `<li><span class="ico">${icons.check}</span><span>${esc(i)}</span></li>`).join('')}</ul>
      <div class="slot"></div></div>`);
    el.querySelector('[data-act="ask"]').addEventListener('click', () => confirm(el, b, isOriginal));
    if (openId === b.id) confirm(el, b, isOriginal);
    return el;
  }

  function confirm(el, b, isOriginal) {
    openId = b.id;
    const slot = el.querySelector('.slot');
    slot.innerHTML = '';
    const box = html(`<div class="banner warn"><div class="stack" style="gap:10px">
      <b>${isOriginal ? 'Put everything back how it was before Setup Coach?' : 'Go back to how things were at this moment?'}</b>
      <span>The settings above are put back. Anything you changed since then in those places will be replaced. We save your current settings first, so you can undo this too.</span>
      <div class="row"><button class="btn primary" data-go>Yes, put it back</button><button class="btn ghost" data-no>Keep things as they are</button></div></div></div>`);
    box.querySelector('[data-no]').addEventListener('click', () => { openId = null; slot.innerHTML = ''; });
    box.querySelector('[data-go]').addEventListener('click', (ev) => restore(el, b, ev.currentTarget));
    slot.append(box);
  }

  async function restore(el, b, btn) {
    const slot = el.querySelector('.slot');
    btn.disabled = true;
    btn.innerHTML = '<span class="spinner tiny"></span> Putting it back…';
    try {
      const r = await api.restoreBackup(b.id);
      showResult(slot, b, r);
    } catch (e) {
      slot.innerHTML = '';
      slot.append(errorBanner(e, () => restore(el, b, btn)));
    }
  }

  function showResult(slot, b, r) {
    slot.innerHTML = '';
    const needObs = (r.failed || []).filter((f) => /^obs_(file|websocket)/.test(f.id));
    const other = (r.failed || []).filter((f) => !needObs.includes(f));
    const parts = [];
    if (r.applied?.length || (!needObs.length && !other.length)) {
      parts.push(`<div class="banner good"><span class="ico">${icons.check}</span><div class="stack" style="gap:4px"><b>${needObs.length ? 'Most of it is back' : "It's back how it was"}</b>
        ${r.restart_obs_needed ? '<span><b>Restart OBS</b> so it picks up its restored settings.</span>' : ''}</div></div>`);
    }
    if (needObs.length) {
      parts.push(`<div class="banner warn"><div class="stack" style="gap:10px"><b>One more step: close OBS</b>
        <span>OBS keeps a few of its settings in files that can only be put back while it is closed. Close OBS, then press the button.</span>
        <div class="row"><button class="btn primary" data-again>OBS is closed, finish</button><button class="btn ghost" data-cant>I can't close OBS right now</button></div></div></div>`);
    }
    if (other.length) {
      parts.push(`<div class="banner bad"><div class="stack" style="gap:4px"><b>These didn't go back</b>${other.map((f) => `<span>${esc(f.error)}</span>`).join('')}</div></div>`);
    }
    if (r.skipped?.length) {
      parts.push(`<div class="banner info"><div class="stack" style="gap:4px"><b>Left as they are</b>${r.skipped.map((s) => `<span>${esc(s.error)}</span>`).join('')}</div></div>`);
    }
    const box = html(`<div class="stack">${parts.join('')}</div>`);
    const again = box.querySelector('[data-again]');
    if (again) again.addEventListener('click', async () => {
      again.disabled = true;
      again.innerHTML = '<span class="spinner tiny"></span> Finishing…';
      try { showResult(slot, b, await api.restoreBackup(b.id)); } catch (e) { toast("Couldn't finish", e.message, 'bad'); again.disabled = false; again.textContent = 'OBS is closed, finish'; }
    });
    const cant = box.querySelector('[data-cant]');
    if (cant) cant.addEventListener('click', () => {
      cant.closest('.banner').replaceWith(html(`<div class="banner info"><div class="stack" style="gap:4px"><b>No problem</b>
        <span>OBS keeps its current file settings for now. Everything else is already back. Come back to this page any time and press the button again.</span></div></div>`));
    });
    slot.append(box);
  }
}
