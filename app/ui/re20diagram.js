// Model-specific drawing for the EV RE20 bass-tilt switch. Self-drawn and simplified from the
// owner's own photo (not shipped): a small vertical slide switch in a dark plate on the side of the
// body, between the label and the mount collar; slanted line at the top = bass cut, flat line at
// the bottom = flat. i = 0 is the normal (flat) position, i = 1 is bass cut.
import { esc } from './components.js';

export function re20Svg(i) {
  const cut = i === 1;
  const alt = 'Drawing of the side of an RE20 microphone with the bass tilt switch enlarged. The knob is '
    + (cut ? 'up, next to the slanted line (thinner sound).' : 'down, next to the flat line (normal sound).');
  const ky = cut ? 46 : 70;
  return `<svg class="sd-svg sd-re20" viewBox="0 0 220 130" role="img" aria-label="${esc(alt)}" focusable="false">
    <rect x="6" y="32" width="34" height="54" rx="12" class="re-grille"/>
    <path d="M16 38v42M24 38v42M32 38v42" class="re-mesh"/>
    <rect x="34" y="30" width="84" height="58" rx="16" class="re-body"/>
    <text x="50" y="63" class="re-text">RE20</text>
    <rect x="118" y="36" width="24" height="46" rx="5" class="re-collar"/>
    <rect x="96" y="48" width="9" height="22" rx="3" class="re-plate"/>
    <path d="M105 48L150 12M105 70L150 118" class="re-lead"/>
    <rect x="150" y="8" width="60" height="114" rx="12" class="re-plate"/>
    <path d="M168 36l24-14" class="re-icon"/>
    <path d="M168 104h24" class="re-icon"/>
    <rect x="173" y="44" width="14" height="42" rx="7" class="re-slot"/>
    <g class="re-knob" style="transform:translateY(${ky}px)"><rect x="169" y="0" width="22" height="16" rx="5"/><path d="M174 8h12"/></g>
  </svg>`;
}
