// 11. Final take: read the same passage again; we run it through your new settings (offline, not live).
import { api } from '../api.js';
import { esc, html, errorBanner } from '../components.js';
import { state } from '../state.js';
import { SCRIPT } from './mictest.js';
import { abErrorWords } from '../backupcopy.js';

const SECS = 11;

export async function render(root, { go }) {
  if (!state.micId || !state.micContext) {
    root.append(html('<div class="banner warn"><span>Test your mic first, then we can compare.</span></div>'));
    const b = html('<button class="btn primary" style="margin-top:12px">Start the mic test</button>');
    b.addEventListener('click', () => go('mic'));
    root.append(b);
    return;
  }
  const page = html(`<section class="stack-lg">
    <div class="stack" style="gap:6px"><span class="eyebrow">Hear my new sound</span><h1 id="title">Read it one more time</h1>
    <p class="lead" id="text">Same passage, same distance from your mic, same voice as before. We run it through your new settings to show how you should sound. This is not a live recording from OBS.</p></div>
    <div class="card stack" id="stage"></div></section>`);
  root.append(page);
  const stage = page.querySelector('#stage');
  let cancelled = false;
  let timer = null;

  intro();

  function intro() {
    stage.innerHTML = `<div class="script">${SCRIPT.replace(/\n/g, '<br/>')}</div>
      <div class="row"><button class="btn primary lg" id="go">Start reading</button><button class="btn ghost" id="cant">I can't do this right now</button></div>`;
    stage.querySelector('#go').addEventListener('click', run);
    stage.querySelector('#cant').addEventListener('click', () => go('ab'));
  }

  async function run() {
    stage.innerHTML = `<div class="script">${SCRIPT.replace(/\n/g, '<br/>')}</div>
      <div class="row" style="gap:12px"><div class="prompt" id="count" aria-live="polite">Get ready…</div></div>`;
    const count = stage.querySelector('#count');
    try {
      await api.startMicTest(state.micId);
      await sleep(1500);
      if (cancelled) return;
      await api.markStep('read', 'start');
      for (let left = SECS; left > 0; left--) {
        count.textContent = `Reading… ${left}`;
        await sleep(1000);
        if (cancelled) return;
      }
      await api.markStep('read', 'end');
      count.innerHTML = '<span class="spinner tiny"></span> Running it through your new settings…';
      await api.recordFinal(state.micId, state.micContext, state.micConstraints);
      go('ab');
    } catch (e) {
      stage.innerHTML = '';
      const w = abErrorWords(e);
      stage.append(e && e.code ? html(`<div class="banner bad"><div class="stack" style="gap:4px"><b>${esc(w.title)}</b><span>${esc(w.text)}</span></div></div>`) : errorBanner(e, intro));
      if (e && e.code) {
        const goElsewhere = w.action && w.action !== 'ab-record';
        const retry = html(`<button class="btn primary" style="margin-top:8px">${goElsewhere ? esc(w.actionLabel) : 'Try again'}</button>`);
        retry.addEventListener('click', goElsewhere ? () => go(w.action) : intro);
        stage.append(retry);
      }
      const skip = html('<button class="btn ghost" style="margin-top:8px">I can\'t do this right now</button>');
      skip.addEventListener('click', () => go('ab'));
      stage.append(skip);
    }
  }

  function sleep(ms) { return new Promise((r) => { timer = setTimeout(r, ms); }); }
  return () => { cancelled = true; clearTimeout(timer); };
}
