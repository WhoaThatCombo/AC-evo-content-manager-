/* ACECM UI - telemetry. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* ------------------------------------------------------------ telemetry -- */
let telTrack = null, telTimer = null, telCars = [], telProfile = null;
let telRaf = null, telLastUi = 0;
async function telemetryPage() {
  const p = $('#page');
  p.innerHTML = '';
  if (telTimer) { clearInterval(telTimer); telTimer = null; }
  if (logTimer) { clearInterval(logTimer); logTimer = null; }
  if (telRaf) { cancelAnimationFrame(telRaf); telRaf = null; }

  const head = el('div', 'card');
  // The server this page is showing is named in the status pill below. Do not
  // label it "first tracker" - the map and the cars used to resolve to
  // DIFFERENT servers, which read as simply the wrong track being drawn.
  head.innerHTML = '<h2>Live telemetry</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">Positions are read out '
    + 'of the dedicated server process — the server publishes no coordinates. '
    + 'Cars are found by which points <b>move</b>, so a car sitting still in '
    + 'the pits cannot be seen at all. Names are bound at <b>join</b>; if four '
    + 'cars leave the pits together and were never bound, they stay unlabeled '
    + 'rather than being guessed (which would swap people).</div>';
  const row = el('div', 'row wrap');
  const st = el('span', 'pill off', '<i class="dot"></i>checking');
  const go = el('button', 'primary sm', 'Start telemetry');
  go.onclick = async () => { const r = await api('telemetry/start', { id: telProfile });
    toast(r.ok ? 'Telemetry started' : (r.error || 'Failed'), !r.ok);
    setTimeout(telemetryPage, 2500); };
  const sp = el('button', 'sm danger', 'Stop');
  sp.onclick = async () => { await api('telemetry/stop', { id: telProfile }); toast('Stopped');
    setTimeout(telemetryPage, 800); };
  const share = el('button', 'sm primary', 'Copy live link');
  share.title = 'Anyone with the link can watch the map — no ACECM install';
  share.onclick = async () => {
    const r = await api('live/link');
    const url = (r && (r.url || r.local_url)) || '';
    if (!url) { toast('No share address yet', true); return; }
    try { await navigator.clipboard.writeText(url); } catch (e) {}
    toast('Copied ' + url + ' — they open it in a browser');
  };
  row.append(go, sp, share, st);
  head.append(row);
  p.append(head);

  // map
  const mapCard = el('div', 'card');
  mapCard.innerHTML = '<h2>Track</h2>';
  const cv = el('canvas');
  cv.style.width = '100%';
  cv.style.height = '58vh';
  cv.style.display = 'block';
  mapCard.append(cv);
  p.append(mapCard);

  // Leaderboard, joined to the live map by carId - the same key the telemetry
  // binds a car to, so "who is P1" and "which dot is that" are the same fact.
  const boardCard = el('div', 'card');
  boardCard.innerHTML = '<h2>Leaderboard</h2>'
    + '<div class="tiny dim">Lap times from the server log. Only completed '
    + 'laps count, so out-laps are absent.</div>';
  const board = el('div');
  boardCard.append(board);
  p.append(boardCard);

  const listCard = el('div', 'card');
  listCard.innerHTML = '<h2>Cars</h2>';
  const list = el('div');
  listCard.append(list);
  p.append(listCard);

  const lapTime = s => `${Math.floor(s / 60)}:${(s % 60).toFixed(3).padStart(6, '0')}`;

  if (!telTrack) {
    const t = await api('telemetry/track' + (telProfile ? '?id=' + telProfile : ''));
    if (t.ok) telTrack = t;
  }

  const cx = cv.getContext('2d');
  // --- smoothing ---------------------------------------------------------
  // Reading positions out of the server costs ~0.007 ms per sample (measured:
  // a ~150 kHz ceiling), so the data was never the limit - the map jumped
  // because the UI polled once a second, which at 200 km/h is a 55 m step.
  // Poll fast, then render every frame at a fixed delay behind the newest
  // sample and interpolate between the two that bracket it. The delay is what
  // makes it smooth rather than rubber-banding: we always draw between two
  // known positions instead of extrapolating past the last one.
  const POLL_MS = 100, INTERP_DELAY = 0.28;   // seconds, on the SERVER clock
  const telBuf = new Map();          // key -> [{t, x, z}, ...] server time, s
  const carKey = c => c.addr || c.name || c.id || 'anon';
  // Server clock -> local clock. Estimated from the newest sample we hold and
  // kept at its MINIMUM (the least-delayed observation wins, like NTP), so a
  // single slow response cannot drag the render clock forward and produce a
  // visible jump.
  let telSkew = null, telNewest = 0;

  function pushSample(c) {
    const k = carKey(c);
    const b = telBuf.get(k) || [];
    const seen = b.length ? b[b.length - 1].t : 0;
    // The payload carries the whole 30 Hz trail, so a 10 Hz poll still yields
    // every intermediate sample - the motion is reconstructed at the rate it
    // was measured, not the rate we happened to ask for it.
    const trail = c.trail && c.trail.length ? c.trail
                : (c.t ? [[c.t, c.x, c.z]] : []);
    for (const [t, x, z] of trail) if (t > seen) b.push({ t, x, z });
    while (b.length > 60) b.shift();
    telBuf.set(k, b);
    if (b.length) telNewest = Math.max(telNewest, b[b.length - 1].t);
  }

  // Adaptive jitter buffer.
  //
  // ⚠ Call this ONCE per frame, not once per car - it advances a clock, so
  // evaluating it per car ran it N times too fast.
  //
  // ⚠ Deriving the render time from a clock SKEW does not work here. Holding
  // the skew at its minimum (least-delayed observation) lets one unusually
  // fast response bias the clock forward for good, so it ends up riding the
  // very edge of the buffer: smooth while data keeps up, then a freeze the
  // moment a sample is late, then a jump to catch up. That is the
  // "smooth, freeze, jump" cycle exactly.
  //
  // Instead: keep our own playback head and steer it. Every frame it advances
  // by real elapsed time, sped up or slowed down slightly to hold a target
  // distance behind the newest sample. Rate changes are invisible (±12%);
  // a freeze or a jump is not.
  let playT = null, lastFrameT = null;
  function renderClock() {
    const now = performance.now() / 1000;
    const dt = lastFrameT == null ? 0 : Math.min(now - lastFrameT, 0.25);
    lastFrameT = now;
    if (!telNewest) return 0;
    if (playT == null) playT = telNewest - INTERP_DELAY;

    const lag = telNewest - playT;          // how far behind the live edge
    if (lag > 2.0 || lag < -0.5) {
      playT = telNewest - INTERP_DELAY;     // stalled or way off: resync once
    } else {
      // Steer toward INTERP_DELAY of lag. Too close to the edge -> run slower
      // and rebuild margin; too far behind -> run a little faster.
      const rate = 1 + Math.max(-0.12, Math.min(0.12, (lag - INTERP_DELAY) * 0.6));
      playT += dt * rate;
      // Never outrun the data: without this the head passes the newest sample,
      // sampleAt() clamps, and the car sits still until the next poll.
      if (playT > telNewest) playT = telNewest;
    }
    return playT;
  }

  function sampleAt(key, when) {
    const b = telBuf.get(key);
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
    return [last.x, last.z];         // clamp, never extrapolate past it
  }

  // Heading, taken from the SAME interpolated path as the drawn position.
  //
  // ⚠ Do not use the server's `heading`. It is the chord across the whole
  // history window (~0.8 s), so through a corner it points where the car was
  // most of a second ago and then swings hard to catch up on exit - the car
  // reads as overshooting the turn even though its position is correct.
  // A short baseline centred on the rendered instant keeps the nose pointing
  // where the car is actually going at the moment we are drawing.
  const telHead = new Map();
  function headingAt(key, when, fallback) {
    const a = sampleAt(key, when - 0.09), b = sampleAt(key, when + 0.09);
    let want = fallback;
    if (a && b) {
      const dx = b[0] - a[0], dz = b[1] - a[1];
      // Below a few cm of travel the direction is noise, so hold the last one
      // rather than letting a parked car spin on the spot.
      if (Math.abs(dx) + Math.abs(dz) > 0.08)
        want = (Math.atan2(dx, dz) * 180 / Math.PI + 360) % 360;
    }
    if (want == null) return null;
    const prev = telHead.get(key);
    if (prev == null) { telHead.set(key, want); return want; }
    // Smooth the short way round the circle, so 359 -> 1 does not spin.
    let d = ((want - prev + 540) % 360) - 180;
    const next = (prev + d * 0.35 + 360) % 360;
    telHead.set(key, next);
    return next;
  }

  let bounds = null;
  function fit() {
    const r = cv.getBoundingClientRect(), d = devicePixelRatio || 1;
    cv.width = r.width * d; cv.height = r.height * d;
    cx.setTransform(d, 0, 0, d, 0, 0);
    if (telTrack) {
      // Fit to the EDGES when we have them, or the track gets clipped by a
      // frame sized to the racing line.
      const all = telTrack.edges && telTrack.edges.left
        ? telTrack.points.concat(telTrack.edges.left, telTrack.edges.right)
        : telTrack.points;
      const xs = all.map(q => q[0]), ys = all.map(q => q[1]);
      const x0 = Math.min(...xs), x1 = Math.max(...xs);
      const y0 = Math.min(...ys), y1 = Math.max(...ys);
      const pad = 26, s = Math.min((r.width - pad*2)/(x1-x0), (r.height - pad*2)/(y1-y0));
      bounds = { x0, y0, s, ox: (r.width - (x1-x0)*s)/2, oy: (r.height - (y1-y0)*s)/2 };
    }
  }
  const P = q => [bounds.ox + (q[0]-bounds.x0)*bounds.s,
                  bounds.oy + (q[1]-bounds.y0)*bounds.s];
  fit();
  addEventListener('resize', fit);

  function draw() {
    const r = cv.getBoundingClientRect();
    cx.clearRect(0, 0, r.width, r.height);
    if (!telTrack || !bounds) return;
    cx.lineJoin = cx.lineCap = 'round';
    const ed = telTrack.edges;
    if (ed && ed.left && ed.right) {
      // Real track surface, from the per-point edge distances in the spline.
      // Without this the map draws the RACING LINE, which hugs the inside of
      // corners - a car running a normal line then sits several metres away
      // and it reads as a calibration error when the positions are correct.
      // ⚠ A circuit is a LOOP, so the surface is an annulus: two separate
      // closed rings. Drawing it as one path (left forward, right backward)
      // seams the two ends together across the track and floods the infield -
      // it reads as the track being doubled over on itself. Two closed
      // subpaths filled even-odd leaves the middle hollow.
      const ring = pts => { pts.forEach((q, i) => { const a = P(q);
        i ? cx.lineTo(a[0], a[1]) : cx.moveTo(a[0], a[1]); }); cx.closePath(); };
      cx.beginPath(); ring(ed.left); ring(ed.right);
      cx.fillStyle = '#22272e'; cx.fill('evenodd');
      cx.strokeStyle = '#414a55'; cx.lineWidth = 1.2; cx.stroke();
      // the racing line, faint, for reference
      cx.strokeStyle = 'rgba(122,162,255,.25)'; cx.lineWidth = 1; cx.beginPath();
      telTrack.points.forEach((q, i) => { const a = P(q);
        i ? cx.lineTo(a[0], a[1]) : cx.moveTo(a[0], a[1]); });
      cx.stroke();
    } else {
      cx.strokeStyle = '#22272e'; cx.lineWidth = 7; cx.beginPath();
      telTrack.points.forEach((q, i) => { const a = P(q);
        i ? cx.lineTo(a[0], a[1]) : cx.moveTo(a[0], a[1]); });
      cx.closePath(); cx.stroke();
      cx.strokeStyle = '#414a55'; cx.lineWidth = 2.5; cx.stroke();
    }

    const rt = renderClock();      // once per frame, not per car
    telCars.forEach(c => {
      const a = P(sampleAt(carKey(c), rt) || [c.x, c.z]);
      const hd = headingAt(carKey(c), rt, c.heading);
      const known = !!c.name;
      cx.save();
      cx.translate(a[0], a[1]);
      if (hd != null) cx.rotate((90 - hd) * Math.PI/180);
      cx.fillStyle = known ? (c.inferred ? '#7aa2ff' : '#ffd166') : '#2ee6c8';
      cx.strokeStyle = '#0b0e13'; cx.lineWidth = 1.4;
      cx.beginPath(); cx.roundRect(-7, -3.4, 14, 6.8, 2); cx.fill(); cx.stroke();
      cx.restore();
      if (known) {
        cx.strokeStyle = c.inferred ? 'rgba(122,162,255,.8)' : 'rgba(255,209,102,.85)';
        cx.lineWidth = 1.6;
        cx.beginPath(); cx.arc(a[0], a[1], 13, 0, 7); cx.stroke();
        cx.fillStyle = c.inferred ? '#7aa2ff' : '#ffd166';
        cx.font = '11px ui-monospace,monospace';
        cx.fillText((c.display || c.name) + (c.kmh != null ? '  ' + Math.round(c.kmh) + ' km/h' : ''),
                    a[0] + 18, a[1] - 8);
      }
    });
  }

  async function tick() {
    const d = await api('telemetry' + (telProfile ? '?id=' + telProfile : ''));
    if (!d.ok) {
      st.className = 'pill off';
      st.innerHTML = '<i class="dot"></i>' + esc(d.hint || 'not running');
      telCars = []; draw(); list.innerHTML = '';
      return;
    }
    telCars = d.cars || [];
    telCars.forEach(pushSample);
    // Drop buffers for cars that are gone, or a returning address resumes from
    // a stale position and the marker streaks across the map.
    const live = new Set(telCars.map(carKey));
    for (const k of [...telBuf.keys()]) if (!live.has(k)) telBuf.delete(k);
    // The map is redrawn every frame by the animation loop; the text below is
    // rebuilt about once a second. Rebuilding the DOM at the poll rate would
    // burn CPU and make the readouts flicker for no gain.
    if (performance.now() - telLastUi < 900) return;
    telLastUi = performance.now();

    const lb = await api('telemetry/leaderboard' + (telProfile ? '?id=' + telProfile : ''));
    board.innerHTML = '';
    const rows = (lb && lb.rows) || [];
    if (!rows.length) board.append(el('div', 'empty', 'No completed laps yet.'));
    // Live timing is joined in by carId - the same key the map binds to - so
    // the board and the dots always describe the same car.
    const liveBy = new Map(telCars.filter(c => c.id).map(c => [c.id, c]));
    rows.forEach(r => {
      const who = r.display || r.name || r.carid.slice(0, 12);
      const lv = liveBy.get(r.carid);
      const where = r.on_track
        ? '<span class="pill on"><i class="dot"></i>on track</span>'
        : (r.connected ? '<span class="pill">in pits</span>'
                       : '<span class="pill off">left</span>');
      const bits = [`${esc(r.model || '')} · ${r.laps} lap(s) · last ${lapTime(r.last)}`];
      if (lv && lv.lap_pct != null) bits.push(`lap ${lv.lap_pct}%`);
      // Only shown once a real line-crossing has been seen; before that the
      // lap clock has no meaningful start.
      if (lv && lv.predicted != null)
        bits.push(`pred <b>${lapTime(lv.predicted)}</b>`
          + (lv.predicted_rough ? ' <span class="dim">(rough)</span>' : '')
          + (lv.delta_best != null
             ? ` <span style="color:${lv.delta_best <= 0 ? '#2ee6c8' : '#ff8189'}">`
               + `${lv.delta_best <= 0 ? '' : '+'}${lv.delta_best.toFixed(3)}</span>` : ''));
      const gap = lv && lv.gap_leader != null && lv.gap_leader > 0
        ? `<span class="pill off">+${lv.gap_leader.toFixed(2)}s</span>` : '';
      board.append(el('div', 'chk',
        `<span class="name"><b>P${r.pos}</b> ${esc(who)}`
        + `<div class="tiny dim">${bits.join(' · ')}</div></span>`
        + gap + `<span class="pill off">${lapTime(r.best)}</span>` + where));
    });
    const c = d.counts || {};
    st.className = 'pill on';
    st.innerHTML = `<i class="dot"></i>${esc(d.server || 'server')} · `
      + `${c.cars} car(s) · ${c.players_connected} player(s) · :${d.port}`;
    list.innerHTML = '';
    if (!telCars.length) list.append(el('div', 'empty',
      'No moving cars. A stationary car cannot be detected.'));
    telCars.forEach(car => {
      const who = car.display || car.name;
      list.append(el('div', 'chk',
        `<span class="name">${who ? esc(who) : '<span class="dim">unidentified</span>'}`
        + (car.inferred ? ' <span class="pill" style="color:#7aa2ff">inferred</span>' : '')
        + `<div class="tiny dim">${esc(car.model || '')} `
        + `(${Math.round(car.x)}, ${Math.round(car.z)})</div></span>`
        + `<span class="pill off">${car.kmh != null ? Math.round(car.kmh) + ' km/h' : '—'}</span>`));
    });
  }
  await tick();
  telTimer = setInterval(tick, POLL_MS);
  // Draw on every animation frame, independent of the poll, so motion is
  // smooth at the display's refresh rate rather than stepping once per fetch.
  (function frame() {
    if (!telTimer || !document.body.contains(cv)) { telRaf = null; return; }
    draw();
    telRaf = requestAnimationFrame(frame);
  })();
}
