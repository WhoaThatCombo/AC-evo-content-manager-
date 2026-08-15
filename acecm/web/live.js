/* Public live map. Read-only. No admin APIs. */
const $ = (s, r) => (r || document).querySelector(s);
const esc = s => String(s ?? '').replace(/[&<>"]/g,
  c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const el = (t, c, h) => { const e = document.createElement(t);
  if (c) e.className = c; if (h != null) e.innerHTML = h; return e; };

const qs = new URLSearchParams(location.search);
const PID = qs.get('id') || '';
const qid = PID ? ('?id=' + encodeURIComponent(PID)) : '';

async function get(path) {
  try {
    const r = await fetch(path, { cache: 'no-store' });
    return await r.json();
  } catch (e) {
    return { ok: false, error: String(e && e.message || e) };
  }
}

const lapTime = s => {
  if (s == null || !isFinite(s)) return '—';
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(3).padStart(6, '0')}`;
};

let track = null, cars = [], bounds = null;
const cv = $('#map');
const cx = cv.getContext('2d');
const buf = new Map();
const heads = new Map();
const INTERP = 0.28;
let newest = 0, playT = null, lastFrame = null;

function carKey(c) { return c.id || c.label || 'anon'; }

function pushSample(c) {
  const k = carKey(c);
  const b = buf.get(k) || [];
  const seen = b.length ? b[b.length - 1].t : 0;
  const trail = c.trail && c.trail.length ? c.trail
              : (c.t ? [[c.t, c.x, c.z]] : []);
  for (const [t, x, z] of trail) if (t > seen) b.push({ t, x, z });
  while (b.length > 60) b.shift();
  buf.set(k, b);
  if (b.length) newest = Math.max(newest, b[b.length - 1].t);
}

function renderClock() {
  const now = performance.now() / 1000;
  const dt = lastFrame == null ? 0 : Math.min(now - lastFrame, 0.25);
  lastFrame = now;
  if (!newest) return 0;
  if (playT == null) playT = newest - INTERP;
  const lag = newest - playT;
  if (lag > 2 || lag < -0.5) playT = newest - INTERP;
  else {
    const rate = 1 + Math.max(-0.12, Math.min(0.12, (lag - INTERP) * 0.6));
    playT += dt * rate;
    if (playT > newest) playT = newest;
  }
  return playT;
}

function sampleAt(key, when) {
  const b = buf.get(key);
  if (!b || !b.length) return null;
  if (b.length === 1 || when <= b[0].t) return [b[0].x, b[0].z];
  for (let i = b.length - 1; i > 0; i--) {
    if (b[i - 1].t <= when && when <= b[i].t) {
      const span = b[i].t - b[i - 1].t;
      const f = span > 0 ? (when - b[i - 1].t) / span : 1;
      return [b[i - 1].x + (b[i].x - b[i - 1].x) * f,
              b[i - 1].z + (b[i].z - b[i - 1].z) * f];
    }
  }
  const last = b[b.length - 1];
  return [last.x, last.z];
}

function headingAt(key, when) {
  const a = sampleAt(key, when - 0.09), b = sampleAt(key, when + 0.09);
  let want = null;
  if (a && b) {
    const dx = b[0] - a[0], dz = b[1] - a[1];
    if (Math.abs(dx) + Math.abs(dz) > 0.08)
      want = (Math.atan2(dx, dz) * 180 / Math.PI + 360) % 360;
  }
  if (want == null) return heads.get(key) ?? null;
  const prev = heads.get(key);
  if (prev == null) { heads.set(key, want); return want; }
  const d = ((want - prev + 540) % 360) - 180;
  const next = (prev + d * 0.35 + 360) % 360;
  heads.set(key, next);
  return next;
}

function fit() {
  const r = cv.getBoundingClientRect(), d = devicePixelRatio || 1;
  cv.width = r.width * d; cv.height = r.height * d;
  cx.setTransform(d, 0, 0, d, 0, 0);
  if (!track || !(track.points || []).length) { bounds = null; return; }
  const all = track.edges && track.edges.left
    ? track.points.concat(track.edges.left, track.edges.right)
    : track.points;
  const xs = all.map(q => q[0]), ys = all.map(q => q[1]);
  const x0 = Math.min(...xs), x1 = Math.max(...xs);
  const y0 = Math.min(...ys), y1 = Math.max(...ys);
  const pad = 26, s = Math.min((r.width - pad * 2) / Math.max(x1 - x0, 1),
                               (r.height - pad * 2) / Math.max(y1 - y0, 1));
  bounds = { x0, y0, s, ox: (r.width - (x1 - x0) * s) / 2,
             oy: (r.height - (y1 - y0) * s) / 2 };
}
const P = q => [bounds.ox + (q[0] - bounds.x0) * bounds.s,
                bounds.oy + (q[1] - bounds.y0) * bounds.s];
addEventListener('resize', fit);

function draw() {
  const r = cv.getBoundingClientRect();
  cx.clearRect(0, 0, r.width, r.height);
  if (!track || !bounds) return;
  cx.lineJoin = cx.lineCap = 'round';
  const ed = track.edges;
  if (ed && ed.left && ed.right) {
    const ring = pts => { pts.forEach((q, i) => { const a = P(q);
      i ? cx.lineTo(a[0], a[1]) : cx.moveTo(a[0], a[1]); }); cx.closePath(); };
    cx.beginPath(); ring(ed.left); ring(ed.right);
    cx.fillStyle = '#22272e'; cx.fill('evenodd');
    cx.strokeStyle = '#414a55'; cx.lineWidth = 1.2; cx.stroke();
    cx.strokeStyle = 'rgba(122,162,255,.25)'; cx.lineWidth = 1; cx.beginPath();
    track.points.forEach((q, i) => { const a = P(q);
      i ? cx.lineTo(a[0], a[1]) : cx.moveTo(a[0], a[1]); });
    cx.stroke();
  } else {
    cx.strokeStyle = '#22272e'; cx.lineWidth = 7; cx.beginPath();
    track.points.forEach((q, i) => { const a = P(q);
      i ? cx.lineTo(a[0], a[1]) : cx.moveTo(a[0], a[1]); });
    cx.closePath(); cx.stroke();
    cx.strokeStyle = '#414a55'; cx.lineWidth = 2.5; cx.stroke();
  }
  const rt = renderClock();
  cars.forEach(c => {
    const pos = sampleAt(carKey(c), rt) || [c.x, c.z];
    const a = P(pos);
    const hd = headingAt(carKey(c), rt);
    cx.save();
    cx.translate(a[0], a[1]);
    if (hd != null) cx.rotate((90 - hd) * Math.PI / 180);
    cx.fillStyle = c.named ? '#ffd166' : '#2ee6c8';
    cx.strokeStyle = '#0b0e13'; cx.lineWidth = 1.4;
    cx.beginPath(); cx.roundRect(-7, -3.4, 14, 6.8, 2); cx.fill(); cx.stroke();
    cx.restore();
    if (c.named) {
      cx.fillStyle = '#ffd166';
      cx.font = '12px ui-sans-serif,sans-serif';
      const spd = c.kmh != null ? '  ' + Math.round(c.kmh) + ' km/h' : '';
      cx.fillText(c.label + spd, a[0] + 16, a[1] - 8);
    }
  });
}

function renderBoard(rows) {
  const board = $('#board');
  board.innerHTML = '';
  if (!(rows || []).length) {
    board.append(el('div', 'empty', 'No completed laps yet.'));
    return;
  }
  rows.forEach(r => {
    board.append(el('div', 'chk',
      `<span class="name"><b>P${r.pos}</b> ${esc(r.label)}`
      + `<div class="tiny dim">${esc(r.model || '')} · ${r.laps} lap(s) · `
      + `last ${lapTime(r.last)}</div></span>`
      + `<span class="pill off">${lapTime(r.best)}</span>`
      + (r.on_track
        ? '<span class="pill on"><i class="dot"></i>on track</span>'
        : '<span class="pill">off</span>')));
  });
}

function renderList() {
  const list = $('#list');
  list.innerHTML = '';
  if (!cars.length) {
    list.append(el('div', 'empty', 'No moving cars yet.'));
    return;
  }
  cars.forEach(c => {
    list.append(el('div', 'chk',
      `<span class="name">${c.named ? esc(c.label) : '<span class="dim">unidentified</span>'}`
      + `<div class="tiny dim">${esc(c.model || '')}</div></span>`
      + `<span class="pill off">${c.kmh != null ? Math.round(c.kmh) + ' km/h' : '—'}</span>`));
  });
}

let lastUi = 0;
async function tick() {
  const d = await get('/api/live' + qid);
  const st = $('#st');
  if (!d.ok) {
    st.className = 'pill off';
    st.innerHTML = '<i class="dot"></i>' + esc(d.hint || d.error || 'telemetry off');
    cars = [];
    return;
  }
  cars = d.cars || [];
  cars.forEach(pushSample);
  const live = new Set(cars.map(carKey));
  for (const k of [...buf.keys()]) if (!live.has(k)) buf.delete(k);
  const c = d.counts || {};
  st.className = 'pill on';
  st.innerHTML = `<i class="dot"></i>${esc(d.server || 'server')} · `
    + `${c.cars || 0} car(s) · ${c.named || 0} named · `
    + `${c.unidentified || 0} unlabeled`;
  if (d.server) $('#ttl').textContent = d.server;
  if (performance.now() - lastUi < 900) return;
  lastUi = performance.now();
  const lb = await get('/api/live/board' + qid);
  renderBoard(lb.rows || []);
  renderList();
}

(async function boot() {
  track = await get('/api/live/track' + qid);
  if (track && track.track) $('#sub').textContent = track.track
    + (track.layout ? ' · ' + track.layout : '');
  fit();
  await tick();
  setInterval(tick, 120);
  (function frame() {
    draw();
    requestAnimationFrame(frame);
  })();
})();
