// 12. Lip-sync clap test: clap in front of the camera, we measure sound vs picture,
// say which side to delay, set the OBS Sync Offset when we can, and check again.
import { api, on } from '../api.js';
import { esc, html, icons, errorBanner, toast, advanced, kvTable, fmt } from '../components.js';
import { state, selectedMic, selectedCamera } from '../state.js';

const CLAPS_WANTED = 4;
const MAX_SECS = 14;
const MIN_SECS = 6;

const pos = (db) => Math.max(0, Math.min(100, ((db + 60) / 60) * 100));

export async function render(root, { go, refreshNav }) {
  let unlisten = [];
  let timer = null;
  let cancelled = false;
  let running = false;
  const mic = selectedMic();
  const cam = selectedCamera();
  const page = html('<section class="stack-lg"></section>');
  root.append(page);

  if (!state.env || !mic || !cam) {
    page.append(html(`<div class="stack" style="gap:6px"><span class="eyebrow">Lip sync</span><h1>Does your voice match your picture?</h1></div>`));
    page.append(html('<div class="banner warn"><span>We need to know your mic and camera first. Start from the Welcome page.</span></div>'));
    const b = html('<button class="btn primary">Go to Welcome</button>');
    b.addEventListener('click', () => go('welcome'));
    page.append(b);
    return;
  }

  if (state.autoStart === 'sync') { state.autoStart = null; start(); } else drawIntro();

  function head(title, lead) {
    return html(`<div class="stack" style="gap:6px"><span class="eyebrow">Lip sync</span><h1>${esc(title)}</h1><p class="lead">${esc(lead)}</p></div>`);
  }

  function drawIntro() {
    page.innerHTML = '';
    page.append(head('Does your voice match your picture?', 'Cameras and capture cards add a small delay to the picture, so viewers can see your mouth move a moment before or after they hear you. A 15-second clap test shows which, and how to fix it.'));
    const card = html(`
      <div class="card stack">
        <div class="row" style="gap:24px;flex-wrap:wrap">
          <div class="device"><div class="ico">${icons.mic}</div><div class="stack" style="gap:2px"><b>${esc(mic.name)}</b><span class="subtle">Your mic</span></div></div>
          <div class="device"><div class="ico">${icons.cam}</div><div class="stack" style="gap:2px"><b>${esc(cam.name)}</b><span class="subtle">Your camera</span></div></div>
        </div>
        <ul class="explain" style="margin:0;padding:0">
          <li><span class="ico">${icons.check}</span><span>Sit where you stream, with your hands in view of the camera.</span></li>
          <li><span class="ico">${icons.check}</span><span>Clap firmly four times. Not at a steady beat: clap, clap, short pause, clap, clap.</span></li>
          <li><span class="ico">${icons.check}</span><span>We change nothing while testing. If it is off, we tell you which side to delay.</span></li>
        </ul>
        <div class="row"><button class="btn primary lg" id="start">Start the clap test</button><button class="btn ghost" id="cant">I can't do this</button></div>
      </div>`);
    card.querySelector('#start').addEventListener('click', start);
    card.querySelector('#cant').addEventListener('click', drawManual);
    page.append(card);
  }

  async function start() {
    page.innerHTML = '';
    page.append(head('Clap in front of the camera', 'Keep your hands where you see them in the picture.'));
    const view = html(`
      <div class="card stack-lg">
        <div class="preview" id="pv"><span class="badge" id="badge">Live</span></div>
        <div class="spread" style="align-items:flex-start">
          <div class="stack" style="gap:8px;flex:1"><div class="prompt" id="title">Getting ready…</div><p class="muted" id="text" aria-live="polite">Starting your mic and camera.</p></div>
          <div class="countdown" id="count" aria-label="Claps heard"></div>
        </div>
        <div class="stack" style="gap:6px"><div class="meter" aria-hidden="true"><div class="fill" id="fill"></div></div>
          <div class="meter-labels"><span>quiet</span><span>each clap should jump up</span><span>loud</span></div></div>
        <div class="row"><button class="btn ghost" id="cancel">Cancel</button><button class="btn ghost" id="cant">I can't do this</button></div>
      </div>`);
    page.append(view);
    const $ = (s) => view.querySelector(s);
    const stop = async () => { cancelled = true; teardown(); try { await api.stopSyncTest(); } catch (_) { /* nothing running */ } };
    $('#cancel').addEventListener('click', async () => { await stop(); cancelled = false; drawIntro(); });
    $('#cant').addEventListener('click', async () => { await stop(); cancelled = false; drawManual(); });
    cancelled = false;
    let claps = 0;
    let lastClapAt = 0;
    const startedAt = performance.now();
    const setClaps = (n) => { $('#count').textContent = `${n} / ${CLAPS_WANTED}`; };
    setClaps(0);
    try {
      unlisten.push(await on('camera-preview', (p) => {
        const src = typeof p === 'string' ? p : p && p.image;
        if (!src) return;
        let img = $('#pv img');
        if (!img) { img = document.createElement('img'); img.alt = 'Live camera preview'; $('#pv').prepend(img); }
        img.src = src;
      }));
      unlisten.push(await on('camera-picture', (p) => {
        const badge = $('#badge');
        if (badge) { badge.textContent = p.live ? 'Live' : 'No picture'; badge.classList.toggle('bad', !p.live); }
      }));
      unlisten.push(await on('sync-progress', (p) => {
        $('#fill').style.width = `${pos(p.level_db + 12)}%`;
        if (p.claps !== claps) {
          claps = p.claps;
          lastClapAt = performance.now();
          setClaps(claps);
          $('#title').textContent = claps >= CLAPS_WANTED ? 'Got it. Hold still for a moment…' : 'Nice. Keep clapping.';
        }
      }));
      await api.startSyncTest(state.micId, state.cameraId);
      running = true;
      $('#title').textContent = 'Clap now';
      $('#text').textContent = 'Four firm claps, with a short pause in the middle. Not a steady beat.';
      await new Promise((resolve) => {
        timer = setInterval(() => {
          const t = (performance.now() - startedAt) / 1000;
          const settled = claps >= CLAPS_WANTED && performance.now() - lastClapAt > 1200;
          if (cancelled || t >= MAX_SECS || (settled && t >= MIN_SECS - 2)) { clearInterval(timer); resolve(); }
        }, 200);
      });
      if (cancelled) return;
      $('#title').innerHTML = '<span class="spinner tiny"></span> Working it out…';
      const report = await api.finishSyncTest();
      running = false;
      teardown();
      state.syncReport = report;
      refreshNav();
      drawResult(report);
    } catch (e) {
      running = false;
      teardown();
      try { await api.stopSyncTest(); } catch (_) { /* nothing running */ }
      if (cancelled) return;
      drawError(e);
    }
  }

  function drawError(e) {
    page.innerHTML = '';
    page.append(head('Does your voice match your picture?', ''));
    const noPicture = e && e.code === 'no_picture';
    page.append(errorBanner(e, null));
    const row = html('<div class="row" style="margin-top:12px"></div>');
    const again = html(`<button class="btn primary">${noPicture ? 'Try again' : 'Back'}</button>`);
    again.addEventListener('click', noPicture ? start : drawIntro);
    const cant = html('<button class="btn ghost">I can\'t do this</button>');
    cant.addEventListener('click', drawManual);
    row.append(again, cant);
    if (noPicture) {
      const cb = html('<button class="btn">Fix my camera</button>');
      cb.addEventListener('click', () => go('camera'));
      row.append(cb);
    }
    page.append(row);
  }

  function drawResult(r) {
    page.innerHTML = '';
    const f = r.finding;
    if (r.verdict === 'in_sync') {
      page.append(head('Your voice and picture line up', state.syncApplied ? 'The change worked. Viewers hear you just as they see you.' : 'Viewers hear you just as they see you. Nothing to change.'));
      page.append(html('<div class="banner good"><b>In sync</b></div>'));
    } else if (r.verdict === 'unclear') {
      page.append(head("We couldn't get a clear answer", r.hint || 'Please try once more.'));
    } else {
      page.append(head(f ? f.title : 'Your voice and picture are out of step', f ? f.why : ''));
    }
    if (r.coarse && r.verdict !== 'unclear') {
      page.append(html('<div class="banner info"><span>Your camera gave us only a few pictures a second, so this is a rough guide. Run it again after you change something.</span></div>'));
    }
    const fix = r.fix;
    if (fix) {
      const side = fix.side === 'delay_camera' ? 'Delay your camera' : 'Delay your sound';
      const why = fix.side === 'delay_camera'
        ? 'Your picture is ahead of your voice, so hold the picture back a little in OBS.'
        : 'Your voice is ahead of your picture, so hold the sound back a little in OBS.';
      const card = html(`<div class="card stack"><h3>${esc(side)}</h3><p class="muted">${esc(why)}</p>
        <ol class="steps-list">${fix.steps.map((s) => `<li>${esc(s)}</li>`).join('')}</ol><div id="auto" class="stack"></div>
        <div class="row"><button class="btn primary lg" id="again">Check again</button></div></div>`);
      page.append(card);
      const auto = card.querySelector('#auto');
      if (fix.can_auto) {
        const b = html(`<button class="btn primary">Set it in OBS for me</button>`);
        auto.append(b);
        b.addEventListener('click', async () => {
          b.disabled = true;
          try {
            const a = await api.applySyncFix();
            auto.innerHTML = '';
            auto.append(html(`<div class="banner ${a.ok ? 'good' : 'warn'}"><span>${esc(a.message)}</span></div>`));
            if (a.ok) {
              state.syncApplied = true;
              const undo = html('<button class="btn sm ghost">Undo</button>');
              undo.addEventListener('click', async () => {
                undo.disabled = true;
                try {
                  const u = await api.undoSyncFix();
                  state.syncApplied = false;
                  toast(u.ok ? 'Put back' : "Couldn't undo", u.message, u.ok ? 'good' : 'warn');
                } catch (e) { toast("Couldn't undo", e.message, 'bad'); }
                undo.remove();
              });
              auto.append(undo);
            } else b.disabled = false;
          } catch (e) {
            b.disabled = false;
            toast("Couldn't set it", e.message, 'bad');
          }
        });
      }
      card.querySelector('#again').addEventListener('click', start);
    } else {
      const row = html('<div class="row"></div>');
      const again = html(`<button class="btn ${r.verdict === 'unclear' ? 'primary' : ''}">${r.verdict === 'unclear' ? 'Try again' : 'Run it again'}</button>`);
      again.addEventListener('click', start);
      row.append(again);
      if (r.verdict === 'unclear') {
        const cant = html('<button class="btn ghost">I can\'t do this</button>');
        cant.addEventListener('click', drawManual);
        row.append(cant);
      } else {
        const home = html('<button class="btn primary">Back to home</button>');
        home.addEventListener('click', () => go('home'));
        row.append(home);
      }
      page.append(row);
    }
    if (r.offset_ms != null) {
      page.append(advanced('Advanced: the numbers', kvTable([
        ['Sound after picture', fmt(r.offset_ms, 0, ' ms') + (r.offset_ms < 0 ? ' (negative: sound first)' : '')],
        ['OBS Sync Offset already set', fmt(r.obs_sync_offset_ms, 0, ' ms')],
        ['Match quality', fmt(r.score, 2)],
        ['Claps heard', String(r.claps)],
        ['Pictures per second', fmt(r.frames_per_s, 1)],
      ])));
    }
  }

  function drawManual() {
    page.innerHTML = '';
    page.append(head('No problem, check it by eye', "You can still tell if it's off without the test."));
    const card = html(`<div class="card stack"><ol class="steps-list">
      <li>In OBS press Start Recording, say "ba ba ba" or clap a few times in front of your camera, then stop.</li>
      <li>Open the recording in any video player and watch your hands or lips.</li>
      <li>If you hear the clap after your hands meet, delay the camera: right-click it in OBS, choose Filters, add a Render Delay, and try 100 ms.</li>
      <li>If you hear the clap before your hands meet, delay the sound: in OBS open the gear next to your mic in the Audio Mixer, choose Advanced Audio Properties and set Sync Offset to 100 ms.</li>
      <li>Record again and repeat with a bigger or smaller number until it looks right.</li>
    </ol><div class="row"><button class="btn primary" id="home">Back to home</button><button class="btn ghost" id="try">Try the clap test after all</button></div></div>`);
    card.querySelector('#home').addEventListener('click', () => go('home'));
    card.querySelector('#try').addEventListener('click', drawIntro);
    page.append(card);
  }

  function teardown() {
    clearInterval(timer);
    for (const u of unlisten) { try { u(); } catch (_) { /* gone */ } }
    unlisten = [];
  }

  return () => {
    cancelled = true;
    teardown();
    if (running) api.stopSyncTest().catch(() => {});
  };
}
