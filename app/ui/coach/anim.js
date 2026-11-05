// Live, animated coaching illustrations (inline SVG + requestAnimationFrame).
// Each illustration reacts to `coach-update`s: the live value, the hint
// (kind + amount), the phase and whether the app is adjusting by itself.
// prefers-reduced-motion: no continuous motion — states snap and text carries it.

const NS = 'http://www.w3.org/2000/svg';
export const reducedMotion = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

function el(tag, attrs = {}, parent) {
  const n = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, v);
  if (parent) parent.append(n);
  return n;
}
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const lerp = (a, b, k) => a + (b - a) * k;
/** Smoothly approach a target (frame-rate independent). */
const approach = (cur, target, dt, speed = 6) => lerp(cur, target, 1 - Math.exp(-speed * dt));

/** Map a value into 0..1 inside [lo, hi]. */
export function norm(v, lo, hi) {
  if (v === null || v === undefined || Number.isNaN(v) || hi === lo) return 0.5;
  return clamp((v - lo) / (hi - lo), 0, 1);
}

/** Gauge domain for an update: backend target ± margin, else the task default. */
export function domainFor(u, fallback) {
  const t = u && u.target;
  if (t && Number.isFinite(t.min) && Number.isFinite(t.max) && t.max > t.min) {
    const span = t.max - t.min;
    const lo = Math.min(t.min - span * 1.5, fallback ? fallback[0] : -Infinity);
    const hi = Math.max(t.max + span * 1.5, fallback ? fallback[1] : Infinity);
    return [Number.isFinite(lo) ? lo : t.min - span * 1.5, Number.isFinite(hi) ? hi : t.max + span * 1.5];
  }
  return fallback || [0, 1];
}

// --------------------------------------------------------------- shared art
function face(g, x, y, r = 30) {
  const grp = el('g', { class: 'ill-face' }, g);
  el('circle', { cx: x, cy: y, r, class: 'ill-skin' }, grp);
  el('circle', { cx: x + r * 0.38, cy: y - r * 0.25, r: r * 0.09, class: 'ill-ink' }, grp);
  el('path', { d: `M${x + r * 0.45} ${y + r * 0.25} q${r * 0.25} 0 ${r * 0.3} -${r * 0.15}`, class: 'ill-line' }, grp);
  return grp;
}
function micSide(g) {
  // Side view of a mic pointing left (capsule end at x=0).
  const grp = el('g', {}, g);
  el('rect', { x: -6, y: -15, width: 54, height: 30, rx: 15, class: 'ill-mic' }, grp);
  el('rect', { x: -6, y: -15, width: 20, height: 30, rx: 15, class: 'ill-grille' }, grp);
  el('rect', { x: 46, y: -4, width: 34, height: 8, rx: 3, class: 'ill-stand' }, grp);
  return grp;
}
function arrow(g, cls = 'ill-arrow') {
  const grp = el('g', { class: cls }, g);
  el('path', { d: 'M-26 0 H18', class: 'shaft' }, grp);
  el('path', { d: 'M8 -10 L22 0 L8 10', class: 'head' }, grp);
  return grp;
}

// ---------------------------------------------------------- illustrations
const KINDS = {};

