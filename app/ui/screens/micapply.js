// 5. Apply the chain, then verify with a before/after comparison.
import { api } from '../api.js';
import { isTuneRelevant } from '../profilenav.js';
import { esc, html, icons, scoreRing, toast, errorBanner, advanced } from '../components.js';
import { state } from '../state.js';
import { guidedFailures, guidedFixCard } from '../guided.js';
import { isRouteHidden } from '../profilenav.js';

/** Plain-language description of every stage in a ProcessingChain. */
export function explainChain(c) {
  const items = [];
  if (c.mono_downmix) items.push(['Your voice in both ears', 'Your interface sends your mic on one side only — we combine it so everyone hears you in both ears.']);
  items.push(['A low-cut filter', `Removes rumble, desk thumps and air-conditioner hum below your voice (under ${Math.round(c.highpass_hz)} Hz).`]);
  for (const b of c.eq || []) {
    const r = b.reason || (b.gain_db < 0 ? 'a gentle trim' : 'a gentle lift');
    items.push([`A gentle tone tweak: ${r}`, b.gain_db < 0 ? 'A small cut where your voice has a build-up.' : 'A small lift where your voice needs a bit more.']);
  }
  items.push(['Quiet gaps between sentences', 'Background noise gets turned down while you\'re not talking — gently, so your words never get cut.']);
  items.push(['Gentle leveling', 'Quiet and loud moments sit closer together, so viewers don\'t have to ride their volume.']);
  if (c.deesser) items.push(['Softer S sounds', 'Sharp "s" and "sh" sounds get smoothed only when they poke out.']);
  items.push(['A safety net', 'Your loudest laughs and shouts never distort.']);
  return items;
}

/** Hardware gain coaching from hardware_gain_change_db / input_gain_db. */
export function gainCoaching(c) {
  const d = c.hardware_gain_change_db || 0;
  if (c.input_gain_db > 0 && Math.abs(d) < 1.5) {
    return { kind: 'info', title: 'Leave your gain knob where it is', text: "It's already at (or near) max, so we add the extra level in software." };
  }
  if (Math.abs(d) < 1.5) return null;
  const amount = Math.abs(d) < 4 ? 'just a touch' : Math.abs(d) < 8 ? 'about a quarter turn' : 'about a third to half a turn';
  return d > 0
    ? { kind: 'warn', title: `Turn your gain knob up ${amount}`, text: 'Then talk normally and watch the meter: you want the green zone. Do this before you press "Apply".' }
    : { kind: 'warn', title: `Turn your gain knob down ${amount}`, text: 'Your loud moments are close to distorting. Then talk normally — the green zone is perfect.' };
}

