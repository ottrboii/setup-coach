// 7. Stream health: live lights + alerts, post-stream report.
import { api, on } from '../api.js';
import { esc, html, scoreRing, scoreWord, findingCard, toast, errorBanner, advanced, kvTable, fmt } from '../components.js';
import { state } from '../state.js';

const LIGHTS = [
  { key: 'network', label: 'Internet', ok: 'Upload keeping up' },
  { key: 'encoding', label: 'PC encoding', ok: 'Encoder keeping up' },
  { key: 'rendering', label: 'Game load', ok: 'OBS getting enough GPU' },
];

function clock(ms) {
  const s = Math.round(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return h ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`;
}

export async function render(root, { refreshNav }) {
  const subs = [];
  let running = false;
  const page = html(`
    <section class="stack-lg">
      <div class="stack" style="gap:6px"><span class="eyebrow">Stream health</span><h1>Is your stream smooth?</h1>
      <p class="lead">Leave this running while you stream. We'll tell you in plain words if frames drop — and whether it's your internet, your PC or the game.</p></div>
      <div id="live"></div>
      <div id="report"></div>
    </section>`);
  root.append(page);
  const live = page.querySelector('#live');
  const reportBox = page.querySelector('#report');

  const status = { network: 'good', encoding: 'good', rendering: 'good' };
  const detail = { network: '', encoding: '', rendering: '' };
  const kbps = [];
  let lastBytes = null;
  let lastT = null;

  function drawLive() {
    live.innerHTML = '';
    const lights = html(`<div class="lights">${LIGHTS.map((l) => {
      const st = status[l.key];
      const word = !running ? 'Not watching' : st === 'good' ? l.ok : st === 'warn' ? 'Struggling a bit' : 'Dropping frames';
      return `<div class="light"><span class="subtle">${l.label}</span><div class="big"><span class="led ${running ? st : ''}"></span>${esc(word)}</div><span class="subtle">${esc(detail[l.key] || '')}</span></div>`;
    }).join('')}</div>`);
    live.append(lights);
    const bit = html(`<div class="card stack" style="margin-top:16px"><div class="spread"><b>Upload speed (bitrate)</b><span class="subtle" id="kb">${kbps.length ? `${Math.round(kbps[kbps.length - 1])} kbps` : '—'}</span></div><svg class="spark" viewBox="0 0 300 46" preserveAspectRatio="none"><polyline id="line" fill="none" stroke="var(--accent)" stroke-width="2" points=""/></svg></div>`);
    live.append(bit);
    drawSpark();
    const ctl = html(`<div class="row" style="margin-top:16px">${running
      ? '<button class="btn danger lg" id="stop">Stop and show my report</button><span class="subtle">Watching OBS every 2 seconds…</span>'
      : '<button class="btn primary lg" id="start">Start watching my stream</button><span class="subtle">Start this when you go live (or just before).</span>'}</div>`);
    ctl.querySelector('#start')?.addEventListener('click', start);
    ctl.querySelector('#stop')?.addEventListener('click', stop);
    live.append(ctl);
  }

  function drawSpark() {
    const line = live.querySelector('#line');
    if (!line || kbps.length < 2) return;
    const max = Math.max(...kbps, 1000);
    const pts = kbps.slice(-60).map((v, i, a) => `${(i / Math.max(1, a.length - 1)) * 300},${46 - (v / max) * 42}`).join(' ');
    line.setAttribute('points', pts);
    const kb = live.querySelector('#kb');
    if (kb) kb.textContent = `${Math.round(kbps[kbps.length - 1])} kbps`;
  }

  async function start() {
    try {
      subs.push(await on('health-sample', (s) => {
        if (lastBytes !== null && s.t_ms > lastT) kbps.push(((s.bytes_sent - lastBytes) * 8) / (s.t_ms - lastT));
        lastBytes = s.bytes_sent; lastT = s.t_ms;
        drawSpark();
      }));
      subs.push(await on('health-alert', (a) => {
        const key = a.kind === 'low_bitrate' || a.kind === 'reconnecting' ? 'network' : a.kind;
        if (status[key] !== undefined) {
          status[key] = a.raised ? 'bad' : 'good';
          detail[key] = a.raised ? a.cause : '';
        }
        toast(a.title, a.raised ? `${a.cause} ${a.fix}` : '', a.raised ? 'bad' : 'good');
        drawLive();
      }));
      await api.startHealthMonitor();
      running = true;
      reportBox.innerHTML = '';
      drawLive();
    } catch (e) {
      live.append(errorBanner(e, start));
    }
  }

  async function stop() {
    try {
      const r = await api.stopHealthMonitor();
      running = false;
      subs.splice(0).forEach((u) => u());
      state.healthReport = r;
      refreshNav();
      drawLive();
      drawReport(r);
    } catch (e) {
      toast("Couldn't stop the monitor", e.message, 'bad');
    }
  }

  function drawReport(r) {
    reportBox.innerHTML = '';
    const pctSeen = (t) => (t && t.total ? 100 - t.percent : null);
    const seen = Math.min(pctSeen(r.network) ?? 100, pctSeen(r.encoding) ?? 100, pctSeen(r.rendering) ?? 100);
    const card = html(`<div class="card hero"><div class="row" style="gap:28px"><div class="ring"></div>
      <div class="stack" style="gap:8px;flex:1;min-width:260px"><span class="eyebrow">After-stream report</span><h2>${esc(scoreWord(r.score.value).replace('Sounding', 'Running'))}</h2>
      <p class="muted">Over ${clock(r.duration_s * 1000)}, viewers got ${seen.toFixed(seen > 99.9 ? 2 : 1)}% of your frames${r.reconnect_events ? ` and you reconnected ${r.reconnect_events} time${r.reconnect_events === 1 ? '' : 's'}` : ''}.</p></div></div></div>`);
    card.querySelector('.ring').append(scoreRing(r.score.value, { label: 'stream' }));
    reportBox.append(card);
    if (r.worst_moments?.length) {
      reportBox.append(html(`<div class="card stack" style="margin-top:16px"><h3>Rough moments</h3><ul class="explain" style="margin:0;padding:0">${r.worst_moments.map((w) => `<li><span class="kbd">${clock(w.offset_ms)}</span><span>${esc(String(w.kind).toLowerCase() === 'network' ? 'Internet dropped frames' : String(w.kind).toLowerCase() === 'encoding' ? 'PC encoding fell behind' : 'Game starved OBS')} · ${w.loss_percent.toFixed(1)}% lost</span></li>`).join('')}</ul></div>`));
    }
    if (r.findings?.length) {
      const list = html('<div class="findings" style="margin-top:16px"></div>');
      r.findings.forEach((f) => list.append(findingCard(f, { visual: false })));
      reportBox.append(list);
    }
    reportBox.append(html('<div style="height:12px"></div>'));
    reportBox.append(advanced('Advanced: the numbers', kvTable([
      ['Network dropped', r.network ? `${r.network.lost} (${fmt(r.network.percent, 2, '%')})` : 'not available'],
      ['Encoding lag', `${r.encoding.lost} (${fmt(r.encoding.percent, 2, '%')})`],
      ['Rendering lag', `${r.rendering.lost} (${fmt(r.rendering.percent, 2, '%')})`],
      ['Bitrate avg / min', `${fmt(r.avg_bitrate_kbps, 0)} / ${fmt(r.min_bitrate_kbps, 0)} kbps`],
      ['Average fps', fmt(r.avg_fps, 1)],
      ['CPU avg / peak', `${fmt(r.avg_cpu_percent, 0, '%')} / ${fmt(r.peak_cpu_percent, 0, '%')}`],
    ])));
  }

  drawLive();
  if (state.healthReport) drawReport(state.healthReport);
  return () => { subs.forEach((u) => u()); if (running) api.stopHealthMonitor().catch(() => {}); };
}