/** Gain knob: rotates with the live level; arrow shows which way to turn. */
KINDS.knob = (svg) => {
  const cx = 160, cy = 100, R = 62;
  const toAngle = (p) => -135 + p * 270;
  const arcPath = (a0, a1, r) => {
    const p = (a) => [cx + r * Math.sin((a * Math.PI) / 180), cy - r * Math.cos((a * Math.PI) / 180)];
    const [x0, y0] = p(a0); const [x1, y1] = p(a1);
    return `M${x0} ${y0} A${r} ${r} 0 ${a1 - a0 > 180 ? 1 : 0} 1 ${x1} ${y1}`;
  };
  el('path', { d: arcPath(-135, 135, R + 16), class: 'ill-track' }, svg);
  const zone = el('path', { class: 'ill-zone' }, svg);
  const level = el('path', { class: 'ill-level' }, svg);
  const body = el('g', {}, svg);
  el('circle', { cx, cy, r: R, class: 'ill-knob' }, body);
  for (let i = 0; i < 24; i++) el('rect', { x: cx - 1.5, y: cy - R + 3, width: 3, height: 7, rx: 1.5, class: 'ill-knurl', transform: `rotate(${i * 15} ${cx} ${cy})` }, body);
  el('circle', { cx, cy, r: R - 14, class: 'ill-knob-cap' }, body);
  el('rect', { x: cx - 3, y: cy - R + 14, width: 6, height: 24, rx: 3, class: 'ill-pointer' }, body);
  const turn = el('g', { class: 'ill-turn' }, svg);
  const turnPath = el('path', { class: 'ill-turn-path' }, turn);
  const turnHead = el('path', { class: 'ill-turn-head' }, turn);
  const gear = el('g', { class: 'ill-gear hidden' }, svg);
  el('circle', { cx: 270, cy: 34, r: 14, class: 'ill-gear-c' }, gear);
  for (let i = 0; i < 8; i++) el('rect', { x: 267, y: 15, width: 6, height: 8, rx: 1.5, class: 'ill-gear-c', transform: `rotate(${i * 45} 270 34)` }, gear);
  el('text', { x: 160, y: 196, 'text-anchor': 'middle', class: 'ill-caption' }, svg).textContent = 'your gain knob';
  const s = { pos: 0.3, target: 0.3, zone: [0.6, 0.75], dir: 0, amount: 0, auto: false, t: 0, gearRot: 0, done: false };
  return {
    update(u, dom) {
      s.target = norm(u.value, dom[0], dom[1]);
      if (u.target) s.zone = [norm(u.target.min, dom[0], dom[1]), norm(u.target.max, dom[0], dom[1])];
      const k = u.hint?.kind;
      s.dir = k === 'turn_up' ? 1 : k === 'turn_down' ? -1 : 0;
      s.amount = clamp(u.hint?.amount ?? 0.5, 0, 1);
      s.auto = !!u.auto_adjusting;
      s.done = u.phase === 'done' || u.phase === 'hold';
    },
    frame(dt) {
      s.t += dt;
      s.pos = approach(s.pos, s.target, dt, 5);
      zone.setAttribute('d', arcPath(toAngle(s.zone[0]), toAngle(s.zone[1]), R + 16));
      level.setAttribute('d', arcPath(-135, Math.max(-134, toAngle(s.pos)), R + 16));
      level.dataset.zone = s.pos >= s.zone[0] && s.pos <= s.zone[1] ? 'good' : s.pos > s.zone[1] ? 'hot' : 'low';
      body.setAttribute('transform', `rotate(${toAngle(s.pos)} ${cx} ${cy})`);
      // Turn arrow arcs around the knob in the needed direction; dashes flow faster for bigger moves.
      const show = s.dir !== 0 && !s.done;
      turn.style.opacity = show ? 1 : 0;
      if (show) {
        const a0 = toAngle(s.pos);
        const a1 = clamp(a0 + s.dir * (35 + s.amount * 45), -135, 135);
        const [from, to] = s.dir > 0 ? [a0, a1] : [a1, a0];
        turnPath.setAttribute('d', arcPath(from + 2, to - 2, R + 34));
        turnPath.style.strokeDashoffset = String((-s.dir * s.t * (40 + s.amount * 120)) % 1000);
        const tipA = (s.dir > 0 ? to : from) * Math.PI / 180;
        const tx = cx + (R + 34) * Math.sin(tipA), ty = cy - (R + 34) * Math.cos(tipA);
        const rot = (s.dir > 0 ? to : from) + (s.dir > 0 ? 90 : -90);
        turnHead.setAttribute('d', 'M-7 -8 L7 0 L-7 8 Z');
        turnHead.setAttribute('transform', `translate(${tx} ${ty}) rotate(${rot})`);
      }
      gear.classList.toggle('hidden', !s.auto);
      if (s.auto) { s.gearRot += dt * 120; gear.setAttribute('transform', `rotate(${s.gearRot} 270 34)`); }
    },
  };
};

