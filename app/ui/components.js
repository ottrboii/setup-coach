// Small UI building blocks shared by the screens.
import { CONFIG } from './config.js';

export function esc(v) {
  return String(v ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

/** Build an element from an HTML string (trusted template, data must be esc()'d). */
export function html(str) {
  const t = document.createElement('template');
  t.innerHTML = str.trim();
  return t.content.firstElementChild;
}

export const icons = {
  check: '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  wrench: '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M14.5 6.5a4 4 0 0 0 5 5l-8.5 8.5a2.1 2.1 0 0 1-3-3L16.5 8.5a4 4 0 0 0-2-2Z" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/></svg>',
  mic: '<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><rect x="9" y="3" width="6" height="11" rx="3" fill="currentColor"/><path d="M6 11a6 6 0 0 0 12 0M12 17v4" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg>',
  cam: '<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><rect x="3" y="6" width="13" height="12" rx="3" fill="none" stroke="currentColor" stroke-width="2"/><path d="M16 10l5-3v10l-5-3z" fill="currentColor"/></svg>',
  play: '<svg viewBox="0 0 24 24" width="30" height="30" aria-hidden="true"><path d="M8 5.5v13l11-6.5z" fill="currentColor"/></svg>',
  stop: '<svg viewBox="0 0 24 24" width="28" height="28" aria-hidden="true"><rect x="6.5" y="6.5" width="11" height="11" rx="2" fill="currentColor"/></svg>',
  spark: '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z" fill="currentColor"/></svg>',
  bag: '<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path d="M6 8h12l-1 12H7L6 8Z M9 8V6a3 3 0 0 1 6 0v2" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/></svg>',
  wifi: '<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path d="M3 9a14 14 0 0 1 18 0M6 12.5a9 9 0 0 1 12 0M9 16a4 4 0 0 1 6 0" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><circle cx="12" cy="19" r="1.4" fill="currentColor"/></svg>',
};

const SEVERITY_LABEL = { critical: 'Fix now', major: 'Important', minor: 'Worth fixing', info: 'Handled' };
const FIX_LABEL = { auto: 'We can fix this', guided: "We'll guide you", manual: 'Quick tweak by you' };

/** Circular 0–100 score. */
export function scoreRing(value, { size = 'lg', label = 'score' } = {}) {
  const r = 52;
  const c = 2 * Math.PI * r;
  const v = Math.max(0, Math.min(100, Math.round(value ?? 0)));
  const color = v >= 85 ? 'var(--good)' : v >= 65 ? 'var(--warn)' : 'var(--bad)';
  const px = size === 'sm' ? 76 : 132;
  return html(`
    <div class="score-ring ${size === 'sm' ? 'sm' : ''}" role="img" aria-label="${esc(label)} ${v} out of 100">
      <svg viewBox="0 0 120 120" width="${px}" height="${px}">
        <circle class="track" cx="60" cy="60" r="${r}" fill="none" stroke-width="10"/>
        <circle class="value" cx="60" cy="60" r="${r}" fill="none" stroke="${color}" stroke-width="10" stroke-linecap="round"
          stroke-dasharray="${c.toFixed(1)}" stroke-dashoffset="${(c * (1 - v / 100)).toFixed(1)}"/>
      </svg>
      <div class="num"><div><b>${v}</b><span>${esc(label)}</span></div></div>
    </div>`);
}

export function scoreWord(v) {
  if (v >= 90) return 'Sounding great';
  if (v >= 75) return 'Pretty good';
  if (v >= 55) return 'Room to improve';
  return 'Needs some love';
}

// ---------------------------------------------------------------- visuals
const mouth = (x, y) => `<g><circle cx="${x}" cy="${y}" r="22" fill="var(--surface)" stroke="var(--text-3)" stroke-width="2"/><path d="M${x + 12} ${y + 6} q7 0 8 -4" stroke="var(--text-2)" stroke-width="2.4" fill="none" stroke-linecap="round"/><circle cx="${x + 8}" cy="${y - 7}" r="2.6" fill="var(--text-2)"/></g>`;
const person = (x, y) => `<g><circle cx="${x}" cy="${y}" r="16" fill="var(--surface)" stroke="var(--text-3)" stroke-width="2"/><path d="M${x - 22} ${y + 44} a22 20 0 0 1 44 0" fill="var(--surface)" stroke="var(--text-3)" stroke-width="2"/></g>`;
const micShape = (x, y, rot = 0) => `<g transform="translate(${x} ${y}) rotate(${rot})"><rect x="-9" y="-22" width="18" height="34" rx="9" fill="var(--accent)"/><rect x="-2" y="12" width="4" height="14" fill="var(--text-3)"/></g>`;

const VISUALS = {
  distance: {
    svg: `<svg viewBox="0 0 180 90" width="180" height="90" aria-hidden="true">${mouth(40, 45)}${micShape(120, 45, -90)}
      <path d="M64 70 H100" stroke="var(--good)" stroke-width="3" stroke-linecap="round"/><path d="M64 64v12M100 64v12" stroke="var(--good)" stroke-width="3" stroke-linecap="round"/>
      <text x="82" y="88" text-anchor="middle" font-size="11" fill="var(--text-2)">a fist away</text></svg>`,
    text: 'Close is good: about a fist (3–10 cm) from your mouth for most streaming mics.',
  },
  angle: {
    svg: `<svg viewBox="0 0 180 90" width="180" height="90" aria-hidden="true">${mouth(40, 45)}${micShape(118, 28, -60)}
      <path d="M62 45 L150 45" stroke="var(--text-3)" stroke-dasharray="4 4"/><path d="M62 45 L140 12" stroke="var(--good)" stroke-width="2.5"/>
      <text x="128" y="80" text-anchor="middle" font-size="11" fill="var(--text-2)">talk past it</text></svg>`,
    text: 'Angle the mic a little to the side so puffs of air on P, B and S go past it.',
  },
  side: {
    svg: `<svg viewBox="0 0 180 90" width="180" height="90" aria-hidden="true">${mouth(40, 45)}
      <rect x="110" y="18" width="26" height="56" rx="13" fill="var(--accent)"/><text x="123" y="50" text-anchor="middle" font-size="9" font-weight="700" fill="var(--accent-ink)">LOGO</text>
      <path d="M64 45 H104" stroke="var(--good)" stroke-width="3" marker-end="url(#a)"/><defs><marker id="a" markerWidth="8" markerHeight="8" refX="6" refY="4" orient="auto"><path d="M0 0L8 4L0 8z" fill="var(--good)"/></marker></defs>
      <text x="123" y="88" text-anchor="middle" font-size="11" fill="var(--text-2)">talk into the side</text></svg>`,
    text: 'This mic listens from the side with the logo, not from the top.',
  },
  boom: {
    svg: `<svg viewBox="0 0 180 90" width="180" height="90" aria-hidden="true">${mouth(46, 45)}
      <rect x="10" y="80" width="160" height="6" rx="3" fill="var(--surface-3)"/><path d="M160 80 V30 L110 14 L84 38" stroke="var(--text-3)" stroke-width="4" fill="none" stroke-linecap="round" stroke-linejoin="round"/>${micShape(80, 44, -110)}</svg>`,
    text: 'A boom arm keeps the mic in front of your mouth and off the desk — no thumps, no reflections.',
  },
  window: {
    svg: `<svg viewBox="0 0 180 90" width="180" height="90" aria-hidden="true"><rect x="96" y="8" width="70" height="58" rx="4" fill="#fff6d8" stroke="var(--border)"/><path d="M131 8v58M96 37h70" stroke="var(--border)"/>
      ${person(66, 40)}
      <path d="M20 22 L44 32" stroke="var(--good)" stroke-width="3" stroke-linecap="round"/><circle cx="16" cy="20" r="8" fill="var(--warn)"/>
      <text x="131" y="82" text-anchor="middle" font-size="11" fill="var(--text-2)">window to the side</text></svg>`,
    text: "Bright window behind you? Turn so it's beside or in front of you, or close the blinds.",
  },
  key: {
    svg: `<svg viewBox="0 0 180 90" width="180" height="90" aria-hidden="true">${person(90, 38)}
      <rect x="18" y="10" width="34" height="26" rx="5" fill="var(--warn)"/><path d="M52 28 L72 38" stroke="var(--warn)" stroke-width="3" stroke-dasharray="3 4"/>
      <text x="35" y="54" text-anchor="middle" font-size="10" fill="var(--text-2)">45°, above</text><rect x="140" y="20" width="24" height="18" rx="3" fill="var(--text-3)"/><text x="152" y="54" text-anchor="middle" font-size="10" fill="var(--text-2)">camera</text></svg>`,
    text: 'Put a soft light in front of you, a little to one side and slightly above your eyes.',
  },
  ring: {
    svg: `<svg viewBox="0 0 180 90" width="180" height="90" aria-hidden="true"><circle cx="90" cy="45" r="30" fill="none" stroke="var(--warn)" stroke-width="7"/><rect x="83" y="40" width="14" height="10" rx="2" fill="var(--text-3)"/>
      <path d="M128 18 L150 8" stroke="var(--good)" stroke-width="3" stroke-linecap="round"/><text x="140" y="84" text-anchor="middle" font-size="11" fill="var(--text-2)">raise &amp; tilt</text></svg>`,
    text: 'Glasses glare? Raise the light and tilt it down, or move it 30–45° to the side.',
  },
  eye: {
    svg: `<svg viewBox="0 0 180 90" width="180" height="90" aria-hidden="true">${person(50, 36)}
      <path d="M68 36 H128" stroke="var(--good)" stroke-width="2.5" stroke-dasharray="5 4"/><rect x="130" y="28" width="30" height="16" rx="4" fill="var(--accent)"/><rect x="141" y="44" width="8" height="40" fill="var(--text-3)"/>
      <text x="100" y="26" text-anchor="middle" font-size="11" fill="var(--text-2)">eye level</text></svg>`,
    text: 'Raise the camera to eye level — a stack of books works great.',
  },
};

const VISUAL_FOR = {
  'mic.too_far': 'distance', 'mic.thin': 'distance', 'mic.too_quiet': 'distance', 'mic.keyboard': 'distance', 'mic.background_noise': 'distance', 'mic.not_enough_gain': 'distance',
  'mic.plosives': 'angle', 'mic.sibilance': 'angle', 'mic.harsh': 'angle',
  'mic.wrong_side': 'side',
  'mic.reflection': 'boom', 'mic.desk_thumps': 'boom', 'mic.level_swings': 'boom',
  'camera.backlit': 'window', 'camera.background_competes': 'window',
  'camera.harsh_shadows': 'key', 'camera.face_needs_light': 'key', 'camera.too_dark_grainy': 'key', 'camera.underexposed': 'key',
  'camera.glare': 'ring',
  'camera.camera_low': 'eye',
};

export function visualFor(id) {
  const v = VISUALS[VISUAL_FOR[id]];
  if (!v) return null;
  return html(`<div class="visual">${v.svg}<p class="muted">${esc(v.text)}</p></div>`);
}

/** Card for one finding. opts: { selectable, checked, onToggle, onDone, onCant, fixed, visual } */
export function findingCard(f, opts = {}) {
  const sev = f.severity || 'minor';
  const node = html(`
    <article class="finding ${esc(sev)}" data-id="${esc(f.id)}">
      <div class="bar"></div>
      <div class="body">
        <div class="head">
          <div class="title">${esc(f.title)}</div>
          <div class="row" style="gap:6px">
            ${opts.fixed ? '<span class="chip ok">Fixed ✓</span>' : `<span class="chip ${esc(sev)}">${SEVERITY_LABEL[sev] || sev}</span>`}
            ${f.fix_kind === 'auto' && !opts.fixed && sev !== 'info' ? `<span class="chip auto">${FIX_LABEL.auto}</span>` : ''}
          </div>
        </div>
        <p class="why">${esc(f.why)}</p>
        <div class="fix"><span class="ico">${sev === 'info' ? icons.check : icons.wrench}</span><span>${esc(f.fix)}</span></div>
        <div class="slot"></div>
        <div class="actions"></div>
      </div>
    </article>`);
  const slot = node.querySelector('.slot');
  if (opts.visual !== false && sev !== 'info') {
    const v = visualFor(f.id);
    if (v) slot.append(v);
  }
  const actions = node.querySelector('.actions');
  if (opts.selectable) {
    const lab = html(`<label class="pick"><input type="checkbox" ${opts.checked ? 'checked' : ''}/> Include in "fix for me"</label>`);
    lab.querySelector('input').addEventListener('change', (e) => opts.onToggle && opts.onToggle(e.target.checked));
    actions.append(lab);
  }
  if (f.can_decline && sev !== 'info' && (opts.onDone || opts.onCant)) {
    if (opts.onDone) {
      const b = html('<button class="btn sm primary">Done — retest</button>');
      b.addEventListener('click', () => opts.onDone(f));
      actions.append(b);
    }
    if (opts.onCant) {
      const b = html('<button class="btn sm">I can\'t do this</button>');
      b.addEventListener('click', () => opts.onCant(f, b));
      actions.append(b);
    }
  }
  if (!actions.children.length) actions.remove();
  return node;
}

/** Soft "optional upgrade" card for a product category. */
export function upgradeCard(product) {
  const p = CONFIG.products[product];
  if (!p) return null;
  const link = CONFIG.affiliateEnabled
    ? `<a class="btn sm" href="${esc(CONFIG.affiliateBaseUrl + product)}" target="_blank" rel="noopener sponsored">See options</a>`
    : '';
  return html(`
    <div class="upgrade">
      <div class="ico">${icons.bag}</div>
      <div class="stack" style="gap:2px;flex:1">
        <div><b>Optional upgrade:</b> ${esc(p.name)}</div>
        <div class="subtle">${esc(p.blurb)} Not required — your settings already work around it.</div>
      </div>
      ${link}
    </div>`);
}

export function toast(title, body = '', kind = '') {
  const box = document.getElementById('toasts');
  const t = html(`<div class="toast ${esc(kind)}" role="status"><b>${esc(title)}</b>${body ? `<span class="muted">${esc(body)}</span>` : ''}</div>`);
  box.append(t);
  setTimeout(() => t.remove(), 6000);
}

export function loading(text) {
  return html(`<div class="loading"><div class="spinner"></div><span>${esc(text)}</span></div>`);
}

export function errorBanner(err, retry) {
  const node = html(`<div class="banner bad"><div class="stack" style="gap:8px"><b>That didn't work</b><span>${esc(err && err.message ? err.message : err)}</span></div></div>`);
  if (retry) {
    const b = html('<button class="btn sm" style="margin-left:auto">Try again</button>');
    b.addEventListener('click', retry);
    node.append(b);
  }
  return node;
}

/** Collapsible "Advanced" drawer with numbers for curious users. */
export function advanced(title, inner) {
  const d = html(`<details class="advanced"><summary>${esc(title)}</summary><div class="inner"></div></details>`);
  d.querySelector('.inner').append(inner);
  return d;
}

export function kvTable(rows) {
  return html(`<table class="kv"><tbody>${rows.map(([k, v]) => `<tr><th>${esc(k)}</th><td>${esc(v)}</td></tr>`).join('')}</tbody></table>`);
}

export function fmt(v, digits = 1, unit = '') {
  if (v === null || v === undefined || Number.isNaN(v)) return '—';
  if (typeof v !== 'number') return String(v);
  return `${v.toFixed(digits)}${unit}`;
}
