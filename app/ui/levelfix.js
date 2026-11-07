// "Fix my level": the app moves the real gain (GoXLR or Windows hardware level) while the user talks.
// Findings that can use it: too quiet, too loud, clipping. Only offered when the mic's gain is software.
import { api, on } from './api.js';
import { esc, html, icons, advanced, kvTable } from './components.js';
import { state } from './state.js';

export const LEVEL_FINDINGS = new Set(['mic.too_quiet', 'mic.too_hot', 'mic.clipping']);

/** Add the panel to a finding card when the finding is about level and the app can reach the gain. */
export function attachLevelFix(card, finding, opts = {}) {
  if (!LEVEL_FINDINGS.has(finding.id) || finding.severity === 'info' || !canFixLevel()) return false;
  const slot = card.querySelector('.slot') || card.querySelector('.body');
  slot.append(levelFixPanel(opts));
  return true;
}

/** True when the app itself can reach this mic's gain (GoXLR Utility or the Windows level). */
export function canFixLevel() {
  return !!state.micId && state.micContext?.gain === 'software';
}

const STATUS = {
  fixed: { cls: 'good', label: 'Fixed', icon: icons.check },
  couldnt: { cls: 'warn', label: "Couldn't fix it", icon: '<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M7 7l10 10M17 7L7 17"/></svg>' },
  needs_you: { cls: 'info', label: 'Needs you', icon: '<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 7v6M12 17h.01"/><circle cx="12" cy="12" r="9"/></svg>' },
};

const db = (v) => (typeof v === 'number' ? `${v > 0 ? '+' : ''}${v.toFixed(1)} dB` : 'n/a');
const dbfs = (v) => (typeof v === 'number' ? `${v.toFixed(1)} dBFS` : 'n/a');

/**
 * Panel with the button and the whole flow. opts: { deviceId, reason, beforeStart(), onFixed(), onUndone() }
 * Never claims success unless the backend says status === 'fixed'.
 */