/** Face + mic: the mic slides with the live distance; green band = sweet spot. */
KINDS.mic_distance = (svg) => {
  const X0 = 96, X1 = 300; // mic x for distance 0 .. max
  el('line', { x1: 60, y1: 150, x2: 304, y2: 150, class: 'ill-ruler' }, svg);
  const band = el('rect', { y: 138, height: 24, rx: 6, class: 'ill-band' }, svg);
  el('text', { y: 182, 'text-anchor': 'middle', class: 'ill-caption band-label' }, svg);
  const bandLabel = svg.lastChild;
  face(svg, 58, 92, 34);
  const waves = el('g', { class: 'ill-waves' }, svg);
  const arcs = [0, 1, 2].map(() => el('path', { class: 'ill-wave' }, waves));
  const mic = el('g', {}, svg);
  micSide(mic);
  const arr = arrow(svg);
  const s = { x: 250, tx: 250, band: [130, 170], dir: 0, amount: 0.5, t: 0, good: false };
  return {
    update(u, dom) {
      s.tx = lerp(X0, X1, norm(u.value, dom[0], dom[1]));
      if (u.target) s.band = [lerp(X0, X1, norm(u.target.min, dom[0], dom[1])), lerp(X0, X1, norm(u.target.max, dom[0], dom[1]))];
      const k = u.hint?.kind;
      s.dir = k === 'closer' ? -1 : k === 'farther' ? 1 : 0;
      s.amount = clamp(u.hint?.amount ?? 0.5, 0, 1);
      s.good = ['hold', 'done'].includes(u.phase);
      bandLabel.textContent = u.target ? `sweet spot` : '';
    },
    frame(dt) {
      s.t += dt;
      s.x = approach(s.x, s.tx, dt, 4);
      band.setAttribute('x', s.band[0]); band.setAttribute('width', Math.max(6, s.band[1] - s.band[0]));
      band.classList.toggle('good', s.good);
      bandLabel.setAttribute('x', (s.band[0] + s.band[1]) / 2);
      mic.setAttribute('transform', `translate(${s.x} 92)`);
      arcs.forEach((a, i) => {
        const ph = ((s.t * 0.9 + i / 3) % 1);
        const x = lerp(96, s.x - 8, ph);
        a.setAttribute('d', `M${x} 76 q8 16 0 32`);
        a.style.opacity = String(0.8 * (1 - ph));
      });
      arr.style.opacity = s.dir && !s.good ? 1 : 0;
      const wob = Math.sin(s.t * 6) * 5 * (0.5 + s.amount);
      arr.setAttribute('transform', `translate(${s.x + 34 + s.dir * wob} 54) scale(${s.dir || 1} 1)`);
    },
  };
};

/** Mic turns off-axis; air puffs from P's either thump into it (red) or fly past (green). */
KINDS.mic_angle = (svg) => {
  face(svg, 58, 100, 34);
  const mic = el('g', {}, svg);
  micSide(mic);
  const puffs = el('g', {}, svg);
  const hits = el('g', {}, svg);
  const label = el('text', { x: 160, y: 190, 'text-anchor': 'middle', class: 'ill-caption' }, svg);
  const s = { prog: 0, tprog: 0, t: 0, spawn: 0, list: [], phase: 'working' };
  return {
    update(u) {
      s.tprog = clamp(u.progress ?? 0, 0, 1);
      s.phase = u.phase;
      label.textContent = s.tprog > 0.75 ? 'air flies past — your voice still goes in' : 'puffs of air hit the mic';
    },
    frame(dt) {
      s.t += dt;
      s.prog = approach(s.prog, s.tprog, dt, 3);
      const ang = s.prog * 38, dy = s.prog * 26;
      mic.setAttribute('transform', `translate(210 ${100 + dy}) rotate(${-ang})`);
      s.spawn -= dt;
      if (s.spawn <= 0 && s.phase !== 'done') { s.spawn = 0.45; s.list.push({ x: 96, y: 100 + (Math.random() - 0.5) * 6, life: 0, hit: false, n: el('circle', { r: 5, class: 'ill-puff' }, puffs) }); }
      s.list = s.list.filter((p) => {
        p.life += dt; p.x += dt * 140;
        const micTop = 100 + dy - 15, micBot = 100 + dy + 15;
        if (!p.hit && p.x >= 204 && p.x <= 214 && p.y > micTop && p.y < micBot) {
          p.hit = true;
          const b = el('circle', { cx: 206, cy: p.y, r: 6, class: 'ill-thud' }, hits);
          setTimeout(() => b.remove(), 450);
        }
        p.n.setAttribute('cx', p.x); p.n.setAttribute('cy', p.y);
        p.n.setAttribute('r', 4 + p.life * 6);
        p.n.classList.toggle('pass', p.y <= micTop || p.y >= micBot);
        p.n.style.opacity = String(Math.max(0, 0.85 - p.life * 0.45));
        if (p.hit || p.x > 320) { p.n.remove(); return false; }
        return true;
      });
    },
  };
};

