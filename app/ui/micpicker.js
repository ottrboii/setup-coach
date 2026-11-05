// "What mic is plugged into this?" - names the physical mic behind a Windows
// input (a GoXLR or audio interface hides it). Shared by Welcome, Mic test and
// the mic report. Commands: list_mic_profiles, set_mic_behind (API.md).
import { api } from './api.js';
import { esc, html, errorBanner, toast } from './components.js';
import { state } from './state.js';

/** "Chat Mic" from "Chat Mic (TC-Helicon GoXLR Mini)". */
function shortName(name) {
  const s = String(name || '').replace(/\s*\([^)]*\)\s*$/, '').trim();
  return s || String(name || '');
}

/** "Chat Mic (Electro-Voice RE20)" when the user named the mic, else the Windows name. */
export function yourMicName(mic) {
  if (!mic) return '';
  const c = mic.mic_choice;
  if (c && c.status === 'chosen' && c.display_name) return `${shortName(mic.name)} (${c.display_name})`;
  return mic.name;
}

/** The "Your mic: ... · Change" line. `onChange` opens the picker. */
export function yourMicLine(mic, onChange, { id = 'yourmic' } = {}) {
  if (!mic) return html('<p class="muted">No mic picked yet · <a href="#/welcome">Pick your mic</a></p>');
  const c = mic.mic_choice;
  const note = c && c.status === 'other' ? ' <span class="subtle">(another mic)</span>' : '';
  const ask = mic.needs_mic_choice ? ' <span class="chip minor">Tell us what is plugged in</span>' : '';
  const line = html(`<p class="muted" id="${id}">Your mic: <b>${esc(yourMicName(mic))}</b>${note} · <a href="#/welcome" class="change-mic">Change</a>${ask}</p>`);
  line.querySelector('.change-mic').addEventListener('click', (ev) => {
    ev.preventDefault();
    onChange();
  });
  return line;
}

/** Drop everything that was worked out for the previous answer. */
function forgetOldResults() {
  state.micContext = null;
  state.micVerify = null;
  for (const k of ['mic_model_id', 'which_mic_on_interface', 'which_model']) delete state.answers[k];
}

let profilesCache = null;
async function loadProfiles() {
  if (!profilesCache) profilesCache = await api.listMicProfiles();
  return profilesCache;
}

/**
 * The picker card. `mic` is the device (AudioDeviceInfo). `onDone(mic)` runs
 * after an answer was saved (`mic` already updated); `onCancel` shows a Cancel
 * button (when there already is an answer, or the question is optional).
 */
export function micChoiceCard(mic, { onDone, onCancel } = {}) {
  const card = html(`
    <div class="card stack mic-choice" role="group" aria-label="What mic is plugged into this?">
      <div class="stack" style="gap:4px">
        <h2>What mic is plugged into this?</h2>
        <p class="muted"><b>${esc(shortName(mic.name))}</b> can hide the real microphone. Tell us which one you use and we'll tune for it. "Not sure" is fine.</p>
      </div>
      <input type="search" class="mic-search" placeholder="Search, e.g. RE20 or SM7B" aria-label="Search microphones" autocomplete="off" />
      <div class="mic-list choices" style="max-height:260px;overflow:auto;padding:2px" aria-live="polite"></div>
      <div class="mic-error"></div>
      <div class="row">
        <button class="btn" type="button" data-kind="other">Another mic (not in the list)</button>
        <button class="btn" type="button" data-kind="skipped">Not sure / skip</button>
        ${onCancel ? '<button class="btn ghost" type="button" data-kind="cancel">Cancel</button>' : ''}
      </div>
    </div>`);
  const list = card.querySelector('.mic-list');
  const search = card.querySelector('.mic-search');
  const errBox = card.querySelector('.mic-error');
  let profiles = [];
  let busy = false;

  async function save(choice) {
    if (busy) return;
    busy = true;
    errBox.innerHTML = '';
    card.querySelectorAll('button').forEach((b) => { b.disabled = true; });
    try {
      const r = await api.setMicBehind(mic.id, choice);
      mic.mic_choice = r.mic_choice;
      mic.profile = r.profile;
      mic.needs_mic_choice = r.needs_mic_choice;
      forgetOldResults();
      if (onDone) await onDone(mic);
    } catch (e) {
      // Never leave a dead card: say what happened and let them try again.
      card.querySelectorAll('button').forEach((b) => { b.disabled = false; });
      errBox.innerHTML = '';
      errBox.append(errorBanner(e, () => save(choice)));
    } finally {
      busy = false;
    }
  }

  function draw() {
    const q = search.value.trim().toLowerCase();
    const words = q.split(/\s+/).filter(Boolean);
    const hits = profiles.filter((p) => {
      const hay = `${p.brand} ${p.model} ${(p.aliases || []).join(' ')}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
    list.innerHTML = '';
    if (!hits.length) {
      list.append(html('<p class="muted" style="grid-column:1/-1">Nothing matches. Try fewer letters, or pick "Another mic".</p>'));
      return;
    }
    const current = mic.mic_choice?.profile_id;
    for (const p of hits) {
      const b = html(`<button type="button" class="choice ${p.id === current ? 'selected' : ''}" aria-pressed="${p.id === current}">${esc(p.display_name)}</button>`);
      b.addEventListener('click', () => save({ kind: 'profile', profile_id: p.id }));
      list.append(b);
    }
  }

  async function init() {
    list.innerHTML = '';
    list.append(html('<p class="muted" style="grid-column:1/-1">Loading the list…</p>'));
    try {
      profiles = await loadProfiles();
      draw();
    } catch (e) {
      list.innerHTML = '';
      list.append(errorBanner(e, init));
    }
  }

  search.addEventListener('input', draw);
  card.querySelector('[data-kind="other"]').addEventListener('click', () => save({ kind: 'other' }));
  card.querySelector('[data-kind="skipped"]').addEventListener('click', () => save({ kind: 'skipped' }));
  card.querySelector('[data-kind="cancel"]')?.addEventListener('click', () => onCancel());
  init();
  return card;
}

/**
 * Show the picker in `slot` (replacing what is there). Returns a function
 * that removes it. `required` = no Cancel (the first, unanswered ask).
 */
export function openMicChoice(slot, mic, { required = false, onDone } = {}) {
  slot.innerHTML = '';
  const close = () => { slot.innerHTML = ''; };
  slot.append(micChoiceCard(mic, {
    onCancel: required ? null : close,
    onDone: async (m) => {
      close();
      toast('Got it', m.mic_choice?.status === 'chosen' ? `We'll tune for your ${m.mic_choice.display_name}.` : 'Okay, we will go by the device name.', 'good');
      if (onDone) await onDone(m);
    },
  }));
  slot.firstElementChild?.scrollIntoView?.({ block: 'nearest' });
  return close;
}
