// Setup Coach UI: tiny hash router + sidebar.
import { isMock, api } from './api.js';
import { esc, html, errorBanner } from './components.js';
import { state } from './state.js';
import * as welcome from './screens/welcome.js';
import * as scan from './screens/scan.js';
import * as mictest from './screens/mictest.js';
import * as micreport from './screens/micreport.js';
import * as micapply from './screens/micapply.js';
import * as camera from './screens/camera.js';
import * as health from './screens/health.js';
import * as dashboard from './screens/dashboard.js';
import * as coach from './screens/coach.js';
import * as backups from './screens/backups.js';
import * as ab from './screens/ab.js';
import * as abrecord from './screens/abrecord.js';
import * as sync from './screens/sync.js';
import { isRouteHidden, activeInfo, ensureProfileForNav, PROFILE_EVENT } from './profilenav.js';
import { initFeedback } from './feedback.js';

const ROUTES = {
  welcome, scan, mic: mictest, 'mic-report': micreport, 'mic-apply': micapply, camera, health, home: dashboard, coach, backups, ab, 'ab-record': abrecord, sync,
};

const NAV = [
  { route: 'welcome', label: 'Welcome', done: () => !!state.env },
  { route: 'scan', label: 'Quick scan', done: () => !!state.quick },
  { route: 'mic', label: 'Microphone', match: ['mic', 'mic-report', 'mic-apply'], done: () => !!state.micVerify },
  { route: 'camera', label: 'Camera', done: () => state.cameraApplied },
  { route: 'sync', label: 'Lip sync', done: () => state.syncReport?.verdict === 'in_sync' },
  { route: 'health', label: 'Stream health', done: () => !!state.healthReport },
  { route: 'backups', label: 'Backups & undo', match: ['backups', 'ab', 'ab-record'], done: () => false },
  { route: 'home', label: 'Home', done: () => false },
];

let cleanup = null;

export function go(route) {
  if (location.hash !== `#/${route}`) location.hash = `#/${route}`;
  else render();
}

function current() {
  const r = location.hash.replace(/^#\/?/, '') || 'welcome';
  return ROUTES[r] ? r : 'welcome';
}

function renderNav(active) {
  const nav = document.getElementById('nav');
  nav.innerHTML = '';
  // Screens the chosen profile does not need go under "Not needed for ...". The one you are on stays in the main list.
  const coachArea = active === 'coach' ? (state.coach?.area === 'camera' ? 'camera' : 'mic') : null;
  const isActiveItem = (n) => n.route === active || (n.match || []).includes(active) || n.route === coachArea;
  const hidden = NAV.filter((n) => isRouteHidden(n.route) && !isActiveItem(n));
  const shown = NAV.filter((n) => !hidden.includes(n));
  const link = (n, num) => {
    const isActive = isActiveItem(n);
    const done = n.done();
    return html(`<a href="#/${n.route}" class="${isActive ? 'active' : ''} ${done ? 'done' : ''}" ${isActive ? 'aria-current="page"' : ''}>
      <span class="dot">${done ? '✓' : num}</span><span class="label">${esc(n.label)}</span></a>`);
  };
  shown.forEach((n, i) => nav.append(link(n, i + 1)));
  if (hidden.length) {
    const more = html(`<details class="nav-more"><summary>Not needed for ${esc(activeInfo()?.label || 'this setup')}</summary></details>`);
    hidden.forEach((n) => more.append(link(n, '·')));
    nav.append(more);
  }
  const pill = document.getElementById('obs-pill');
  const obs = state.env?.obs;
  const led = !obs ? '' : obs.connected ? 'good' : obs.running ? 'warn' : 'bad';
  const text = !obs ? 'OBS: not checked yet' : obs.connected ? `OBS ${obs.version || ''} connected` : obs.running ? 'OBS: not connected' : 'OBS is not running';
  pill.innerHTML = `<div class="pill"><span class="led ${led}"></span><span>${esc(text)}${isMock ? '<br/><span class="subtle" title="Running in a browser with sample data">Demo data</span>' : ''}</span></div>`;
}

async function render() {
  if (cleanup) {
    try { cleanup(); } catch (_) { /* ignore */ }
    cleanup = null;
  }
  const route = current();
  renderNav(route);
  if (!state.profile || !state.profileInfos) ensureProfileForNav().then(() => renderNav(current()));
  const main = document.getElementById('main');
  main.innerHTML = '';
  main.scrollTop = 0;
  window.scrollTo(0, 0);
  try {
    const res = await ROUTES[route].render(main, { go, refreshNav: () => renderNav(current()) });
    if (typeof res === 'function') cleanup = res;
  } catch (e) {
    showCrash(main, `screen:${route}`, e, () => render());
  }
  main.focus({ preventScroll: true });
}

/** Never leave a blank or frozen screen: say what happened in plain words, offer a way on, and log it. */
function showCrash(main, source, e, retry) {
  api.logUi('error', source, e && e.stack ? e.stack : e);
  main.querySelector('.crash')?.remove();
  const box = html('<div class="crash" style="margin-top:16px"></div>');
  box.append(errorBanner(new Error("Something went wrong on this screen. It's been written to the log. Try again, or go back to the start."), retry));
  const back = html('<a class="btn sm" href="#/welcome" style="margin-top:8px">Back to the start</a>');
  box.append(back);
  main.append(box);
}

window.addEventListener('error', (ev) => {
  const main = document.getElementById('main');
  if (main) showCrash(main, 'window.error', ev.error || ev.message, () => render());
});
window.addEventListener('unhandledrejection', (ev) => {
  const main = document.getElementById('main');
  if (main) showCrash(main, 'unhandledrejection', ev.reason, () => render());
});

window.addEventListener('hashchange', render);
window.addEventListener(PROFILE_EVENT, () => renderNav(current()));
initFeedback();
if (document.readyState === 'loading') window.addEventListener('DOMContentLoaded', render);
else render();