/** Top-down view: the mic turns until the logo side faces the mouth. */
KINDS.logo_side = (svg) => {
  face(svg, 70, 100, 34);
  el('text', { x: 230, y: 30, 'text-anchor': 'middle', class: 'ill-caption' }, svg).textContent = 'mic seen from above';
  const mic = el('g', {}, svg);
  el('circle', { cx: 0, cy: 0, r: 36, class: 'ill-mic' }, mic);
  el('circle', { cx: 0, cy: 0, r: 28, class: 'ill-grille' }, mic);
  const logo = el('g', {}, mic);
  el('rect', { x: -46, y: -14, width: 22, height: 28, rx: 5, class: 'ill-logo' }, logo);
  el('text', { x: -35, y: 4, 'text-anchor': 'middle', class: 'ill-logo-t' }, logo).textContent = 'LOGO';
  const beam = el('path', { class: 'ill-beam' }, svg);
  const s = { a: 110, ta: 110, t: 0 };
  return {
    update(u) { s.ta = (1 - clamp(u.progress ?? 0, 0, 1)) * 110; },
    frame(dt) {
      s.t += dt;
      s.a = approach(s.a, s.ta, dt, 3);
      mic.setAttribute('transform', `translate(230 104) rotate(${s.a})`);
      const facing = s.a < 18;
      beam.setAttribute('d', 'M108 104 L186 104');
      beam.classList.toggle('good', facing);
      beam.style.strokeDashoffset = String((-s.t * 60) % 100);
    },
  };
};

function nullScene(svg, sourceKind) {
  // Mic in the middle turns its back toward a noise source on the right.
  face(svg, 50, 100, 30);
  const src = el('g', {}, svg);
  if (sourceKind === 'fan') {
    el('circle', { cx: 270, cy: 100, r: 34, class: 'ill-fan-ring' }, src);
    const blades = el('g', {}, src);
    for (let i = 0; i < 3; i++) el('path', { d: 'M270 100 q8 -26 0 -30 q-8 4 0 30', class: 'ill-blade', transform: `rotate(${i * 120} 270 100)` }, blades);
    src.blades = blades;
  } else {
    el('rect', { x: 230, y: 118, width: 84, height: 34, rx: 6, class: 'ill-kb' }, src);
    src.keys = [];
    for (let r = 0; r < 3; r++) for (let c = 0; c < 7; c++) src.keys.push(el('rect', { x: 236 + c * 11, y: 123 + r * 9, width: 8, height: 6, rx: 1.5, class: 'ill-key' }, src));
  }
  const waves = el('g', {}, svg);
  const arcs = [0, 1, 2, 3].map(() => el('path', { class: 'ill-noise' }, waves));
  const mic = el('g', {}, svg);
  micSide(mic);
  el('text', { x: 160, y: 190, 'text-anchor': 'middle', class: 'ill-caption' }, svg).textContent = 'point the back of the mic at the noise';
  const s = { a: 0, ta: 0, level: 1, tlevel: 1, t: 0, rot: 0 };
  return {
    update(u, dom) {
      s.ta = clamp(u.progress ?? 0, 0, 1) * 180;
      s.tlevel = norm(u.value, dom[0], dom[1]);
    },
    frame(dt) {
      s.t += dt;
      s.a = approach(s.a, s.ta, dt, 3);
      s.level = approach(s.level, s.tlevel, dt, 4);
      // Start: capsule aimed at the noise. Progress turns the mic so its capsule
      // faces you and its deaf back faces the noise source.
      mic.setAttribute('transform', `translate(150 100) rotate(${180 - s.a}) translate(-37 0)`);
      if (src.blades) { s.rot += dt * 600; src.blades.setAttribute('transform', `rotate(${s.rot} 270 100)`); }
      if (src.keys) src.keys.forEach((k) => k.classList.toggle('press', Math.random() < 0.06));
      arcs.forEach((a, i) => {
        const ph = (s.t * 0.8 + i / 4) % 1;
        const x = lerp(src.blades ? 232 : 228, 176, ph);
        const h = 14 + 20 * s.level;
        a.setAttribute('d', `M${x} ${100 - h} q-12 ${h} 0 ${h * 2}`);
        a.style.opacity = String((0.15 + 0.85 * s.level) * (1 - ph));
      });
    },
  };
}
KINDS.noise_null = (svg) => nullScene(svg, 'fan');
KINDS.keyboard_null = (svg) => nullScene(svg, 'keyboard');

