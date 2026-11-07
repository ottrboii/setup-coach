// 4. Mic report card: score, ranked findings, "Done — retest" / "I can't do this".
import { api } from '../api.js';
import { esc, html, scoreRing, scoreWord, findingCard, upgradeCard, toast, advanced, kvTable, fmt } from '../components.js';
import { state, MIC_CONSTRAINT, selectedMic } from '../state.js';
import { openMicChoice, yourMicLine } from '../micpicker.js';
import { profileCard, profileLine } from '../profilepicker.js';
import { cleanAnswers } from './mictest.js';
import { attachLevelFix } from '../levelfix.js';

const STATUS = { met: ['ok', 'On target'], close: ['major', 'Close'], missed: ['critical', 'Off target'], not_measured: ['', 'Not measured'] };

export async function render(root, { go }) {
  if (!state.micReport) {
    root.append(html('<div class="banner warn"><span>Run the mic test first.</span></div>'));
    const b = html('<button class="btn primary" style="margin-top:12px">Start the mic test</button>');
    b.addEventListener('click', () => go('mic'));
    root.append(b);
    return;
  }
  const page = html('<section class="stack-lg"></section>');
  root.append(page);
  draw();

  function draw() {
    const r = state.micReport;
    page.innerHTML = '';
    const open = r.findings.filter((f) => f.severity !== 'info');
    const handled = r.findings.filter((f) => f.severity === 'info');
    const physical = open.filter((f) => f.fix_kind !== 'auto');

    const head = html(`
      <div class="card hero">
        <div class="row" style="gap:28px">
          <div class="ring"></div>
          <div class="stack" style="gap:8px;flex:1;min-width:260px">
            <span class="eyebrow">Your mic report</span>
            <div id="miclinebox"></div>
            <div id="profilelinebox"></div>
            <div class="row" style="gap:10px"><h1>${esc(scoreWord(r.score.value))}</h1>${delta(state.scoreBefore?.mic, r.score.value)}</div>
            <p class="muted">${open.length
              ? `${open.length} thing${open.length === 1 ? '' : 's'} holding you back. ${physical.length ? `Start with the quick tweaks below — they matter more than any setting.` : 'All of them are settings we can apply for you.'}`
              : "Nothing big to fix. We'll still fine-tune your settings."}</p>
          </div>
        </div>
      </div>`);
    head.querySelector('.ring').append(scoreRing(r.score.value, { label: 'mic' }));
    const mic = selectedMic();
    const slot = html('<div id="choice-slot"></div>');
    // Changing the mic re-checks the same recording against the new mic; no retest needed.
    head.querySelector('#miclinebox').append(yourMicLine(mic, () => openMicChoice(slot, mic, { onDone: () => reanalyse() })));
    head.querySelector('#profilelinebox').append(profileLine(() => {
      const box = page.querySelector('#profile-slot');
      if (box.firstChild) { box.innerHTML = ''; return; }
      box.append(profileCard(mic, { mode: 'switch', go }));
      box.scrollIntoView({ block: 'nearest' });
    }));
    page.append(head);
    page.append(html('<div id="profile-slot"></div>'));
    page.append(slot);

    if (open.length) {
      if (physical.length) {
        const coachCard = html(`<div class="card coach-cta"><div class="stack" style="gap:4px;flex:1;min-width:240px"><h2>Let's fix these together</h2>
          <p class="muted">We'll walk you through each one live — you'll see and hear it improve as you go. Takes about ${Math.max(2, physical.length)} minutes.</p></div>
          <button class="btn primary lg">Coach me through it</button></div>`);
        coachCard.querySelector('button').addEventListener('click', () => { state.coach = { area: 'mic', steps: null, index: 0, status: {} }; go('coach'); });
        page.append(coachCard);
      }
      page.append(html(`<div class="stack" style="gap:4px"><h2>What to do, most important first</h2>
        <p class="muted">Physical fixes first, then we set the settings. Can't change something? That's fine — tell us and we'll tune around it.</p></div>`));
      const list = html('<div class="findings"></div>');
      const shownUpgrades = new Set();
      for (const f of open) {
        const card = findingCard(f, { onDone: () => retest(), onCant: (fd, btn) => cant(fd, btn) });
        attachLevelFix(card, f, { deviceId: state.micId, reason: 'We can fix this one for you', onFixed: () => retest() });
        list.append(card);
        if (f.suggest_product && !shownUpgrades.has(f.suggest_product)) {
          shownUpgrades.add(f.suggest_product);
          const u = upgradeCard(f.suggest_product);
          if (u && f.fix_kind !== 'auto') list.append(u);
        }
      }
      page.append(list);
    }

    if (handled.length) {
      page.append(html('<h2>Handled for you</h2>'));
      const list = html('<div class="findings"></div>');
      handled.forEach((f) => list.append(findingCard(f, { visual: false })));
      page.append(list);
    }

    const cta = html(`
      <div class="card hero stack">
        <h2>Ready when you are</h2>
        <p class="muted">Made a physical change? Retest first so we tune for the new setup. Otherwise, apply your settings now.</p>
        <div class="row">${physical.length ? '<button class="btn primary lg" id="coach">Coach me through it</button><button class="btn lg" id="apply">Skip to settings</button>' : '<button class="btn primary lg" id="apply">Set up my mic</button>'}<button class="btn lg" id="retest">Retest my mic</button></div>
      </div>`);
    cta.querySelector('#apply').addEventListener('click', () => go('mic-apply'));
    const coachBtn = cta.querySelector('#coach');
    if (coachBtn) coachBtn.addEventListener('click', () => { state.coach = { area: 'mic', steps: null, index: 0, status: {} }; go('coach'); });
    cta.querySelector('#retest').addEventListener('click', retest);
    page.append(cta);

    const m = r.metrics;
    const adv = html('<div class="stack"></div>');
    adv.append(html(`<table class="kv"><thead><tr><th>Target</th><th>Yours</th><th>Goal</th><th></th></tr></thead><tbody>
      ${r.targets.map((t) => `<tr><td>${esc(t.label)}</td><td>${fmt(t.value, 1)}</td><td>${esc(t.target)}</td><td><span class="chip ${STATUS[t.status]?.[0] || ''}">${STATUS[t.status]?.[1] || esc(t.status)}</span></td></tr>`).join('')}
    </tbody></table>`));
    adv.append(kvTable([
      ['Speech peaks', fmt(m.speech_peak_dbfs, 1, ' dBFS')],
      ['Noise floor', fmt(m.noise_floor_dbfs, 1, ' dBFS')],
      ['Voice above noise', fmt(m.snr_db, 1, ' dB')],
      ['Loudness', fmt(m.speech_lufs, 1, ' LUFS')],
      ['Room decay', fmt(m.reverb_decay_s, 2, ' s')],
      ['Voice pitch', fmt(m.f0_hz, 0, ' Hz')],
    ]));
    page.append(advanced('Advanced: the numbers', adv));
  }

  async function reanalyse() {
    try {
      state.micContext = await api.micContext(state.micId, cleanAnswers());
      state.micReport = await api.rescanMic(state.micContext, state.micConstraints);
      toast('Updated', 'We looked at your recording again, this time for your mic.', 'good');
    } catch (e) {
      toast("Couldn't re-check your recording", `${e.message} Run the test again to use your new answer.`, 'bad');
    }
    draw();
  }

  function retest() {
    go('mic');
  }

  async function cant(f, btn) {
    const c = MIC_CONSTRAINT[f.id];
    btn.disabled = true;
    if (!c) {
      // No settings workaround exists for this one; acknowledge and move on.
      state.micReport = { ...state.micReport, findings: state.micReport.findings.map((x) => (x.id === f.id ? { ...x, severity: 'info', fix: "No problem — we'll leave this one. Your settings will still help.", can_decline: false } : x)) };
      draw();
      return;
    }
    if (!state.micConstraints.includes(c)) state.micConstraints.push(c);
    btn.innerHTML = '<span class="spinner" style="width:16px;height:16px;border-width:2px"></span> Re-tuning…';
    try {
      state.micReport = await api.rescanMic(state.micContext, state.micConstraints);
      toast('Got it', "We've re-tuned your settings around that.", 'good');
      draw();
    } catch (e) {
      toast("Couldn't re-tune", e.message, 'bad');
      btn.disabled = false;
      btn.textContent = "I can't do this";
    }
  }
}

/** "Up from 62" chip after coaching. */
export function delta(before, now) {
  if (before === null || before === undefined || now === undefined) return '';
  const d = Math.round(now - before);
  if (d === 0) return '<span class="chip">Same as before</span>';
  return `<span class="chip ${d > 0 ? 'ok' : 'major'} delta-chip">${d > 0 ? '▲' : '▼'} ${d > 0 ? 'Up' : 'Down'} from ${Math.round(before)}</span>`;
}
