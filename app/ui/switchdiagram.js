// Self-drawn, generic switch diagram: one small picture per position.
// Deliberately generic (a slider with N stops, knob at one stop). It never shows where the
// switch sits on a mic body, because that is not sourced. Plain words go in the captions.
import { esc } from './components.js';

/** SVG for a slider with `n` stops and the knob at stop `i` (0-based). */
export function switchSvg(n, i, label) {
  const W = 120, H = 56, pad = 22, y = 28;
  const step = n > 1 ? (W - pad * 2) / (n - 1) : 0;
  const xs = Array.from({ length: n }, (_, k) => pad + k * step);
  const stops = xs.map((x, k) => (k === i ? '' : `<circle cx="${x}" cy="${y}" r="3.5" class="sd-stop"/>`)).join('');
  const alt = `Drawing of a switch with the knob at position ${i + 1} of ${n}: ${label}`;
  return `<svg class="sd-svg" viewBox="0 0 ${W} ${H}" role="img" aria-label="${esc(alt)}" focusable="false">
    <rect x="8" y="14" width="${W - 16}" height="28" rx="14" class="sd-track"/>
    ${stops}
    <g class="sd-knob" style="transform:translateX(${xs[i]}px)">
      <circle cx="0" cy="${y}" r="11" class="sd-knob-c"/>
      <path d="M-4 ${y - 3}h8M-4 ${y}h8M-4 ${y + 3}h8" class="sd-grip"/>
    </g>
  </svg>`;
}

/** Row of figures, one per position. Returns an HTML string; each figure has data-label. */
export function switchDiagramHtml(positions) {
  const ps = (positions || []).filter((p) => p && p.label);
  if (ps.length < 2 || ps.length > 6) return '';
  const figs = ps.map((p, i) => `
    <figure class="sd-fig" data-label="${esc(p.label)}">
      ${switchSvg(ps.length, i, p.label)}
      <figcaption><b>${esc(p.label)}</b><span>${esc(p.meaning || '')}</span></figcaption>
    </figure>`).join('');
  return `<div class="sd" role="group" aria-label="The switch in each position">${figs}
    <p class="subtle sd-note">Simple drawings to help you match what you see. Your switch may look different or slide another way.</p></div>`;
}