/** Light around a face: arrow shows where to move it; face shading evens out. */
KINDS.light_move = (svg) => {
  const bg = el('rect', { x: 0, y: 0, width: 320, height: 200, rx: 14, class: 'ill-room' }, svg);
  const win = el('rect', { x: 226, y: 22, width: 70, height: 84, rx: 4, class: 'ill-window' }, svg);
  const head = el('g', {}, svg);
  el('ellipse', { cx: 160, cy: 104, rx: 38, ry: 48, class: 'ill-skin' }, head);
  const shadeL = el('path', { d: 'M160 56 a38 48 0 0 0 0 96 z', class: 'ill-shade' }, head);
  const shadeR = el('path', { d: 'M160 56 a38 48 0 0 1 0 96 z', class: 'ill-shade' }, head);
  el('rect', { x: 110, y: 150, width: 100, height: 50, rx: 30, class: 'ill-body' }, svg);
  const lamp = el('g', {}, svg);
  el('rect', { x: -16, y: -12, width: 32, height: 24, rx: 5, class: 'ill-lamp' }, lamp);
  const glow = el('circle', { cx: 0, cy: 0, r: 30, class: 'ill-glow' }, lamp);
  const arr = arrow(svg);
  const s = { lx: 70, tlx: 70, ly: 46, tly: 46, bright: 0.5, tbright: 0.5, side: 0.6, tside: 0.6, bgDim: 0, tbgDim: 0, dir: null, t: 0, good: false };
  return {
    update(u) {
      const p = clamp(u.progress ?? 0, 0, 1);
      const k = u.hint?.kind;
      s.dir = ['move_light_left', 'move_light_right', 'move_light_up', 'add_light', 'dim_background'].includes(k) ? k : null;
      s.good = ['hold', 'done'].includes(u.phase);
      // Light walks from the far side toward ~40° in front as progress rises.
      s.tlx = lerp(30, 92, p); s.tly = lerp(96, 40, p);
      s.tbright = u.task === 'light_face' ? lerp(0.2, 1, p) : 0.8;
      s.tside = u.task === 'light_direction' ? lerp(0.8, 0.18, p) : lerp(0.5, 0.12, p);
      s.tbgDim = u.task === 'light_face' ? p : 0;
      win.style.display = u.task === 'light_face' ? '' : 'none';
    },
    frame(dt) {
      s.t += dt;
      s.lx = approach(s.lx, s.tlx, dt, 3); s.ly = approach(s.ly, s.tly, dt, 3);
      s.bright = approach(s.bright, s.tbright, dt, 3); s.side = approach(s.side, s.tside, dt, 3);
      s.bgDim = approach(s.bgDim, s.tbgDim, dt, 3);
      lamp.setAttribute('transform', `translate(${s.lx} ${s.ly})`);
      glow.style.opacity = String(0.25 + 0.6 * s.bright);
      shadeL.style.opacity = '0';
      shadeR.style.opacity = String(clamp(s.side, 0, 0.85));
      head.style.filter = `brightness(${0.55 + 0.6 * s.bright})`;
      win.style.opacity = String(1 - 0.6 * s.bgDim);
      arr.style.opacity = s.dir && !s.good ? 1 : 0;
      const wob = Math.sin(s.t * 6) * 5;
      let tr = `translate(${s.lx + 40 + wob} ${s.ly}) scale(-1 1) scale(-1 1)`;
      if (s.dir === 'move_light_left' || s.dir === 'add_light') tr = `translate(${s.lx + 44 + wob} ${s.ly}) rotate(25)`;
      if (s.dir === 'move_light_right') tr = `translate(${s.lx - 40 - wob} ${s.ly}) scale(-1 1)`;
      if (s.dir === 'move_light_up') tr = `translate(${s.lx} ${s.ly + 36 - wob}) rotate(-90)`;
      if (s.dir === 'dim_background') tr = `translate(${200 - wob} 64) scale(-1 1) rotate(0)`;
      arr.setAttribute('transform', tr);
      bg.style.opacity = '1';
    },
  };
};