export function levelFixPanel(opts = {}) {
  const deviceId = opts.deviceId || state.micId;
  const node = html('<div class="levelfix" aria-live="polite"></div>');
  let unlisten = null;
  let unlistenLevel = null;

  function stopListening() {
    if (unlisten) { unlisten(); unlisten = null; }
    if (unlistenLevel) { unlistenLevel(); unlistenLevel = null; }
  }

  function drawIdle() {
    node.innerHTML = '';
    const row = html(`<div class="lf-idle"><div class="stack" style="gap:2px;flex:1;min-width:200px">
      <b>${esc(opts.reason || 'We can set this for you')}</b>
      <span class="subtle">We will turn your mic level up or down ourselves while you talk, then check it. Takes about 15 seconds.</span></div>
      <button class="btn primary" data-act="fix">${icons.wrench}<span>Fix my level</span></button></div>`);
    row.querySelector('[data-act=fix]').addEventListener('click', run);
    node.append(row);
  }

  async function run() {
    node.innerHTML = '';
    const view = html(`<div class="lf-run">
      <div class="lf-wave" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i><i></i><i></i></div>
      <div class="stack" style="gap:2px;flex:1;min-width:200px"><b>Talk normally for a few seconds</b>
        <span class="subtle lf-progress">Getting ready...</span></div></div>`);
    node.append(view);
    const bars = [...view.querySelectorAll('.lf-wave i')];
    const progress = view.querySelector('.lf-progress');
    try {
      if (opts.beforeStart) await opts.beforeStart();
      unlisten = await on('level-fix-progress', (p) => { if (p && p.text) progress.textContent = p.text; });
      // The meter is a bonus: when the backend sends live levels the bars follow your voice, otherwise they just breathe.
      unlistenLevel = await on('mic-level', (l) => {
        view.classList.add('live');
        const base = Math.max(0, Math.min(1, (l.rms_db + 60) / 50));
        bars.forEach((b, i) => { b.style.transform = `scaleY(${(0.15 + base * (0.55 + 0.45 * Math.abs(Math.sin(i * 1.7 + performance.now() / 160)))).toFixed(2)})`; });
      });
      const res = await api.fixMicLevel(deviceId);
      stopListening();
      drawResult(res);
    } catch (e) {
      stopListening();
      drawError(e);
    }
  }

  function drawError(e) {
    node.innerHTML = '';
    const box = html(`<div class="lf-result warn"><span class="lf-ico">${STATUS.couldnt.icon}</span>
      <div class="stack" style="gap:4px;flex:1;min-width:200px"><b>${STATUS.couldnt.label}</b>
      <span>${esc((e && e.message) || 'Something went wrong, and nothing was changed.')}</span></div></div>`);
    const again = html('<button class="btn sm">Try again</button>');
    again.addEventListener('click', run);
    box.append(again);
    node.append(box);
  }

  function drawResult(res) {
    node.innerHTML = '';
    // Anything that is not exactly "fixed" is never shown as success.
    const st = STATUS[res && res.status] || STATUS.couldnt;
    const line = (res && res.line) || "We couldn't change your level this time.";
    const box = html(`<div class="lf-result ${st.cls}"><span class="lf-ico">${st.icon}</span>
      <div class="stack" style="gap:4px;flex:1;min-width:200px"><b>${st.label}</b><span class="lf-line">${esc(line)}</span></div>
      <div class="row lf-actions" style="gap:8px"></div></div>`);
    const actions = box.querySelector('.lf-actions');
    if (res && res.backup_id) {
      const undo = html('<button class="btn sm" data-act="undo">Undo</button>');
      undo.addEventListener('click', () => undoFix(res, undo));
      actions.append(undo);
    }
    if (res && res.status === 'fixed' && opts.onFixed) {
      const re = html('<button class="btn sm primary">Check my mic again</button>');
      re.addEventListener('click', () => opts.onFixed());
      actions.append(re);
    }
    if (!res || res.status !== 'fixed') {
      const again = html('<button class="btn sm">Try again</button>');
      again.addEventListener('click', run);
      actions.append(again);
    }
    node.append(box);
    if (res && (res.gain_before_db != null || res.peak_before_dbfs != null)) {
      node.append(advanced('Advanced: what changed', kvTable([
        ['Changed', res.changed || 'nothing'],
        ['Gain', `${db(res.gain_before_db)} to ${db(res.gain_after_db)}`],
        ['Speech peaks', `${dbfs(res.peak_before_dbfs)} to ${dbfs(res.peak_after_dbfs)}`],
        ['Steps', String(res.steps ?? 0)],
      ])));
    }
  }

  async function undoFix(res, btn) {
    btn.disabled = true;
    btn.textContent = 'Putting it back...';
    try {
      const r = await api.restoreBackup(res.backup_id);
      const failed = (r && r.failed) || [];
      const skipped = (r && r.skipped) || [];
      const box = node.querySelector('.lf-result');
      if (failed.length || skipped.length) {
        // Be honest: say what did not go back.
        const why = [...failed, ...skipped].map((x) => x.error).filter(Boolean)[0] || 'Part of it could not be put back.';
        btn.disabled = false;
        btn.textContent = 'Undo';
        box.querySelector('.lf-line').textContent = `We could not put everything back. ${why}`;
        return;
      }
      box.className = 'lf-result info';
      box.querySelector('b').textContent = 'Put back';
      box.querySelector('.lf-line').textContent = 'Your mic level is back to how it was before.';
      box.querySelector('.lf-ico').innerHTML = icons.check;
      box.querySelector('.lf-actions').innerHTML = '';
      const again = html('<button class="btn sm">Fix my level again</button>');
      again.addEventListener('click', run);
      box.querySelector('.lf-actions').append(again);
      if (opts.onUndone) opts.onUndone();
    } catch (e) {
      btn.disabled = false;
      btn.textContent = 'Undo';
      node.querySelector('.lf-line').textContent = (e && e.message) || "We couldn't put it back. Nothing else was changed.";
    }
  }

  drawIdle();
  return node;
}
