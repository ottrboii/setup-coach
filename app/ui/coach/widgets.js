// Live coaching widgets: target-zone gauge, hold-steady ring, camera overlay.
import { html, esc } from '../components.js';
import { norm, reducedMotion } from './anim.js';

/** Horizontal gauge: green target zone + a marker that glides to the live value. */
export function gauge(lowLabel, highLabel) {
  const node = html(`
    <div class="cgauge" aria-hidden="true">
      <div class="cgauge-track"><span class="cgauge-zone-label">sweet spot</span><div class="cgauge-zone"></div><div class="cgauge-fill"></div><div class="cgauge-marker"><span></span></div></div>
      <div class="cgauge-labels"><span>${esc(lowLabel || '')}</span><span>${esc(highLabel || '')}</span></div>
    </div>`);
  const zone = node.querySelector('.cgauge-zone');
  const zoneLabel = node.querySelector('.cgauge-zone-label');
  const fill = node.querySelector('.cgauge-fill');
  const marker = node.querySelector('.cgauge-marker');
  return {
    node,
    update(u, dom) {
      const has = u.value !== null && u.value !== undefined && Number.isFinite(u.value);
      node.classList.toggle('empty', !has);
      if (u.target) {
        const a = norm(u.target.min, dom[0], dom[1]) * 100;
        const b = norm(u.target.max, dom[0], dom[1]) * 100;
        zone.style.left = `${a}%`; zone.style.width = `${Math.max(2, b - a)}%`;
        zoneLabel.style.left = `${(a + b) / 2}%`;
      }
      if (!has) return;
      const p = norm(u.value, dom[0], dom[1]) * 100;
      marker.style.left = `${p}%`;
      fill.style.width = `${p}%`;
      const inZone = u.target ? u.value >= u.target.min && u.value <= u.target.max : u.phase === 'hold';
      node.dataset.state = inZone ? 'good' : u.phase === 'almost' ? 'close' : 'off';
    },
  };
}

/** Circular "hold steady" ring that fills while phase=hold. */
export function holdRing() {
  const r = 30; const c = 2 * Math.PI * r;
  const node = html(`
    <div class="hold" aria-hidden="true">
      <svg viewBox="0 0 72 72" width="72" height="72"><circle cx="36" cy="36" r="${r}" class="hold-track"/><circle cx="36" cy="36" r="${r}" class="hold-val" stroke-dasharray="${c.toFixed(1)}" stroke-dashoffset="${c.toFixed(1)}"/></svg>
      <span class="hold-t">hold</span>
    </div>`);
  const val = node.querySelector('.hold-val');
  const t = node.querySelector('.hold-t');
  return {
    node,
    update(u) {
      const need = u.hold_needed_s || 0;
      const f = need > 0 ? Math.max(0, Math.min(1, (u.hold_s || 0) / need)) : u.phase === 'done' ? 1 : 0;
      val.style.strokeDashoffset = String(c * (1 - f));
      val.style.opacity = f > 0.01 ? '1' : '0';
      node.classList.toggle('active', u.phase === 'hold' || u.phase === 'done');
      t.textContent = u.phase === 'done' ? '✓' : need > 0 && u.phase === 'hold' ? `${Math.max(0, need - (u.hold_s || 0)).toFixed(1)}s` : 'hold';
    },
  };
}

/** Live camera preview with face/paper boxes, clipped areas and animated arrows. */
export function cameraOverlay() {
  const node = html('<div class="coverlay"><canvas></canvas><span class="badge">Live</span></div>');
  const canvas = node.querySelector('canvas');
  const g = canvas.getContext('2d');
  const img = new Image();
  const mask = new Image();
  let data = null; let raf = 0; let alive = true; let t0 = performance.now();
  img.onload = draw;
  function draw() {
    if (!alive || !img.naturalWidth) return;
    const W = img.naturalWidth, H = img.naturalHeight;
    if (canvas.width !== W) { canvas.width = W; canvas.height = H; }
    g.drawImage(img, 0, 0, W, H);
    if (data?.clipped_mask && mask.complete && mask.naturalWidth) {
      g.save(); g.globalAlpha = 0.55; g.drawImage(mask, 0, 0, W, H); g.restore();
    }
    const box = (r, color, dash) => {
      if (!r) return;
      g.save(); g.strokeStyle = color; g.lineWidth = Math.max(2, W / 200); if (dash) g.setLineDash([8, 6]);
      g.strokeRect(r.x, r.y, r.w, r.h); g.restore();
    };
    box(data?.face, data?.face_ok ? '#3ecf8e' : '#f5b84a');
    box(data?.paper, '#ffffff', true);
    const t = (performance.now() - t0) / 1000;
    const pulse = reducedMotion() ? 0 : Math.sin(t * 6) * 6;
    for (const a of data?.arrows || []) {
      const normd = [a.x, a.y, a.dx, a.dy].every((v) => Math.abs(v) <= 1.5);
      const sx = normd ? W : 1, sy = normd ? H : 1;
      const x = a.x * sx, y = a.y * sy, dx = a.dx * sx, dy = a.dy * sy;
      const len = Math.hypot(dx, dy) || 1; const ux = dx / len, uy = dy / len;
      const ox = ux * pulse, oy = uy * pulse;
      g.save(); g.strokeStyle = '#fff'; g.fillStyle = '#fff'; g.lineWidth = Math.max(4, W / 110); g.lineCap = 'round';
      g.shadowColor = 'rgba(0,0,0,.6)'; g.shadowBlur = 6;
      g.beginPath(); g.moveTo(x + ox, y + oy); g.lineTo(x + dx + ox, y + dy + oy); g.stroke();
      const hx = x + dx + ox, hy = y + dy + oy, hs = Math.max(12, W / 40);
      g.beginPath(); g.moveTo(hx + ux * hs * 0.4, hy + uy * hs * 0.4);
      g.lineTo(hx - ux * hs + -uy * hs * 0.7, hy - uy * hs + ux * hs * 0.7);
      g.lineTo(hx - ux * hs - -uy * hs * 0.7, hy - uy * hs - ux * hs * 0.7); g.closePath(); g.fill();
      g.restore();
    }
  }
  const loop = () => { if (!alive) return; draw(); raf = requestAnimationFrame(loop); };
  if (!reducedMotion()) raf = requestAnimationFrame(loop);
  return {
    node,
    update(d) {
      data = d;
      if (d.clipped_mask) mask.src = d.clipped_mask;
      if (d.frame) img.src = d.frame;
    },
    destroy() { alive = false; cancelAnimationFrame(raf); },
  };
}