/** Camera slides up/down to the eye line. */
KINDS.camera_height = (svg) => {
  const head = el('g', {}, svg);
  el('ellipse', { cx: 90, cy: 86, rx: 32, ry: 40, class: 'ill-skin' }, head);
  el('circle', { cx: 104, cy: 78, r: 3, class: 'ill-ink' }, head);
  el('rect', { x: 44, y: 128, width: 92, height: 72, rx: 30, class: 'ill-body' }, svg);
  const eye = el('line', { x1: 108, y1: 78, x2: 300, y2: 78, class: 'ill-eyeline' }, svg);
  const cam = el('g', {}, svg);
  el('rect', { x: -22, y: -12, width: 44, height: 24, rx: 6, class: 'ill-cam' }, cam);
  el('circle', { cx: -22, cy: 0, r: 8, class: 'ill-lens' }, cam);
  el('rect', { x: -4, y: 12, width: 8, height: 200, class: 'ill-stand' }, cam);
  const arr = arrow(svg);
  const s = { y: 140, ty: 140, dir: 0, t: 0, good: false };
  return {
    update(u, dom) {
      s.ty = lerp(170, 0, norm(u.value, dom[0], dom[1])) ;
      const k = u.hint?.kind;
      s.dir = k === 'raise_camera' ? -1 : k === 'lower_camera' ? 1 : 0;
      s.good = ['hold', 'done'].includes(u.phase);
    },
    frame(dt) {
      s.t += dt;
      s.y = approach(s.y, clamp(s.ty, 26, 172), dt, 3);
      cam.setAttribute('transform', `translate(250 ${s.y})`);
      eye.classList.toggle('good', s.good || Math.abs(s.y - 78) < 10);
      arr.style.opacity = s.dir && !s.good ? 1 : 0;
      const wob = Math.sin(s.t * 6) * 5;
      arr.setAttribute('transform', `translate(292 ${s.y + s.dir * (26 + wob)}) rotate(${s.dir < 0 ? -90 : 90})`);
    },
  };
};

/** White paper slides into the guide box; its colour tint fades to neutral. */
KINDS.paper = (svg) => {
  el('rect', { x: 20, y: 14, width: 280, height: 168, rx: 12, class: 'ill-frame' }, svg);
  const guide = el('rect', { x: 120, y: 50, width: 80, height: 96, rx: 6, class: 'ill-guide' }, svg);
  const paper = el('rect', { width: 72, height: 88, rx: 3, class: 'ill-paper' }, svg);
  const tint = el('rect', { width: 72, height: 88, rx: 3, class: 'ill-tint' }, svg);
  const tick = el('path', { d: 'M138 98 l14 14 l28 -30', class: 'ill-tick' }, svg);
  const s = { p: 0, tp: 0, tint: 0.6, ttint: 0.6, done: false };
  return {
    update(u, dom) {
      s.tp = u.phase === 'waiting' ? 0.2 : 1;
      s.ttint = norm(u.value, dom[0], dom[1]);
      s.done = u.phase === 'done' || u.phase === 'hold';
    },
    frame(dt) {
      s.p = approach(s.p, s.tp, dt, 3); s.tint = approach(s.tint, s.ttint, dt, 3);
      const x = lerp(250, 124, s.p), y = lerp(110, 54, s.p);
      paper.setAttribute('x', x); paper.setAttribute('y', y);
      tint.setAttribute('x', x); tint.setAttribute('y', y);
      tint.style.opacity = String(0.85 * s.tint);
      guide.classList.toggle('good', s.done);
      tick.style.opacity = s.done ? 1 : 0;
    },
  };
};

