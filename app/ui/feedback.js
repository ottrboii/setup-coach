// "Tell us what's wrong" notes panel. Lives outside #main, so opening it never
// re-renders or interrupts the screen underneath (mic test, camera preview,
// apply, recording all keep running). The draft is kept in memory and in
// localStorage, so closing, reopening or restarting never loses text.
import { api } from './api.js';
import { esc, html, toast } from './components.js';
import { state, selectedMic, selectedCamera } from './state.js';

const DRAFT_KEY = 'setupcoach.noteDraft';
const TAGS = [
  { id: 'confusing', label: 'Confusing' },
  { id: 'broken', label: 'Something broke' },
  { id: 'looks_wrong', label: 'Looks wrong' },
  { id: 'idea', label: 'I have an idea' },
];
const SCREEN_NAMES = {
  welcome: 'Welcome', scan: 'Quick scan', mic: 'Microphone test', 'mic-report': 'Microphone results', 'mic-apply': 'Microphone: applying',
  camera: 'Camera', health: 'Stream health', home: 'Home', coach: 'Coaching', backups: 'Backups & undo', ab: 'Before / after', 'ab-record': 'Before / after recording',
};

let draft = { text: '', tag: null };
try { draft = { ...draft, ...JSON.parse(localStorage.getItem(DRAFT_KEY) || '{}') }; } catch (_) { /* storage may be unavailable */ }
let panel = null;
let opener = null;
let lastFocus = null;

function saveDraft() {
  try { localStorage.setItem(DRAFT_KEY, JSON.stringify(draft)); } catch (_) { /* fine */ }
}

