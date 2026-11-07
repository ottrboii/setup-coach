// Setup profile picker: "What is this setup for?" (Streaming / Recording / Call).
// Commands: list_setup_profiles, get_setup_profile, set_setup_profile (API.md "Setup profiles").
// The UI never edits context.profile itself: it switches, then asks for mic_context again.
import { api } from './api.js';
import { esc, html, errorBanner, toast, advanced, kvTable } from './components.js';
import { state } from './state.js';
import { profileChanged } from './profilenav.js';

const FALLBACK_LABEL = { streaming: 'Streaming', voiceover: 'Recording / voiceover', call: 'Call / meeting' };
export const profileLabel = (id) => state.profileInfos?.find((p) => p.id === id)?.label || FALLBACK_LABEL[id] || 'Streaming';

/** Load the three profiles for this mic and the session's current one. Cached per device. */
export async function loadProfiles(deviceId) {
  if (!state.profileInfos || state.profileDevice !== deviceId) {
    state.profileInfos = await api.listSetupProfiles(deviceId);
    state.profileDevice = deviceId;
  }
  if (!state.profile) state.profile = await api.getSetupProfile();
  return state.profileInfos;
}

const band = (b) => (b ? `${b[0]} to ${b[1]}` : 'not set');
function advancedBlock(info) {
  const t = info.targets;
  const rows = [['Mic distance', info.distance_cm ? `${info.distance_cm[0]} to ${info.distance_cm[1]} cm` : 'by mic']];
  if (t) {
    rows.push(['Talking peaks (dBFS)', band(t.speech_peak)], ['Loudest (dBFS) good / close', `${t.loud_peak.met} / ${t.loud_peak.close}`],
      ['Noise floor (dBFS) good / close', `${t.noise_floor.met} / ${t.noise_floor.close}`], ['Voice above noise (dB) good / close', `${t.snr.met} / ${t.snr.close}`],
      ['Room echo (s) good / close', `${t.room_decay.met} / ${t.room_decay.close}`]);
  }
  return advanced('The numbers for this profile', kvTable(rows));
}

/**
 * The card. mode 'start' (before the first test): choosing just remembers it.
 * mode 'switch' (after a report): choosing re-applies the stored setup, then shows
 * placement and offers a quick level + distance re-check instead of saying "done".
 */
export function profileCard(mic, { mode = 'start', go, onSwitched } = {}) {
  const card = html(`<div class="card stack profile-card" role="group" aria-label="What is this setup for?">
    <div class="stack" style="gap:4px"><h2>${mode === 'start' ? 'What is this setup for?' : 'Switch what this setup is for'}</h2>
    <p class="muted">We tune to how you will use it. You can switch any time with one click.</p></div>
    <div class="profile-options" role="radiogroup" aria-label="Setup type"></div>
    <div class="profile-after" aria-live="polite"></div></div>`);
  const opts = card.querySelector('.profile-options');
  const after = card.querySelector('.profile-after');
  let busy = false;

  function drawOptions(infos) {
    opts.innerHTML = '';
    for (const p of infos) {
      const on = p.id === state.profile;
      const b = html(`<button type="button" class="profile-opt ${on ? 'selected' : ''}" role="radio" aria-checked="${on}" data-profile="${esc(p.id)}" tabindex="${on ? 0 : -1}">
        <b>${esc(p.label)}</b><span class="muted">${esc(p.summary)}</span></button>`);
      b.addEventListener('click', () => choose(p.id));
      b.addEventListener('keydown', (ev) => {
        const i = infos.findIndex((x) => x.id === p.id);
        const d = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[ev.key];
        if (!d) return;
        ev.preventDefault();
        const next = infos[(i + d + infos.length) % infos.length];
        choose(next.id);
      });
      opts.append(b);
    }
  }
  function showPlacement(info, sw) {
    after.innerHTML = '';
    const box = html(`<div class="profile-place stack" style="gap:8px">
      <p><b>Where to put your mic</b><br/>${esc(info.placement)}</p>
      ${info.room_advice ? `<p class="muted">${esc(info.room_advice)}</p>` : ''}</div>`);
    box.append(advancedBlock(info));
    after.append(box);
    if (!sw) return;
    const note = sw.applied
      ? `We loaded the settings you saved for ${info.label}.`
      : `We haven't set up ${info.label} yet, so your earlier settings are still in place.`;
    after.append(html(`<div class="banner info"><span>${esc(note)} Position and mic level are not saved, so let's check them now.</span></div>`));
    const row = html(`<div class="row"><button class="btn primary" data-act="recheck">Check my level and distance</button>
      ${sw.needs_scan ? '<button class="btn" data-act="scan">Run the full mic test</button>' : ''}
      <button class="btn ghost" data-act="cant">I can't do this</button></div>`);
    after.append(row);
    row.querySelector('[data-act=recheck]').addEventListener('click', () => {
      state.coach = { area: 'mic', steps: null, index: 0, status: {}, only: ['set_gain', 'find_distance'], backTo: 'mic-report' };
      go('coach');
    });
    row.querySelector('[data-act=scan]')?.addEventListener('click', () => go('mic'));
    row.querySelector('[data-act=cant]').addEventListener('click', () => {
      if (!state.micConstraints.includes('cant_move_mic_closer')) state.micConstraints.push('cant_move_mic_closer');
      toast('No problem', "We'll tune around your setup instead.", 'good');
      go('mic');
    });
  }
  async function choose(id) {
    if (busy) return;
    const was = state.profile;
    if (id === was && mode === 'start') return;
    busy = true;
    try {
      const infos = state.profileInfos;
      const sw = await api.setSetupProfile(id);
      state.profile = sw.profile;
      // The backend now stamps the profile into the context; ask again.
      if (mic) { try { state.micContext = await api.micContext(mic.id, state.answers); } catch (_) { /* next test asks again */ } }
      if (id !== was) { state.micVerify = null; state.profileSwitchedAt = Date.now(); }
      drawOptions(infos);
      profileChanged();
      opts.querySelector('[aria-checked=true]')?.focus();
      showPlacement(infos.find((p) => p.id === sw.profile), mode === 'switch' && id !== was ? sw : null);
      if (id !== was && onSwitched) onSwitched(sw);
    } catch (e) {
      after.innerHTML = '';
      after.append(errorBanner(e, () => choose(id)));
    } finally { busy = false; }
  }

  (async () => {
    try {
      const infos = await loadProfiles(mic ? mic.id : '');
      drawOptions(infos);
      showPlacement(infos.find((p) => p.id === state.profile) || infos[0], null);
    } catch (e) {
      opts.innerHTML = '';
      opts.append(errorBanner(e, () => { state.profileInfos = null; card.replaceWith(profileCard(mic, { mode, go, onSwitched })); }));
    }
  })();
  return card;
}

/** One-line "Set up for: Streaming · Change" for the report. */
export function profileLine(onChange) {
  const line = html(`<p class="muted">Set up for: <b>${esc(profileLabel(state.profile))}</b> · <a href="#/mic-report" class="change-profile">Change</a></p>`);
  line.querySelector('a').addEventListener('click', (ev) => { ev.preventDefault(); onChange(); });
  return line;
}