/** Head-and-shoulders inside a 16:9 frame with thirds; arrows show where to move. */
KINDS.framing = (svg) => {
  el('rect', { x: 20, y: 14, width: 280, height: 158, rx: 10, class: 'ill-frame' }, svg);
  for (const x of [113, 206]) el('line', { x1: x, y1: 14, x2: x, y2: 172, class: 'ill-third' }, svg);
  for (const y of [67, 119]) el('line', { x1: 20, y1: y, x2: 300, y2: y, class: 'ill-third' }, svg);
  const clip = el('clipPath', { id: 'frameclip' }, svg);
  el('rect', { x: 20, y: 14, width: 280, height: 158, rx: 10 }, clip);
  const who = el('g', { 'clip-path': 'url(#frameclip)' }, svg);
  const p = el('g', {}, who);
  el('ellipse', { cx: 0, cy: -30, rx: 26, ry: 32, class: 'ill-skin' }, p);
  el('rect', { x: -60, y: 6, width: 120, height: 80, rx: 40, class: 'ill-body' }, p);
  const arr = arrow(svg);
  const s = { x: 160, tx: 160, y: 120, ty: 120, sc: 1, tsc: 1, dir: null, t: 0, good: false };
  return {
    update(u) {
      const pr = clamp(u.progress ?? 0, 0, 1);
      const k = u.hint?.kind;
      s.dir = k;
      const off = 1 - pr;
      s.tx = 160 + (k === 'move_left' ? 70 : k === 'move_right' ? -70 : 0) * off;
      s.tsc = 1 + (k === 'move_back' ? 0.7 : k === 'move_forward' ? -0.45 : 0) * off;
      s.ty = 108 + (s.tsc - 1) * 30;
      s.good = ['hold', 'done'].includes(u.phase);
    },
    frame(dt) {
      s.t += dt;
      s.x = approach(s.x, s.tx, dt, 3); s.y = approach(s.y, s.ty, dt, 3); s.sc = approach(s.sc, s.tsc, dt, 3);
      p.setAttribute('transform', `translate(${s.x} ${s.y}) scale(${s.sc})`);
      const show = ['move_left', 'move_right', 'move_back', 'move_forward'].includes(s.dir) && !s.good;
      arr.style.opacity = show ? 1 : 0;
      const wob = Math.sin(s.t * 6) * 5;
      const map = { move_left: `translate(${s.x - 70 - wob} 60) scale(-1 1)`, move_right: `translate(${s.x + 70 + wob} 60)`,
        move_back: `translate(160 ${186 - wob}) rotate(90) scale(0.7)`, move_forward: `translate(160 ${186 + wob}) rotate(-90) scale(0.7)` };
      if (show) arr.setAttribute('transform', map[s.dir]);
    },
  };
};

/** Rolling bands that calm down as the flicker setting takes effect. */
KINDS.flicker = (svg) => {
  el('rect', { x: 20, y: 14, width: 280, height: 168, rx: 12, class: 'ill-frame' }, svg);
  const clip = el('clipPath', { id: 'flickclip' }, svg);
  el('rect', { x: 20, y: 14, width: 280, height: 168, rx: 12 }, clip);
  const g = el('g', { 'clip-path': 'url(#flickclip)' }, svg);
  el('ellipse', { cx: 160, cy: 92, rx: 34, ry: 44, class: 'ill-skin' }, g);
  el('rect', { x: 110, y: 134, width: 100, height: 60, rx: 30, class: 'ill-body' }, g);
  const bands = [0, 1, 2, 3].map(() => el('rect', { x: 20, width: 280, height: 22, class: 'ill-bandflick' }, g));
  const s = { lvl: 1, tl: 1, t: 0 };
  return {
    update(u, dom) { s.tl = norm(u.value, dom[0], dom[1]); },
    frame(dt) {
      s.t += dt; s.lvl = approach(s.lvl, s.tl, dt, 3);
      bands.forEach((b, i) => { b.setAttribute('y', 14 + ((s.t * 70 + i * 48) % 200) - 20); b.style.opacity = String(0.55 * s.lvl); });
    },
  };
};