export function currentRoute() {
  return location.hash.replace(/^#\/?/, '').split(/[?/]/)[0] || 'welcome';
}
export function screenName() {
  const r = currentRoute();
  return SCREEN_NAMES[r] || r;
}

export function isOpen() { return !!panel; }

export function close() {
  if (!panel) return;
  draft.text = panel.querySelector('textarea').value;
  saveDraft();
  panel.remove();
  panel = null;
  updateBadge();
  const back = lastFocus && document.contains(lastFocus) ? lastFocus : opener;
  back?.focus?.({ preventScroll: true });
}

export function open() {
  if (panel) { panel.querySelector('textarea').focus(); return; }
  lastFocus = document.activeElement;
  panel = html(`
    <section class="notes-panel" role="dialog" aria-modal="false" aria-labelledby="notes-title">
      <div class="notes-head">
        <h2 id="notes-title">Tell us what's wrong</h2>
        <button type="button" class="btn sm ghost" data-close aria-label="Close notes. Your words are kept.">Close</button>
      </div>
      <p class="muted notes-sub">Whatever you're doing keeps going. Write it in your own words. Your words stay on this computer.</p>
      <div class="notes-screen muted">About: <b data-screen></b></div>
      <div class="notes-tags" role="group" aria-label="What kind of note is this? (optional)">
        ${TAGS.map((t) => `<button type="button" class="chip" data-tag="${t.id}" aria-pressed="false">${esc(t.label)}</button>`).join('')}
      </div>
      <label class="notes-label" for="notes-text">What happened, or what did you expect?</label>
      <textarea id="notes-text" rows="7" placeholder="For example: I pressed Start and nothing moved."></textarea>
      <div class="notes-actions">
        <button type="button" class="btn primary lg" data-save>Save note</button>
        <span class="muted" data-status role="status"></span>
      </div>
      <details class="notes-send">
        <summary>Send my notes to Otto</summary>
        <p class="muted">Nothing is sent automatically. Copy your notes and paste them into a message, or save them as a file on your Desktop.</p>
        <div class="notes-actions">
          <button type="button" class="btn" data-copy>Copy my notes</button>
          <button type="button" class="btn" data-desktop>Save notes to Desktop</button>
        </div>
        <p class="muted" data-sendstatus role="status"></p>
      </details>
    </section>`);
  const ta = panel.querySelector('textarea');
  const status = panel.querySelector('[data-status]');
  const sendStatus = panel.querySelector('[data-sendstatus]');
  ta.value = draft.text || '';
  panel.querySelector('[data-screen]').textContent = screenName();
  const paintTags = () => panel.querySelectorAll('[data-tag]').forEach((b) => {
    const on = b.dataset.tag === draft.tag;
    b.classList.toggle('on', on);
    b.setAttribute('aria-pressed', String(on));
  });
  paintTags();
  ta.addEventListener('input', () => { draft.text = ta.value; saveDraft(); status.textContent = ''; updateBadge(); });
  panel.addEventListener('click', async (ev) => {
    const t = ev.target.closest('button');
    if (!t) return;
    if (t.dataset.close !== undefined) close();
    else if (t.dataset.tag) { draft.tag = draft.tag === t.dataset.tag ? null : t.dataset.tag; saveDraft(); paintTags(); }
    else if (t.dataset.save !== undefined) {
      const text = ta.value.trim();
      if (!text) { status.textContent = 'Write a few words first.'; ta.focus(); return; }
      t.disabled = true;
      try {
        const res = await api.saveFeedback({ text, route: currentRoute(), tag: draft.tag, mic_name: selectedMic()?.name || null, camera_name: selectedCamera()?.name || null });
        draft = { text: '', tag: null };
        saveDraft();
        ta.value = '';
        paintTags();
        updateBadge();
        status.textContent = `Saved. Thank you! (${res.count} saved so far)`;
        toast('Note saved', 'Thank you. It is kept on this computer.', 'good');
      } catch (e) {
        status.textContent = `${e.message} Your words are still here.`;
      } finally { t.disabled = false; }
    } else if (t.dataset.copy !== undefined) {
      try {
        const res = await api.getFeedback();
        if (!res.count) { sendStatus.textContent = 'There are no saved notes yet.'; return; }
        await copyText(res.text);
        sendStatus.textContent = `Copied ${res.count} note${res.count === 1 ? '' : 's'}. Now paste them into a message.`;
      } catch (e) { sendStatus.textContent = `${e.message} Try "Save notes to Desktop" instead.`; }
    } else if (t.dataset.desktop !== undefined) {
      try {
        const where = await api.saveFeedbackToDesktop();
        sendStatus.textContent = `Saved to your Desktop as "${String(where).split(/[\\/]/).pop()}".`;
      } catch (e) { sendStatus.textContent = e.message; }
    }
  });
  panel.addEventListener('keydown', (ev) => { if (ev.key === 'Escape') { ev.stopPropagation(); close(); } });
  document.body.append(panel);
  updateBadge();
  ta.focus();
  ta.setSelectionRange(ta.value.length, ta.value.length);
}

async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return; } catch (_) { /* fall back below */ }
  const tmp = document.createElement('textarea');
  tmp.value = text;
  tmp.style.cssText = 'position:fixed;opacity:0;top:0;left:0';
  document.body.append(tmp);
  tmp.select();
  const ok = document.execCommand && document.execCommand('copy');
  tmp.remove();
  if (!ok) throw new Error("Couldn't copy.");
}

function updateBadge() {
  const hasDraft = !!(panel ? panel.querySelector('textarea').value.trim() : (draft.text || '').trim());
  opener?.classList.toggle('has-draft', hasDraft && !panel);
  const hint = opener?.querySelector('[data-draft]');
  if (hint) hint.hidden = !(hasDraft && !panel);
}

/** Wire the sidebar button and the keyboard shortcut (Ctrl+Shift+F, or Ctrl+Alt+N). */
export function initFeedback() {
  opener = document.getElementById('notes-btn');
  opener?.addEventListener('click', () => (panel ? close() : open()));
  window.addEventListener('keydown', (ev) => {
    const k = ev.key.toLowerCase();
    if (ev.ctrlKey && ((ev.shiftKey && k === 'f') || (ev.altKey && k === 'n'))) {
      ev.preventDefault();
      panel ? close() : open();
    }
  });
  window.addEventListener('hashchange', () => {
    // The label follows the screen while the panel is open; the text stays.
    const s = panel?.querySelector('[data-screen]');
    if (s) s.textContent = screenName();
  });
  updateBadge();
}