export async function render(root, { go }) {
  const r = state.micReport;
  if (!r) {
    root.append(html('<div class="banner warn"><span>Run the mic test first.</span></div>'));
    const b = html('<button class="btn primary" style="margin-top:12px">Start the mic test</button>');
    b.addEventListener('click', () => go('mic'));
    root.append(b);
    return;
  }
  let playing = null;
  const page = html('<section class="stack-lg"></section>');
  root.append(page);
  if (state.micVerify) drawVerify();
  else drawPlan();

  function drawPlan() {
    const c = r.chain;
    page.innerHTML = '';
    page.append(html(`<div class="stack" style="gap:6px"><span class="eyebrow">Set up my mic</span><h1>Here's what we'll set</h1>
      <p class="lead">Plain, static settings — the same thing a sound engineer would dial in. No AI processing.</p></div>`));
    const g = gainCoaching(c);
    if (g) page.append(html(`<div class="banner ${g.kind}"><div class="stack" style="gap:4px"><b>${esc(g.title)}</b><span>${esc(g.text)}</span></div></div>`));
    const list = html(`<div class="card explain"><ul>${explainChain(c).map(([t, d]) => `<li><span class="ico">${icons.check}</span><div><b>${esc(t)}</b><div class="muted">${esc(d)}</div></div></li>`).join('')}</ul></div>`);
    page.append(list);
    if (c.notes?.length) page.append(html(`<div class="banner info"><div class="stack" style="gap:4px">${c.notes.map((n) => `<span>${esc(n)}</span>`).join('')}</div></div>`));
    const actions = html(`<div class="row"><button class="btn primary lg" id="apply">Apply to OBS</button><button class="btn lg" id="back">Back to report</button></div>`);
    actions.querySelector('#back').addEventListener('click', () => go('mic-report'));
    actions.querySelector('#apply').addEventListener('click', apply);
    page.append(actions);
    page.append(advanced('Advanced: exact settings', html(`<pre class="json">${esc(JSON.stringify(c, null, 2))}</pre>`)));
  }

  // Applying must never end in a dead screen: every failure says what happened,
  // keeps the button usable and is logged (api.call logs backend rejections).
  function showApplyError(btn, e, retry) {
    page.querySelector('.apply-error')?.remove();
    const box = html('<div class="apply-error"></div>');
    box.append(errorBanner(e, retry));
    page.append(box);
    btn.disabled = false;
    btn.textContent = 'Apply to OBS';
  }

  async function apply(ev) {
    const btn = ev.currentTarget;
    btn.disabled = true;
    page.querySelector('.apply-error')?.remove();
    btn.innerHTML = '<span class="spinner" style="width:18px;height:18px;border-width:2px"></span> Applying…';
    let res;
    try {
      res = await api.applyMicChain(r.chain, state.obsInput);
    } catch (e) {
      showApplyError(btn, e, () => btn.click());
      return;
    }
    const failed = res && res.failed ? res.failed : [];
    // Windows said no to something we can still walk them through.
    page.querySelectorAll('.guided-fix').forEach((n) => n.remove());
    for (const f of guidedFailures(res)) page.append(guidedFixCard(f, { from: 'mic-apply', go }));
    if (failed.length && !(res.applied && res.applied.length)) {
      // Nothing took: don't carry on to a before/after of something that wasn't applied.
      api.logUi('error', 'micapply.apply', failed.map((f) => f.error).join(' '));
      showApplyError(btn, new Error(failed.map((f) => f.error).join(' ') || "Your settings couldn't be applied."), () => btn.click());
      return;
    }
    if (failed.length) toast('Some settings need a hand', failed.map((f) => f.error).join(' '), 'warn');
    await verify(btn, res);
  }

  async function verify(btn, res) {
    btn.disabled = true;
    btn.innerHTML = '<span class="spinner" style="width:18px;height:18px;border-width:2px"></span> Checking the result…';
    try {
      state.micVerify = await api.verifyMic();
      drawVerify(res);
    } catch (e) {
      // Applied, but the check failed: say so, offer the check again and a way on.
      api.logUi('error', 'micapply.verify', e && e.stack ? e.stack : e);
      page.querySelector('.apply-error')?.remove();
      const box = html('<div class="apply-error stack"></div>');
      box.append(errorBanner(new Error("Your settings are applied, but we couldn't check the result. " + (e && e.message ? e.message : '')), () => verify(btn, res)));
      const next = html('<div class="row"><button class="btn lg" id="skip">Continue without the check</button></div>');
      next.querySelector('#skip').addEventListener('click', () => go(isRouteHidden('camera') ? 'home' : 'camera'));
      box.append(next);
      page.append(box);
      btn.disabled = false;
      btn.textContent = 'Apply to OBS';
    }
  }

  function drawVerify(res) {
    const { before, after } = state.micVerify;
    page.innerHTML = '';
    page.append(html(`<div class="stack" style="gap:6px"><span class="eyebrow">Before &amp; after</span><h1>Hear the difference</h1>
      <p class="lead">Same recording — before, and with your new settings. Try it on "phone" too: that's how a lot of viewers hear you.</p></div>`));
    if (res?.restart_obs_needed) page.append(html('<div class="banner warn"><span><b>Restart OBS</b> to load the new mic settings.</span></div>'));

    const scores = html(`<div class="card"><div class="row" style="gap:28px;justify-content:center">
      <div class="stack" style="align-items:center;gap:6px"><div class="b"></div><span class="subtle">Before</span></div>
      <div style="font-size:28px;color:var(--text-3)">→</div>
      <div class="stack" style="align-items:center;gap:6px"><div class="a"></div><span class="subtle">After</span></div>
    </div></div>`);
    scores.querySelector('.b').append(scoreRing(before.score.value, { label: 'before' }));
    scores.querySelector('.a').append(scoreRing(after.score.value, { label: 'after' }));
    page.append(scores);

    const ab = html(`<div class="stack">
      <div class="ab">
        <button class="play before" data-which="before"><span class="circle">${icons.play}</span><b>Before</b><span class="subtle">Your mic as it was</span></button>
        <button class="play after" data-which="after"><span class="circle">${icons.play}</span><b>After</b><span class="subtle">With your new settings</span></button>
      </div>
      <label class="toggle"><input type="checkbox" id="phone"/> <span>How it sounds on a phone speaker</span></label>
    </div>`);
    const phone = ab.querySelector('#phone');
    ab.querySelectorAll('.play').forEach((b) => b.addEventListener('click', () => toggle(b, ab, phone.checked)));
    phone.addEventListener('change', () => {
      const cur = ab.querySelector('.play.playing');
      if (cur) toggle(cur, ab, phone.checked, true);
    });
    page.append(ab);

    const beforeIds = new Set(before.findings.filter((f) => f.severity !== 'info').map((f) => f.id));
    const afterOpen = after.findings.filter((f) => f.severity !== 'info');
    const fixed = [...beforeIds].filter((id) => !afterOpen.find((f) => f.id === id));
    const fixedTitles = before.findings.filter((f) => fixed.includes(f.id)).map((f) => f.title);
    const res2 = html(`<div class="grid-2">
      <div class="card stack"><h3>Fixed</h3>${fixedTitles.length ? `<ul class="explain" style="margin:0;padding:0">${fixedTitles.map((t) => `<li><span class="ico">${icons.check}</span><span>${esc(t)}</span></li>`).join('')}</ul>` : '<p class="muted">Nothing new — your mic already sounded good.</p>'}</div>
      <div class="card stack"><h3>Still to do</h3>${afterOpen.length ? `<ul class="explain" style="margin:0;padding:0">${afterOpen.map((f) => `<li><span class="ico" style="color:var(--warn)">${icons.wrench}</span><span><b>${esc(f.title)}</b><div class="muted">${esc(f.fix)}</div></span></li>`).join('')}</ul>` : '<p class="muted">Nothing — you\'re all set.</p>'}</div>
    </div>`);
    page.append(res2);

    const noCam = isRouteHidden('camera');
    const next = html(`<div class="row"><button class="btn primary lg" id="cam">${noCam ? 'Done: see my setup' : 'Next: your camera'}</button><button class="btn lg" id="ab">Hear my new sound</button><button class="btn lg" id="again">Retest my mic</button></div>`);
    next.querySelector('#cam').addEventListener('click', () => go(noCam ? 'home' : 'camera'));
    next.querySelector('#ab').addEventListener('click', () => go('ab-record'));
    next.querySelector('#again').addEventListener('click', () => go('mic'));
    page.append(next);
    if (isTuneRelevant()) {
      const t = html('<div class="card tune-cta"><div class="stack" style="gap:4px;flex:1;min-width:240px"><b>Want to tune your tone too?</b><span class="muted">We can adjust the tone of your voice to your mic. You hear Before and After first, and you can undo it.</span></div><button class="btn" id="tune">Make it sound better</button></div>');
      t.querySelector('#tune').addEventListener('click', () => go('tune'));
      page.append(t);
    }
  }

  async function toggle(btn, ab, phone, keep = false) {
    const which = btn.dataset.which;
    const wasPlaying = btn.classList.contains('playing');
    ab.querySelectorAll('.play').forEach((b) => { b.classList.remove('playing'); b.querySelector('.circle').innerHTML = icons.play; });
    try {
      await api.stopPreview();
      if (wasPlaying && !keep) { playing = null; return; }
      await api.playPreview(which, phone);
      playing = which;
      btn.classList.add('playing');
      btn.querySelector('.circle').innerHTML = icons.stop;
    } catch (e) {
      toast("Couldn't play the preview", e.message, 'bad');
    }
  }

  return () => { if (playing) api.stopPreview().catch(() => {}); };
}