/** A mini app window with a toggle; a magnifier pulses while we watch for the change. */
KINDS.settings = (svg) => {
  el('rect', { x: 40, y: 24, width: 240, height: 150, rx: 12, class: 'ill-frame' }, svg);
  el('rect', { x: 40, y: 24, width: 240, height: 26, rx: 12, class: 'ill-titlebar' }, svg);
  [0, 1, 2].forEach((i) => el('circle', { cx: 58 + i * 14, cy: 37, r: 4, class: 'ill-dot' }, svg));
  const label = el('text', { x: 64, y: 98, class: 'ill-setting-t' }, svg);
  el('rect', { x: 64, y: 112, width: 150, height: 8, rx: 4, class: 'ill-skel' }, svg);
  el('rect', { x: 64, y: 128, width: 110, height: 8, rx: 4, class: 'ill-skel' }, svg);
  const sw = el('g', {}, svg);
  const track = el('rect', { x: 222, y: 84, width: 44, height: 24, rx: 12, class: 'ill-switch' }, sw);
  const knob = el('circle', { cy: 96, r: 9, class: 'ill-switch-k' }, sw);
  const pointer = el('path', { d: 'M0 0 l0 22 l6 -6 l8 12 l4 -2 l-8 -12 l8 0 z', class: 'ill-cursor' }, svg);
  const watch = el('g', { class: 'ill-watch' }, svg);
  el('circle', { cx: 266, cy: 150, r: 12, class: 'ill-watch-c' }, watch);
  el('path', { d: 'M275 159 l10 10', class: 'ill-watch-h' }, watch);
  const s = { on: 1, ton: 1, t: 0, phase: 'waiting', text: 'Voice effects' };
  return {
    update(u) {
      s.phase = u.phase;
      s.ton = u.phase === 'done' ? 0 : 1;
      label.textContent = u.readings?.setting_label ? String(u.readings.setting_label) : (u.detail_setting || 'The setting');
    },
    frame(dt) {
      s.t += dt; s.on = approach(s.on, s.ton, dt, 6);
      knob.setAttribute('cx', lerp(234, 254, s.on));
      track.classList.toggle('on', s.on > 0.5);
      const cx = lerp(250, 242, (Math.sin(s.t * 2) + 1) / 2), cy = lerp(104, 98, (Math.sin(s.t * 2) + 1) / 2);
      pointer.setAttribute('transform', `translate(${cx} ${cy})`);
      pointer.style.opacity = s.phase === 'done' ? 0 : 1;
      watch.style.opacity = s.phase === 'working' ? String(0.5 + 0.5 * Math.sin(s.t * 5)) : '0';
    },
  };
};

/**
 * Mount an illustration into `host`. Returns { update(u, domain), destroy() }.
 * Unknown kinds fall back to the gauge-only view.
 */
// The backend names the Windows-settings step `settings_toggle`.
KINDS.settings_toggle = KINDS.settings;

export function mountIllustration(kind, host) {
  host.innerHTML = '';
  const svg = el('svg', { viewBox: '0 0 320 200', class: `ill ill-${kind}`, role: 'img', 'aria-hidden': 'true' });
  host.append(svg);
  const make = KINDS[kind];
  if (!make) { svg.remove(); return { update() {}, destroy() {} }; }
  const inst = make(svg);
  let raf = 0; let last = performance.now(); let alive = true;
  const reduced = reducedMotion();
  const loop = (now) => {
    if (!alive) return;
    const dt = Math.min(0.1, (now - last) / 1000); last = now;
    inst.frame(dt);
    raf = requestAnimationFrame(loop);
  };
  if (!reduced) raf = requestAnimationFrame(loop);
  return {
    update(u, domain) {
      inst.update(u, domain);
      if (reduced) inst.frame(10); // snap to the new state, no motion
    },
    destroy() { alive = false; cancelAnimationFrame(raf); svg.remove(); },
  };
}
