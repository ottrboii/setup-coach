// 10. A/B: first test vs. your new sound (the same reading, run through your new settings offline).
import { api } from '../api.js';
import { esc, html, icons, scoreRing, toast, loading, errorBanner, advanced, kvTable, fmt } from '../components.js';
import { state, MIC_CONSTRAINT } from '../state.js';
import { abErrorWords } from '../backupcopy.js';

export async function render(root, { go }) {
  const page = html('<section class="stack-lg"></section>');
  root.append(page);
  page.append(html(`<div class="stack" style="gap:6px"><span class="eyebrow">Before &amp; after</span><h1>Your old sound next to your new sound</h1>
    <p class="lead">Both clips are the same reading, played at the same volume, so the only thing you hear is the difference.</p></div>`));
  const body = html('<div class="stack-lg"></div>');
  page.append(body);
  let playing = false;

  const micId = state.micId;
  if (!micId) {
    body.append(html('<div class="banner warn"><span>Pick and test your mic first, then come back here.</span></div>'));
    const b = html('<button class="btn primary">Start the mic test</button>');
    b.addEventListener('click', () => go('mic'));
    body.append(b);
    return;
  }
  await load();

  async function load() {
    body.innerHTML = '';
    body.append(loading('Getting your recordings…'));
    let info;
    try {
      info = await api.getAb(micId);
    } catch (e) {
      body.innerHTML = '';
      const w = abErrorWords(e);
      body.append(html(`<div class="banner info"><div class="stack" style="gap:4px"><b>${esc(w.title)}</b><span>${esc(w.text)}</span></div></div>`));
      const b = html(`<button class="btn primary">${esc(w.actionLabel || 'Test my mic')}</button>`);
      b.addEventListener('click', () => go(w.action || 'mic'));
      body.append(b);
      return;
    }
    body.innerHTML = '';
    if (!info.final) return draw(info, false);
    draw(info, true);
  }

  function draw(info, hasFinal) {
    if (!hasFinal) {
      body.append(html(`<div class="card stack hero"><h2>One more short reading</h2>
        <p class="muted">Apply your new settings, then read the same passage again for about ten seconds. We run it through your new settings to show how you should sound. It is not a live recording from OBS.</p>
        <div class="row"><button class="btn primary lg" id="rec">Hear my new sound</button></div></div>`));
      body.querySelector('#rec').addEventListener('click', () => go(state.micVerify ? 'ab-record' : 'mic-apply'));
      if (!state.micVerify) body.append(html('<p class="subtle">First we need to apply your settings, so the button takes you there.</p>'));
      return;
    }
    const louder = info.matched_gain_db > 0.5 ? 'new' : info.matched_gain_db < -0.5 ? 'old' : null;
    const fair = html(`<div class="banner info"><span>${louder === 'new' ? 'Your new sound was louder, so we turned it down a little to keep it fair.' : louder === 'old' ? 'Your first test was louder, so we turned it down a little to keep it fair.' : 'Both clips already had the same volume.'}</span></div>`);
    body.append(fair);
    if (info.note) body.append(html(`<div class="banner warn"><span>${esc(info.note)}</span></div>`));

    const scores = html(`<div class="card"><div class="row" style="gap:28px;justify-content:center">
      <div class="stack" style="align-items:center;gap:6px"><div class="b"></div><span class="subtle">First test</span></div>
      <div style="font-size:28px;color:var(--text-3)" aria-hidden="true">→</div>
      <div class="stack" style="align-items:center;gap:6px"><div class="a"></div><span class="subtle">New sound</span></div></div></div>`);
    scores.querySelector('.b').append(scoreRing(info.original.score, { label: 'before' }));
    scores.querySelector('.a').append(scoreRing(info.final.score, { label: 'after' }));
    body.append(scores);

    const player = html(`<div class="stack">
      <div class="ab">
        <button class="play before" data-which="original"><span class="circle">${icons.play}</span><b>First test</b><span class="subtle">Your mic as it was</span></button>
        <button class="play after" data-which="final"><span class="circle">${icons.play}</span><b>New sound</b><span class="subtle">How you should sound with your new settings</span></button>
      </div>
      <label class="toggle"><input type="checkbox" id="phone"/> <span>How it sounds on a phone speaker</span></label></div>`);
    const phone = player.querySelector('#phone');
    player.querySelectorAll('.play').forEach((b) => b.addEventListener('click', () => toggle(b, player, phone.checked)));
    phone.addEventListener('change', () => {
      const cur = player.querySelector('.play.playing');
      if (cur) toggle(cur, player, phone.checked, true);
    });
    body.append(player);

    const actions = html('<div class="row"><button class="btn" id="exp">Save both clips to my Desktop</button><button class="btn ghost" id="again">Read it again</button></div>');
    actions.querySelector('#exp').addEventListener('click', async (ev) => {
      ev.currentTarget.disabled = true;
      try {
        const p = await api.exportAb(micId);
        toast('Saved to your Desktop', p, 'good');
      } catch (e) { const w = abErrorWords(e); toast(w.title, w.text, 'bad'); }
      ev.currentTarget.disabled = false;
    });
    actions.querySelector('#again').addEventListener('click', () => go('ab-record'));
    body.append(actions);

    // Anything left to fix? "I can't do this" is always allowed and re-tunes around it.
    // From the final take itself (get_ab.findings), not from the earlier verify step.
    const open = (info.findings || []).filter((f) => f.severity !== 'info' && f.fix_kind !== 'auto');
    if (open.length && state.micContext) {
      const box = html('<div class="card stack"><h3>Still not quite right?</h3><p class="muted">If you can do one of these, it helps most. If you can\'t, tell us and we will tune your settings around it.</p><div class="findings"></div></div>');
      const list = box.querySelector('.findings');
      for (const f of open) {
        const row = html(`<div class="spread" style="gap:12px;align-items:flex-start"><div class="stack" style="gap:2px;flex:1;min-width:200px"><b>${esc(f.title)}</b><span class="muted">${esc(f.fix)}</span></div>
          <button class="btn sm">I can't do this</button></div>`);
        row.querySelector('button').addEventListener('click', (ev) => cant(f, ev.currentTarget));
        list.append(row);
      }
      body.append(box);
    }

    const m = html('<div></div>');
    m.append(kvTable([
      ['First test loudness', fmt(info.original.lufs, 1, ' LUFS')],
      ['New sound loudness', fmt(info.final.lufs, 1, ' LUFS')],
      ['Volume match applied', fmt(info.matched_gain_db, 1, ' dB')],
      ['First test file', info.original.path],
      ['New sound file', info.final.path],
    ]));
    body.append(advanced('Advanced: the numbers', m));
  }

  async function cant(f, btn) {
    const c = MIC_CONSTRAINT[f.id];
    btn.disabled = true;
    if (!c) {
      btn.closest('.spread').replaceWith(html(`<div class="banner info"><span>No problem. We'll leave "${esc(f.title)}" as it is.</span></div>`));
      return;
    }
    if (!state.micConstraints.includes(c)) state.micConstraints.push(c);
    btn.innerHTML = '<span class="spinner tiny"></span> Re-tuning…';
    try {
      state.micReport = await api.rescanMic(state.micContext, state.micConstraints);
      state.micVerify = null;
      toast('Got it', "We've re-tuned your settings around that.", 'good');
      go('mic-apply');
    } catch (e) {
      toast("Couldn't re-tune", e.message, 'bad');
      btn.disabled = false;
      btn.textContent = "I can't do this";
    }
  }

  async function toggle(btn, ab, phone, keep = false) {
    const which = btn.dataset.which;
    const was = btn.classList.contains('playing');
    ab.querySelectorAll('.play').forEach((b) => { b.classList.remove('playing'); b.querySelector('.circle').innerHTML = icons.play; });
    try {
      await api.stopPreview();
      playing = false;
      if (was && !keep) return;
      await api.playPreview(which, phone);
      playing = true;
      btn.classList.add('playing');
      btn.querySelector('.circle').innerHTML = icons.stop;
    } catch (e) {
      toast("Couldn't play it", e.message, 'bad');
    }
  }

  return () => { if (playing) api.stopPreview().catch(() => {}); };
}
