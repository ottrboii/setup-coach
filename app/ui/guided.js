// "Windows wouldn't let us do this one" -> a guided fix. An apply failure that
// carries `guided` (a CoachStep) becomes one plain explanation and one big
// button. The button hands the step to the live coaching screen, which opens
// the exact Windows page, watches for the change and brings the user back
// (no "did you do it?"). When it passes, the caller's screen re-checks.
import { esc, html } from './components.js';
import { state } from './state.js';

/** Failures from an ApplyResult that can be finished with a guided step. */
export const guidedFailures = (res) => (res && res.failed ? res.failed.filter((f) => f && f.guided) : []);

/** Failures with no guided step: only a message. */
export const plainFailures = (res) => (res && res.failed ? res.failed.filter((f) => f && !f.guided) : []);

/** Pre-stream heads-up for cameras that usually switch off after a while (`QuickChecks.runtime_warnings`). */
export function runtimeWarningBanners(quick) {
  return ((quick && quick.runtime_warnings) || []).map((w) => html(`<div class="banner warn" role="alert"><div class="stack" style="gap:4px"><b>${esc(w.camera)} may switch off soon</b><span>${esc(w.message)}</span></div></div>`));
}

/** Open the coaching screen for one guided step; `from` is the route to return to. */
export function startGuidedFix(failure, from, go) {
  state.coach = { area: 'mic', steps: [failure.guided], index: 0, status: {}, returnTo: from, autoOpen: true };
  go('coach');
}

/** One card: what happened, in plain words, plus the big "Show me how" button. */
export function guidedFixCard(failure, { from, go, label }) {
  const g = failure.guided;
  const clicks = g.open_target?.steps?.length;
  const card = html(`
    <div class="card guided-fix stack" style="gap:10px;border-left:4px solid var(--accent, #6c5ce7)">
      <b>${esc(label || g.title)}</b>
      <p class="muted">${esc(failure.error)}</p>
      <div class="row"><button class="btn primary lg" data-act="show">Show me how</button>
      <span class="subtle">${clicks ? `About ${clicks === 1 ? 'one click' : `${clicks} clicks`}. ` : ''}We open the right page and notice when it's done.</span></div>
    </div>`);
  card.querySelector('[data-act=show]').addEventListener('click', (e) => {
    e.currentTarget.disabled = true;
    startGuidedFix(failure, from, go);
  });
  return card;
}
