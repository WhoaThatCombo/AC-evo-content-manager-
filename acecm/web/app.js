/* Assetto Corsa EVO Content Manager - UI */
const $ = (s, r) => (r || document).querySelector(s);
const el = (t, c, h) => { const e = document.createElement(t);
  if (c) e.className = c; if (h != null) e.innerHTML = h; return e; };
const esc = s => String(s ?? '').replace(/[&<>"]/g,
  c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

async function api(path, body) {
  const opt = body ? { method: 'POST', body: JSON.stringify(body) } : {};
  const r = await fetch('/api/' + path, opt);
  const j = await r.json().catch(() => ({ error: 'bad response' }));
  if (j && j.error) toast(j.error, true);
  return j;
}
let toastT;
function toast(msg, bad) {
  const t = $('#toast');
  t.textContent = msg;
  t.style.borderLeftColor = bad ? 'var(--red)' : 'var(--accent)';
  t.classList.add('show');
  clearTimeout(toastT);
  toastT = setTimeout(() => t.classList.remove('show'), 3200);
}

/* ------------------------------------------------------------ dashboard -- */
async function dashboard() {
  // ⚠ Fetch everything FIRST, then clear and build. Clearing early leaves the
  // page blank for as long as the slowest request takes, and any await between
  // the clear and the appends is a window for a second render to interleave.
  const s = await api('state');
  const profs = (await api('profiles')).profiles || [];
  const cars = await api('cars');
  const trk = await api('tracks');
  const ov = await api('overview');
  const b = s.backend || {};
  const p = $('#page');
  p.innerHTML = '';

  // (the missing-executable warning now comes from /api/overview's attention
  // list, together with every other problem, so it is not a special case)

  // ⚠ Lead with what is WRONG. Counting things that exist is true and useless;
  // someone opens this page because something is not working, or to see
  // whether their server is up. (`ov` is fetched above, before the clear.)
  (ov.attention || []).forEach(a => {
    const box = el('div', a.level === 'bad' ? 'err' : 'warn');
    box.innerHTML = `<b>${esc(a.what)}</b><br>${esc(a.do)}`;
    p.append(box);
  });

  // A new version, said once and plainly. Checked in the background so a slow
  // or offline GitHub never delays the dashboard - the banner just appears
  // when the answer arrives.
  const upd = el('div', 'warn');
  upd.style.display = 'none';
  p.append(upd);
  api('update/check').then(r => {
    if (!r || !r.ok || !r.available) return;
    upd.style.display = '';
    upd.innerHTML = `<b>ACECM v${esc(r.latest)} is available</b> — you have `
      + `v${esc(r.current)}.`;
    const go = el('button', 'sm primary', 'Update now');
    go.style.marginTop = '6px';
    go.onclick = async () => {
      go.disabled = true;
      go.textContent = 'downloading…';
      const a = await api('update/apply', {});
      if (!a.ok) { toast(a.error || 'update failed', true);
                   go.disabled = false; go.textContent = 'Update now'; return; }
      go.textContent = 'Restart to finish';
      go.disabled = false;
      go.onclick = async () => {
        const s = await api('app/restart', {});
        if (!s.ok) toast(s.error || 'restart failed', true);
      };
    };
    upd.append(el('div'), go);
  }).catch(() => {});

  const g = el('div', 'stats');
  const tiles = [
    [ov.running ?? 0, `server${ov.running === 1 ? '' : 's'} running`],
    [ov.players ?? 0, 'players connected'],
    [ov.tracks ?? trk.total ?? '—', 'tracks you can load'],
    [cars.total ?? '—', `cars (${cars.mods ?? 0} modded)`],
    [ov.shared ?? 0, 'items shared for download'],
    [b.listening ? 'UP' : 'DOWN', `own backend :${b.port ?? '—'}`],
  ];
  tiles.forEach(([v, k]) => g.append(el('div', 'stat',
    `<b>${esc(v)}</b><span>${esc(k)}</span>`)));
  p.append(g);

  // What is actually running, with a way to act on it
  const live = el('div', 'card');
  live.innerHTML = '<h2>Servers</h2>';
  if (!(ov.servers || []).length) {
    live.append(el('div', 'empty',
      'No server profiles yet — create one on the Servers page'));
  } else {
    const t = el('table');
    t.innerHTML = '<thead><tr><th>Profile</th><th>Track</th><th>Players</th>'
      + '<th>Port</th><th></th></tr></thead>';
    const tb = el('tbody');
    ov.servers.forEach(sv => {
      const tr = el('tr');
      tr.innerHTML = `<td><span class="pill ${sv.running ? 'on' : 'off'}">`
        + `<i class="dot"></i>${esc(sv.name)}</span>`
        + (sv.pid ? `<div class="tiny dim">pid ${sv.pid}</div>` : '')
        + `</td>`
        + `<td>${esc(sv.track || '—')}<div class="tiny dim">`
        + `${esc(String(sv.layout || '').replace(/^layout_/, ''))}</div></td>`
        + `<td>${sv.running ? (sv.clients ?? '—') : '—'}</td>`
        + `<td class="dim">${esc(sv.port)}</td>`;
      const td = el('td');
      const act = el('button', sv.running ? 'sm danger' : 'sm primary',
                    sv.running ? 'Stop' : 'Start');
      act.onclick = async () => {
        const r = await api(sv.running ? 'server/stop' : 'server/start',
                            { id: sv.id });
        toast(r && r.error ? r.error : (sv.running ? 'Stopped' : 'Starting…'),
              !!(r && r.error));
        setTimeout(dashboard, sv.running ? 900 : 2000);
      };
      td.append(act);
      tr.append(td);
      tb.append(tr);
    });
    t.append(tb);
    live.append(t);
  }
  p.append(live);

  const c = el('div', 'card');
  c.innerHTML = '<h2>Quick start</h2>';
  const row = el('div', 'row wrap');
  const bStart = el('button', 'primary', 'Start default server');
  bStart.onclick = async () => {
    if (!profs.length) { toast('Create a server profile first'); return; }
    await api('server/start', { id: profs[0].id });
    toast('Starting ' + profs[0].name); setTimeout(dashboard, 1500);
  };
  const bStop = el('button', 'danger', 'Stop all servers');
  bStop.onclick = async () => { await api('server/stop', {}); toast('Stopped'); setTimeout(dashboard, 800); };
  // "Join server directly" was removed: the backend accepts the go-to-server
  // push and the client acknowledges it, but the join never completes, so the
  // button did nothing a user could see. Join through the in-game browser -
  // the proxy injects local servers into that list. The API endpoints are
  // still there for when the push is understood.
  const bGame = el('button', null, 'Launch game');
  bGame.onclick = async () => { const r = await api('game/launch', {}); if (r.ok) toast('Launching game'); };
  const bAI = el('button', null, 'Launch real AI race');
  bAI.title = 'One client. Instant Race with AiDriverEvo — not dedicated vAI ghosts.';
  bAI.onclick = async () => {
    const r = await api('game/launch_ai', { opponents: 16, min_strength: 70, max_strength: 95 });
    toast(r.ok ? 'Client starting — Instant Race, then Start. Look for Creating AiDriverEvo.'
               : (r.error || 'Launch failed'), !r.ok);
  };
  row.append(bStart, bStop, bGame, bAI);
  c.append(row);
  p.append(c);

  const st = el('div', 'card');
  st.innerHTML = '<h2>Status</h2>';
  st.append(el('div', 'row wrap',
    `<span class="pill ${s.tools_ok ? 'on' : 'bad'}"><i class="dot"></i>`
    + `backend tools ${s.tools_ok ? 'found' : 'missing'}</span>`
    + `<span class="pill ${b.have_cert ? 'on' : 'warn'}"><i class="dot"></i>`
    + `TLS ${b.have_cert ? 'ready' : 'run gencert.sh'}</span>`
    + `<span class="pill ${b.listening ? 'on' : 'off'}"><i class="dot"></i>`
    + `lobby ${b.listening ? 'listening' : 'stopped'}</span>`));
  st.append(el('div', 'tiny dim', 'Server dir: ' + esc(s.server_dir)));
  p.append(st);
}

/* -------------------------------------------------------------- servers -- */
let editing = null;
async function serversPage() {
  const { profiles, template, options, telemetry: telState } = await api('profiles');
  const trk = (await api('tracks')).tracks || [];
  const worker = await api('game/worker');
  const p = $('#page');
  p.innerHTML = '';
  if (worker && worker.attached) {
    const wc = el('div', 'card');
    const sc = worker.scan || {};
    wc.innerHTML = `<h2>AI worker</h2>
      <div class="tiny dim">phase <b>${esc(worker.phase || '?')}</b>
      &middot; profile ${esc(worker.profile_id || '')}
      &middot; game ${worker.game_running ? 'running' : 'not running'}
      &middot; AiDriverEvo lines: ${sc.ai_driver_evo_lines ?? 0}
      &middot; joined: ${sc.joined ? 'yes' : 'not yet'}</div>`;
    const hits = (sc.client_hits || []).slice(-8);
    if (hits.length) {
      const pre = el('pre', 'log', hits.map(esc).join('\n'));
      pre.style.maxHeight = '10em';
      wc.append(pre);
    }
    p.append(wc);
  }

  const head = el('div', 'row');
  const add = el('button', 'primary', '+ New server');
  add.onclick = () => { editing = { ...template }; serversPage(); };
  head.append(add);
  const stopAll = el('button', 'danger', 'Stop all');
  stopAll.onclick = async () => { await api('server/stop', {}); toast('Stopped'); };
  head.append(stopAll);
  p.append(el('div', 'card').appendChild(head).parentElement);

  if (editing) {
    // Real choices for the two fields that used to be typed by hand: tracks
    // this machine can actually load, and the car catalogue.
    const cat = await api('cars');
    const loc = await api('browser/local');
    const imported = new Set(loc.tracks || []);
    const modTracks = Object.entries(loc.track_map || {})
      .filter(([, folder]) => imported.has(folder))
      .sort((a, b) => a[0].localeCompare(b[0]));
    p.append(editor(editing, trk, options || {}, { cat, modTracks }));
  }

  if (!profiles.length && !editing) {
    p.append(el('div', 'card').appendChild(
      el('div', 'empty', 'No server profiles yet. Create one to get going.')).parentElement);
  }

  for (const prof of profiles) {
    const card = el('div', 'card');
    const t = trk.find(x => x.index === prof.track_index);
    // ⚠ A custom track WINS the summary. The row was built from track_index
    // alone, so a profile set to a deployed track still read as whatever stock
    // event the index happened to hold - "Watkins Glen" on a Miami server.
    // That is the same lie that made a profile look configured when it was not.
    const shown = prof.custom_track
      ? prof.custom_track + ' (deployed)'
      : (t ? t.label : 'track ' + prof.track_index);
    card.innerHTML = `<div class="row"><div class="grow">
        <b>${esc(prof.name)}</b>
        <div class="tiny dim">${esc(shown)}
        &middot; ${esc(prof.game_mode || 'PRACTICE')}
        &middot; ${prof.ai} AI &middot; ${prof.max_players} slots
        &middot; ${esc(prof.weather || 'CLEAR')}
        &middot; ${String(prof.tod_hour ?? 13).padStart(2,'0')}:${String(prof.tod_minute ?? 0).padStart(2,'0')}
        ${prof.driver_password ? '&middot; 🔒' : ''}
        &middot; tcp ${prof.tcp_port}</div></div>
        <span class="pill off" data-st="${prof.id}"><i class="dot"></i>checking</span>
      </div>`;
    const row = el('div', 'row wrap');
    row.style.marginTop = '10px';
    const start = el('button', 'primary sm', 'Start');
    // ⚠ A dedicated server takes ~30 s to bind its ports. Without visible
    // feedback people click Start again, and a second server on the same port
    // spins retrying its sockets until it exhausts memory. Disable the button
    // for the whole startup window and say what is happening.
    start.onclick = async () => {
      start.disabled = true;
      const was = start.textContent;
      let left = 30;
      start.textContent = `starting… ${left}s`;
      const tick = setInterval(() => {
        left -= 1;
        start.textContent = left > 0 ? `starting… ${left}s` : 'starting…';
      }, 1000);
      const r = await api('server/start', { id: prof.id });
      if (!r.ok) {
        clearInterval(tick);
        start.disabled = false; start.textContent = was;
        toast(r.error || 'Could not start', true);
        return;
      }
      toast('Starting — this takes about 30 seconds');
      setTimeout(() => { clearInterval(tick); serversPage(); }, 31000);
    };
    // ⚠ Stops THIS server, by its own pid or its own HTTP port. "Stop all" is
    // still there, but it should be a choice rather than the only option -
    // with several servers up, stopping one meant ending everyone's session.
    const stop = el('button', 'sm danger', 'Stop');
    stop.title = 'Stop only this server';
    stop.onclick = async () => {
      stop.disabled = true;
      const r = await api('server/stop', { id: prof.id });
      stop.disabled = false;
      toast(r.ok ? `Stopped ${prof.name}` : (r.error || 'Could not stop'),
            !r.ok);
      setTimeout(serversPage, 900);
    };

    const edit = el('button', 'sm', 'Edit');
    edit.onclick = () => { editing = { ...prof }; serversPage(); };
    const logs = el('button', 'sm', 'Logs');
    const pre = el('pre', 'log');
    pre.style.display = 'none';
    logs.onclick = async () => {
      pre.style.display = pre.style.display === 'none' ? '' : 'none';
      if (pre.style.display === '') {
        const r = await api('server/log?id=' + prof.id);
        pre.textContent = (r.lines || []).join('\n') || r.error || '(empty)';
        pre.scrollTop = pre.scrollHeight;
      }
    };
    const del = el('button', 'sm danger', 'Delete');
    del.onclick = async () => {
      if (!confirm('Delete "' + prof.name + '"?')) return;
      await api('profiles/delete', { id: prof.id }); editing = null; serversPage();
    };
    // --- per-server telemetry -------------------------------------------
    // Each server gets its OWN tracker on its OWN port, bound to that server's
    // process, so several can be watched at once.
    const ts = (telState || {})[prof.id] || {};
    const tpill = el('span', 'pill ' + (ts.running ? 'on' : 'off'),
      `<i class="dot"></i>telemetry ${ts.running ? 'on :' + ts.port : 'off'}`);
    const tOn = el('button', 'sm', ts.running ? 'Restart telemetry' : 'Start telemetry');
    tOn.onclick = async () => {
      const r = await api('telemetry/start', { id: prof.id });
      toast(r.ok ? `Telemetry on port ${r.port}` : (r.error || 'Failed'), !r.ok);
      setTimeout(serversPage, 1800);
    };
    const tOff = el('button', 'sm danger', 'Stop telemetry');
    tOff.disabled = !ts.running;
    tOff.onclick = async () => {
      await api('telemetry/stop', { id: prof.id });
      toast('Telemetry stopped'); setTimeout(serversPage, 800);
    };
    const tView = el('button', 'sm primary', 'View map');
    tView.disabled = !ts.running;
    tView.onclick = () => { telProfile = prof.id; telTrack = null; go('telemetry'); };
    const wAI = el('button', 'sm', 'Attach AI worker');
    wAI.title = 'One client joins this server with -ai_player_car (AiDriverEvo), not vAI ghosts';
    wAI.onclick = async () => {
      const r = await api('game/attach_worker', { id: prof.id, ai_player: true });
      toast(r.ok ? (r.hint || 'Worker launching') : (r.error || 'Failed'), !r.ok);
      setTimeout(serversPage, 2000);
    };
    row.append(start, stop, edit, logs, del, tOn, tOff, tView, wAI, tpill);
    card.append(row, pre);
    p.append(card);

    api('server/status?id=' + prof.id).then(s => {
      const pill = $(`[data-st="${prof.id}"]`);
      if (!pill) return;
      pill.className = 'pill ' + (s.running ? 'on' : 'off');
      pill.innerHTML = '<i class="dot"></i>' + (s.running
        ? `running${s.clients != null ? ' · ' + s.clients + ' clients' : ''}`
        : 'stopped');
    });
  }
}

function editor(prof, trk, opts, extra) {
  const c = el('div', 'card');
  c.innerHTML = `<h2>${prof.id ? 'Edit server' : 'New server'}</h2>`;

  // One field builder for every input type, so adding an option is one line.
  const mk = (parent, key, label, kind, extra) => {
    const l = el('label', 'f', `<span>${label}</span>`);
    let inp;
    if (kind === 'select') {
      inp = el('select');
      (extra || []).forEach(o => {
        const val = (o && o.value !== undefined) ? o.value : o;
        const txt = (o && o.label !== undefined) ? o.label : String(o).replace(/_/g, ' ');
        const op = el('option', null, esc(txt));
        op.value = val;
        if (String(val) === String(prof[key])) op.selected = true;
        inp.append(op);
      });
      inp.onchange = () => {
        const v = inp.value;
        prof[key] = (extra && extra.length && typeof extra[0] === 'object'
                     && extra[0].value !== undefined) ? Number(v) : v;
      };
    } else if (kind === 'bool') {
      l.className = 'ctl';
      l.innerHTML = '';
      inp = el('input'); inp.type = 'checkbox'; inp.checked = !!prof[key];
      inp.onchange = () => { prof[key] = inp.checked; };
      l.append(inp, el('span', null, esc(label)));
      parent.append(l);
      return;
    } else {
      inp = el('input');
      inp.type = kind === 'number' ? 'number' : (kind === 'password' ? 'password' : 'text');
      if (kind === 'number' && extra) { inp.step = extra.step || 1; }
      inp.value = prof[key] ?? '';
      inp.oninput = () => {
        prof[key] = kind === 'number' ? Number(inp.value) : inp.value;
      };
    }
    l.append(inp);
    parent.append(l);
  };

  // ⚠ Two tiers. Nearly every server needs six fields; the other forty are
  // things you set once a year. Showing all of them flat, equally weighted, is
  // what made this page hard to read - so the rare ones fold away and the page
  // opens on what you actually came to change.
  // ⚠ Every section stays VISIBLE as a heading; the rare ones just start
  // folded. Bundling them into a single "Everything else" box hid the fact
  // that AI, weather, penalties and ports exist at all - which is worse than
  // the flat wall of fields it replaced, because now you cannot even find
  // them. One labelled fold each: scannable, and nothing is a mystery.
  const section = (title, hint, rare) => {
    const g = el('div', 'grid g2');
    g.style.margin = '6px 0 12px';
    if (!rare) {
      c.append(el('div', 'tiny dim',
        `<b style="color:var(--fg)">${esc(title)}</b>`
        + (hint ? ` — ${hint}` : '')));
      c.append(g);
      return g;
    }
    const d = el('details');
    d.style.margin = '4px 0 10px';
    d.append(el('summary', 'tiny dim',
      `<b style="color:var(--fg)">${esc(title)}</b>`
      + (hint ? ` — ${hint}` : '')));
    d.append(g);
    c.append(d);
    return g;
  };

  /* A dropdown instead of typing an id.

     ⚠ `track_label` used to be free text, and it has to match a track the
     server can actually resolve - a typo produces a server that starts and
     then cannot load its own track. The options come from YOUR tracks.table,
     so anything listed is real. */
  const trackPicker = (parent, key, label, choices, blank) => {
    const l = el('label', 'f', `<span>${label}</span>`);
    const s = el('select');
    const b = el('option', null, blank);
    b.value = '';
    s.append(b);
    choices.forEach(([name, folder]) => {
      const o = el('option', null, `${name}  (${folder})`);
      o.value = name;
      if (name === prof[key]) o.selected = true;
      s.append(o);
    });
    // a value saved before this dropdown existed must not vanish silently
    if (prof[key] && !choices.some(([n]) => n === prof[key])) {
      const o = el('option', null, `${prof[key]}  (not installed here)`);
      o.value = prof[key];
      o.selected = true;
      s.append(o);
    }
    s.onchange = () => { prof[key] = s.value; };
    l.append(s);
    parent.append(l);
  };

  let g = section('Identity');
  mk(g, 'name', 'Server name', 'text');

  /* ⚠ ONE track chooser, listing stock events and deployed tracks together.
     They are selected by different mechanisms - an index into events_*.json
     versus a name the server resolves through tracks.table - and exposing that
     as two controls let a profile hold both, with nothing saying which wins.
     That is exactly how "Highland drift" came to host Nurburgring. */
  {
    const l = el('label', 'f', '<span>Track / layout</span>');
    const s = el('select');
    const stock = el('optgroup');
    stock.label = 'Stock tracks';
    trk.forEach(t => {
      const o = el('option', null, esc(t.label));
      o.value = 'idx:' + t.index;
      if (!prof.custom_track && String(t.index) === String(prof.track_index))
        o.selected = true;
      stock.append(o);
    });
    s.append(stock);
    const mods = (extra && extra.modTracks) || [];
    if (mods.length) {
      const og = el('optgroup');
      og.label = 'Deployed tracks';
      mods.forEach(([name, folder]) => {
        const o = el('option', null, `${name}  (${folder})`);
        o.value = 'custom:' + name;
        if (prof.custom_track === name) o.selected = true;
        og.append(o);
      });
      s.append(og);
    }
    // a track saved before this existed, or since uninstalled, must not vanish
    if (prof.custom_track && !mods.some(([n]) => n === prof.custom_track)) {
      const o = el('option', null,
                   `${prof.custom_track}  (not installed here)`);
      o.value = 'custom:' + prof.custom_track;
      o.selected = true;
      s.append(o);
    }
    s.onchange = () => {
      const v = s.value;
      if (v.startsWith('custom:')) {
        prof.custom_track = v.slice(7);
      } else {
        prof.custom_track = '';
        prof.track_index = Number(v.slice(4));
      }
    };
    l.append(s);
    g.append(l);
  }

  g = section('Session', 'what the server actually runs');
  mk(g, 'game_mode', 'Game mode', 'select', opts.game_mode);
  mk(g, 'session_type', 'Listing type', 'select', opts.session_type);
  mk(g, 'practice_duration', 'Session duration (min)', 'number');
  mk(g, 'max_players', 'Player slots', 'number');
  mk(g, 'cycle', 'Cycle sessions', 'bool');

  g = section('AI', 'skill min/max spread the field — equal skill makes them clump');
  mk(g, 'ai', 'AI cars', 'number');
  mk(g, 'skill_min', 'Skill min', 'number');
  mk(g, 'skill_max', 'Skill max', 'number');

  g = section('Time & weather', 'time multiplier 0 freezes the clock');
  mk(g, 'tod_hour', 'Time of day (hour)', 'number');
  mk(g, 'tod_minute', 'Minute', 'number');
  mk(g, 'time_mult', 'Time multiplier', 'number');
  mk(g, 'weather', 'Weather', 'select', opts.weather);
  mk(g, 'weather_behaviour', 'Weather behaviour', 'select', opts.weather_behaviour);
  mk(g, 'grip', 'Initial grip', 'select', opts.grip);

  g = section('Rules');
  mk(g, 'tuning', 'Tuning', 'select', opts.tuning);
  mk(g, 'pi_min', 'PI min (0 = no limit)', 'number', { step: 0.1 });
  mk(g, 'pi_max', 'PI max (0 = no limit)', 'number', { step: 0.1 });

  g = section('Access', 'leave blank for an open server');
  mk(g, 'driver_password', 'Driver password', 'password');
  mk(g, 'spectator_password', 'Spectator password', 'password');
  mk(g, 'admin_password', 'Admin password', 'password');

  g = section('Session pacing', 'both were hardcoded to 10 before', true);
  mk(g, 'overtime_wait', 'Overtime wait, next session (s)', 'number');
  mk(g, 'max_wait_to_box', 'Max wait to box (s)', 'number');

  g = section('In-game date', 'the clock starts here; multiplier 0 freezes it', true);
  mk(g, 'tod_year', 'Year', 'number');
  mk(g, 'tod_month', 'Month', 'number');
  mk(g, 'tod_day', 'Day', 'number');
  mk(g, 'tod_second', 'Second', 'number');

  g = section('Visibility & output');
  mk(g, 'no_lobby', 'Private — do NOT list in the server browser', 'bool');
  mk(g, 'write_results', 'Write server results', 'bool');
  mk(g, 'export_json', 'Export season JSON', 'bool');

  g = section('Handicaps', 'per car: name:ballast:restrictor, comma separated', true);
  mk(g, 'car_handicaps', 'Car handicaps', 'text');

  // ⚠ Display only. Which track the server HOSTS is chosen once, above; this
  // just corrects the NAME when a track is sitting in a borrowed stock slot,
  // so maps and labels do not report the host's name.
  g = section('Slot label',
    'Only for a borrowed-slot install: what is really in that stock slot. ' +
    'A native install already reports its own name.', true);
  trackPicker(g, 'track_label', 'Really running',
              (extra && extra.modTracks) || [],
              'Nothing borrowed — report the track as-is');

  // Which cars this server accepts. Modded cars are the ones worth picking
  // deliberately: a mod nobody else has is the usual reason a join is refused.
  g = section('Cars allowed',
    'Empty means every Kunos car plus every installed mod.');
  const cars = ((extra && extra.cat && extra.cat.cars) || []);
  const chosen = new Set(prof.cars || []);
  const box = el('div');
  box.style.cssText = 'grid-column:1/-1';
  const chips = el('div', 'row wrap');
  const redraw = () => {
    chips.innerHTML = '';
    if (!chosen.size) {
      chips.append(el('span', 'tiny dim', 'Every car is allowed'));
    } else {
      [...chosen].forEach(id => {
        const meta = cars.find(x => x.id === id);
        const b = el('button', 'sm',
          `${esc(meta ? meta.label : id)} ✕`);
        b.title = id;
        b.onclick = () => { chosen.delete(id); prof.cars = [...chosen]; redraw(); };
        chips.append(b);
      });
      const clr = el('button', 'sm danger', 'Allow every car');
      clr.onclick = () => { chosen.clear(); prof.cars = []; redraw(); };
      chips.append(clr);
    }
  };
  /* Pick cars by looking at them.

     ⚠ A <select> cannot show a picture, and an id like preset_695b_mech_1
     tells you nothing about which car it is. These are the same Vulkan renders
     the gallery uses, keyed by MODEL - the catalogue is keyed by preset, and
     several presets share one model, so the thumbnail comes from x.model.

     Modded and Kunos are deliberately separate lists: mods are the ones you
     choose on purpose, and 11 of them were previously lost among 97. */
  const panels = el('div');
  panels.style.cssText = 'grid-column:1/-1';
  const panel = (title, list, open) => {
    if (!list.length) return;
    const d = el('details');
    if (open) d.open = true;
    d.style.margin = '4px 0';
    d.append(el('summary', 'tiny dim', `${title} — ${list.length}`));
    const wrap = el('div');
    wrap.style.cssText = 'display:grid;gap:6px;margin:8px 0 4px;'
      + 'grid-template-columns:repeat(auto-fill,minmax(104px,1fr));'
      + 'max-height:260px;overflow:auto';
    list.forEach(x => {
      const t = el('div');
      const on = () => chosen.has(x.id);
      const paint = () => {
        t.style.cssText = 'cursor:pointer;border-radius:6px;overflow:hidden;'
          + 'border:1px solid ' + (on() ? 'var(--accent)' : 'var(--line,#262b31)')
          + ';background:var(--card,#16191d)';
      };
      const img = el('img');
      img.loading = 'lazy';
      img.alt = '';   // blank, not a broken-image glyph + caption
      img.style.cssText = 'width:100%;display:block;aspect-ratio:3/2;'
        + 'object-fit:cover;background:#0c0e11';
      img.src = 'api/thumb/car?id=' + encodeURIComponent(x.model || x.id);
      // a car with no render yet (server-only mods have no client package)
      // shows a blank tile rather than a broken image
      img.onerror = () => { img.style.visibility = 'hidden'; };
      const cap = el('div', 'tiny');
      cap.style.cssText = 'padding:4px 6px;line-height:1.25';
      cap.textContent = x.label;
      // ⚠ No model means no geometry on THIS machine - the car is named in
      // cars.json but its package is not installed, so there is nothing to
      // render. Say so, or the blank tile reads as a broken thumbnail.
      if (!x.model) {
        const w = el('div', 'tiny dim');
        w.textContent = 'not installed here';
        cap.append(w);
      }
      t.append(img, cap);
      t.title = x.model ? x.id : x.id + ' — no content installed for this car';
      t.onclick = () => {
        if (on()) chosen.delete(x.id); else chosen.add(x.id);
        prof.cars = [...chosen];
        paint();
        redraw();
      };
      paint();
      wrap.append(t);
    });
    d.append(wrap);
    panels.append(d);
  };
  panel('Modded cars', cars.filter(x => x.mod), true);
  panel('Kunos cars', cars.filter(x => !x.mod), false);

  box.append(chips, panels);
  g.append(box);
  redraw();

  g = section('Penalties',
    'Per server — carried in this server’s season blob, so no game files ' +
    'are modified. Accepted by the server, but enforcement is unverified: ' +
    'test it by cutting a corner on track.');
  mk(g, 'penalties', 'Enable custom penalties', 'bool');
  mk(g, 'car_cut_tyres_out', 'Wheels off track to count as a cut (1-4)', 'number');
  mk(g, 'warning_trigger_countdown', 'Warnings before the penalty', 'number');
  mk(g, 'time_penalty_ms', 'Time penalty (ms)', 'number');

  g = section('Ports & files', 'internal ports default to the listener port');
  mk(g, 'tcp_port', 'TCP/UDP port', 'number');
  mk(g, 'http_port', 'HTTP status port', 'number');
  mk(g, 'tcp_internal_port', 'TCP internal (0 = same)', 'number');
  mk(g, 'udp_internal_port', 'UDP internal (0 = same)', 'number');
  mk(g, 'entry_list_path', 'Entry list path', 'text');
  mk(g, 'results_path', 'Results path', 'text');
  mk(g, 'entry_list_url', 'Entry list URL', 'text');
  mk(g, 'results_post_url', 'Results POST URL', 'text');
  mk(g, 'log', 'Log file', 'text');
  mk(g, 'telemetry', 'Start telemetry with this server', 'bool');

  const row = el('div', 'row');
  const save = el('button', 'primary', 'Save');
  save.onclick = async () => {
    await api('profiles/save', prof); editing = null; toast('Saved'); serversPage();
  };
  const cancel = el('button', null, 'Cancel');
  cancel.onclick = () => { editing = null; serversPage(); };
  row.append(save, cancel);
  c.append(row);
  return c;
}

/* ----------------------------------------------------------------- cars -- */

/* The 3D viewer picker. These are the cars inside the CLIENT package, which is
   a different set from cars.json above: that lists what the dedicated server
   can load, this lists what actually has geometry to show. */
let viewFilter = '';
async function viewerCard(p) {
  const card = el('div', 'card');
  card.innerHTML = '<h2>3D viewer</h2>'
    + '<div class="tiny dim" style="margin-bottom:9px">Opens a car in the '
    + 'model viewer. Cars are read straight out of the game package - '
    + 'nothing is extracted or written to disk.</div>';
  p.append(card);

  const st = await api('viewer/status');
  if (!st.package) {
    card.append(el('div', 'empty',
      'Game install not found — set the game folder in Settings.'));
    return;
  }
  if (!st.exe) {
    card.append(el('div', 'empty',
      'evoview.exe not found. Put it in the tools folder next to ACECM, '
      + 'or set viewer_exe in Settings.'));
    return;
  }

  const search = el('input');
  search.placeholder = 'Search cars to view…';
  search.value = viewFilter;
  card.append(search);
  const list = el('div', 'list');
  list.style.marginTop = '10px';
  card.append(list);
  list.append(el('div', 'empty', 'Reading the game package…'));

  const d = await api('viewer/cars');
  if (d.error) {
    list.innerHTML = '';
    list.append(el('div', 'empty', esc(d.error)));
    return;
  }
  search.oninput = () => { viewFilter = search.value.toLowerCase(); draw(); };

  function draw() {
    list.innerHTML = '';
    const rows = (d.cars || []).filter(c =>
      !viewFilter || c.label.toLowerCase().includes(viewFilter)
      || c.id.toLowerCase().includes(viewFilter));
    if (!rows.length) { list.append(el('div', 'empty', 'No matches')); return; }
    rows.slice(0, 400).forEach(car => {
      const r = el('div', 'chk');
      r.innerHTML = `<span class="name">${esc(car.label)}</span>`
        + (car.mod ? '<span class="pill">mod</span>' : '')
        + `<span class="id">${esc(car.id)}</span>`;
      const b = el('button', 'small');
      b.textContent = 'View';
      b.onclick = async () => {
        b.disabled = true;
        b.textContent = 'Opening…';
        await api('viewer/open', { id: car.id });
        // Extraction runs in the background; report what it is doing rather
        // than leaving a dead button.
        const poll = setInterval(async () => {
          const j = await api('viewer/job?id=' + encodeURIComponent(car.id));
          if (j.state === 'extracting') b.textContent = j.detail || 'Extracting…';
          if (j.state === 'open' || j.state === 'ready') {
            clearInterval(poll);
            b.textContent = 'View';
            b.disabled = false;
          }
          if (j.state === 'error') {
            clearInterval(poll);
            b.textContent = 'Failed';
            b.disabled = false;
            toast(j.detail || 'could not open the viewer');
          }
        }, 700);
      };
      r.append(b);
      list.append(r);
    });
  }
  draw();
}

let carFilter = '';
/* Every car as a real Vulkan render.

   The pictures come from evoview rendering each car out of the archive, the
   same path the full 74-car sweep used - so this is the actual car with its
   own paint, rims and tyres, not a stock photo. Renders are cached on disk and
   made once; a tile with no render yet simply stays blank rather than blocking
   the page while ~2 s of GPU work happens per car. */
async function carGallery(p) {
  const cars = (await api('viewer/cars')).cars || [];
  const st = await api('thumbs/status');
  const have = new Set(st.have || []);
  const cat = await api('cars');
  const profs = (await api('profiles')).profiles || [];

  // ⚠ Two different ids. The gallery is keyed by MODEL (ks_abarth_695_biposto)
  // because that is what has a render; a server's allowed list is keyed by
  // PRESET (preset_695b_mech_1), and one model usually has several. Allowing
  // "a car" therefore means allowing every preset of that model - toggling one
  // preset would leave the car half-allowed and the difference is invisible.
  const byModel = {};
  (cat.cars || []).forEach(x => {
    (byModel[x.model] = byModel[x.model] || []).push(x);
  });

  let prof = profs.find(x => x.id === galProfile) || null;
  const allowed = () => new Set(prof ? (prof.cars || []) : []);

  const c = el('div', 'card');
  c.innerHTML = `<h2>Car gallery &middot; ${cars.length} cars</h2>`;
  const row = el('div', 'row wrap');
  const search = el('input');
  search.placeholder = 'Filter cars…';
  search.value = galFilter;
  search.oninput = () => { galFilter = search.value.toLowerCase(); draw(); };

  // Which server profile are we editing the allowed list of?
  const psel = el('select');
  const none = el('option', null, 'Allowed cars: pick a profile…');
  none.value = '';
  psel.append(none);
  profs.forEach(x => {
    const o = el('option', null, x.name);
    o.value = x.id;
    if (x.id === galProfile) o.selected = true;
    psel.append(o);
  });
  psel.onchange = () => {
    galProfile = psel.value;
    prof = profs.find(x => x.id === galProfile) || null;
    draw();
  };
  row.append(psel);

  async function saveAllowed(list) {
    prof.cars = list;
    const r = await api('profiles/save', prof);
    if (r && r.error) { toast(r.error, true); return; }
    toast(list.length ? `${list.length} preset(s) allowed`
                      : 'Every car allowed');
    draw();
  }
  const all = el('button', 'sm', 'Allow all');
  all.onclick = () => prof && saveAllowed([]);
  const onlyMods = el('button', 'sm', 'Only mods');
  onlyMods.onclick = () => prof && saveAllowed(
    (cat.cars || []).filter(x => x.mod).map(x => x.id));
  row.append(all, onlyMods);
  const build = el('button', 'sm primary',
    `Render missing (${cars.length - have.size})`);
  build.onclick = async () => {
    const r = await api('thumbs/build', {});
    if (!r.ok) { toast(r.error || 'busy', true); return; }
    toast('Rendering cars — this runs in the background');
    const poll = setInterval(async () => {
      const j = await api('thumbs/status');
      build.textContent = j.state === 'running'
        ? `Rendering ${j.done}/${j.total} — ${j.current}` : 'Render missing';
      if (j.state !== 'running') {
        clearInterval(poll);
        toast(`${j.made} car render(s) made`);
        carsPage();
      }
    }, 1500);
  };
  row.append(search, build);
  c.append(row);
  const grid = el('div', 'grid g3');
  grid.style.marginTop = '10px';
  c.append(grid);
  p.append(c);

  function draw() {
    grid.innerHTML = '';
    cars.filter(x => !galFilter
        || (x.label + ' ' + x.id).toLowerCase().includes(galFilter))
      .forEach(x => {
        const card = el('div', 'stat');
        card.style.cssText = 'padding:0;overflow:hidden;text-align:left';
        const img = el('img');
        img.loading = 'lazy';
        img.alt = '';   // blank, not a broken-image glyph + caption
        img.style.cssText = 'width:100%;display:block;aspect-ratio:3/2;'
          + 'object-fit:cover;background:#0c0e11';
        img.src = 'api/thumb/car?id=' + encodeURIComponent(x.id);
        // no render yet -> leave the tile blank rather than show a broken image
        img.onerror = () => { img.style.visibility = 'hidden'; };
        const cap = el('div');
        cap.style.cssText = 'padding:8px 10px';
        const presets = byModel[x.id] || [];
        cap.innerHTML = `<b style="font-size:13px">${esc(x.label)}</b>`
          + (x.mod ? ' <span class="pill">mod</span>' : '')
          + `<div class="tiny dim">${esc(x.id)}`
          + (presets.length > 1 ? ` · ${presets.length} presets` : '')
          + '</div>';
        const open = el('button', 'sm', 'View in 3D');
        open.onclick = async () => {
          await api('viewer/open', { id: x.id });
          toast('Opening ' + x.label + ' in the viewer');
        };
        cap.append(open);

        if (prof && presets.length) {
          const cur = allowed();
          const on = presets.every(q => cur.has(q.id));
          const t = el('button', on ? 'sm primary' : 'sm',
                       on ? 'Allowed' : 'Allow');
          t.title = cur.size ? '' :
            'This profile allows every car — allowing one starts a whitelist';
          t.onclick = () => {
            const set = allowed();
            // an empty list means "everything"; the first explicit pick has to
            // start from every car, or one click would silently ban the rest
            if (!set.size) (cat.cars || []).forEach(q => set.add(q.id));
            presets.forEach(q => on ? set.delete(q.id) : set.add(q.id));
            saveAllowed([...set]);
          };
          cap.append(t);
        }
        card.append(img, cap);
        grid.append(card);
      });
    if (!grid.children.length) grid.append(el('div', 'empty', 'No matches'));
  }
  draw();
}

let galFilter = '', galProfile = '';

/* Installed car mods, and getting rid of one.

   ⚠ A car mod has TWO homes: the client's folder so you can drive it, and the
   dedicated server's so it can host it. A mod present on one side only loads
   for one of you, which surfaces as a join rejection rather than anything
   naming the mod - so both sides are shown per mod rather than a single tick. */
async function modStrip(p) {
  const [srv, cli] = [await api('mods?side=server'), await api('mods?side=client')];
  const names = [...new Set([...(srv.mods || []), ...(cli.mods || [])]
    .map(m => m.name))].sort();
  const c = el('div', 'card');
  c.innerHTML = `<h2>Car mods &middot; ${names.length}</h2>`;
  const add = el('div', 'row wrap');
  const path = el('input');
  path.placeholder = 'Folder or .zip holding <mod>.kspkg + <mod>.json…';
  path.style.minWidth = '340px';
  const go = el('button', 'sm primary', 'Install');
  go.onclick = async () => {
    if (!path.value.trim()) { toast('Give a path first', true); return; }
    toast('Installing…');
    const r = await api('mods/install', { path: path.value.trim() });
    toast(r.ok === false ? (r.error || 'Install failed')
                         : 'Installed — check both sides below', r.ok === false);
    carsPage();
  };
  add.append(path, go);
  c.append(add);

  if (!names.length) {
    c.append(el('div', 'empty', 'No car mods installed'));
  } else {
    const t = el('table');
    t.innerHTML = '<thead><tr><th>Mod</th><th>Client</th><th>Server</th>'
      + '<th></th></tr></thead>';
    const tb = el('tbody');
    const has = (list, n) => (list || []).some(m => m.name === n);
    names.forEach(n => {
      const tr = el('tr');
      const onC = has(cli.mods, n), onS = has(srv.mods, n);
      tr.innerHTML = `<td>${esc(n)}</td>`
        + `<td><span class="pill ${onC ? 'on' : 'off'}"><i class="dot"></i>`
        + `${onC ? 'yes' : 'no'}</span></td>`
        + `<td><span class="pill ${onS ? 'on' : 'off'}"><i class="dot"></i>`
        + `${onS ? 'yes' : 'no'}</span></td>`;
      const td = el('td');
      const rm = el('button', 'sm danger', 'Remove');
      rm.onclick = async () => {
        if (!confirm(`Remove "${n}" from both sides?`)) return;
        const r = await api('mods/remove', { name: n });
        toast(r && r.error ? r.error : 'Removed', !!(r && r.error));
        carsPage();
      };
      td.append(rm);
      tr.append(td);
      tb.append(tr);
    });
    t.append(tb);
    c.append(t);
  }
  p.append(c);
}

async function carsPage() {
  const d = await api('cars');
  const p = $('#page');
  p.innerHTML = '';
  // ⚠ The old text-list viewer card used to live here. The gallery does the
  // same job with a picture of each car and the same "View in 3D" button, so
  // keeping both meant two lists of 74 cars and a screenful of duplication
  // before you reached anything new. viewerCard() is still defined for reuse.
  await carGallery(p);
  await modStrip(p);
  const c = el('div', 'card');
  c.innerHTML = `<h2>Cars &middot; ${d.total ?? 0} total, `
    + `${d.kunos ?? 0} Kunos, ${d.mods ?? 0} modded</h2>`;
  const search = el('input');
  search.placeholder = 'Search cars…';
  search.value = carFilter;
  search.oninput = () => { carFilter = search.value.toLowerCase(); render(); };
  c.append(search);
  const list = el('div', 'list');
  list.style.marginTop = '10px';
  c.append(list);
  p.append(c);

  // Real model names harvested from the server log. cars.json ids are mostly
  // preset_<code>, which cannot be expanded into a model name without a lookup
  // we do not have - so show both truthfully rather than guess a mapping.
  api('models').then(m => {
    if (!m || !m.total) return;
    const mc = el('div', 'card');
    mc.innerHTML = `<h2>Models seen on this server &middot; ${m.total}</h2>`
      + '<div class="tiny dim" style="margin-bottom:9px">Harvested from join '
      + 'records in the server log — these are full model names. The ids above '
      + 'come from cars.json and use a different scheme; the two are not '
      + 'mapped to each other.</div>';
    const l = el('div', 'list');
    m.models.forEach(x => l.append(el('div', 'chk',
      `<span class="name">${esc(x.label)}</span>`
      + `<span class="id">${esc(x.id)}</span>`)));
    mc.append(l);
    p.append(mc);
  });

  if (d.mods)
    p.append(el('div', 'card', '<h2>Modded content</h2>'
      + `<div class="tiny dim">${d.mods} car(s) are not Kunos presets. The `
      + 'dedicated server\'s content package does not contain them, so a player '
      + 'picking one gets a broken join. They are excluded from server car '
      + 'lists by default.</div>'));

  function render() {
    list.innerHTML = '';
    const rows = (d.cars || []).filter(c =>
      !carFilter || c.label.toLowerCase().includes(carFilter)
      || c.id.toLowerCase().includes(carFilter));
    if (!rows.length) { list.append(el('div', 'empty', 'No matches')); return; }
    rows.slice(0, 400).forEach(car => {
      const r = el('div', 'chk');
      r.innerHTML = `<span class="name">${esc(car.label)}</span>`
        + (car.mod ? '<span class="pill warn">mod</span>' : '')
        + `<span class="id">${esc(car.id)}</span>`;
      list.append(r);
    });
  }
  render();
}

/* --------------------------------------------------------------- tracks -- */
async function tracksPage() {
  const d = await api('tracks');
  const p = $('#page');
  p.innerHTML = '';

  // Every track the game can load, with the cover art it ships. Only some
  // tracks have one; the rest show a blank tile rather than a placeholder
  // pretending to be a photo.
  const loc = await api('browser/local');
  const map = loc.track_map || {};
  const gal = el('div', 'card');
  const st = await api('thumbs/status');
  const shipped = (st.covers || []).length;
  const done = (st.covers_have || []).length;
  gal.innerHTML = `<h2>Tracks &middot; ${Object.keys(map).length}</h2>`
    + '<div class="tiny dim" style="margin-bottom:9px">From your own '
    + 'tracks.table — imported tracks included. Cover art is whatever the game '
    + 'ships: most of it is a compressed texture rather than an image file, so '
    + 'it has to be decoded once. Imported tracks usually have none.</div>';

  // ⚠ Decoding is ~2s per track and the tiles do it one at a time as they
  // scroll into view, so the first visit trickles. One button up front turns
  // that into a single wait.
  const drow = el('div', 'row wrap');
  const dec = el('button', 'sm primary',
    done >= shipped ? 'Re-decode covers' : `Decode all covers (${shipped - done} left)`);
  dec.onclick = async () => {
    const r = await api('thumbs/covers', { force: done >= shipped });
    if (!r.ok) { toast(r.error || 'busy', true); return; }
    dec.disabled = true;
    const poll = setInterval(async () => {
      const j = (await api('thumbs/status')).cover_job || {};
      dec.textContent = j.state === 'running'
        ? `Decoding ${j.done}/${j.total} — ${j.current}` : 'Decode all covers';
      if (j.state !== 'running') {
        clearInterval(poll);
        toast(`${j.made} cover(s) ready in ${j.seconds}s`);
        tracksPage();
      }
    }, 1200);
  };
  drow.append(dec, el('span', 'tiny dim',
    `${done}/${shipped} decoded`));
  gal.append(drow);

  const grid = el('div', 'grid g3');
  grid.style.marginTop = '10px';
  Object.entries(map).sort((a, b) => a[0].localeCompare(b[0]))
    .forEach(([name, folder]) => {
      const card = el('div', 'stat');
      card.style.cssText = 'padding:0;overflow:hidden;text-align:left';
      const img = el('img');
      img.loading = 'lazy';
      img.alt = '';   // blank, not a broken-image glyph + caption
      img.style.cssText = 'width:100%;display:block;aspect-ratio:16/9;'
        + 'object-fit:cover;background:#0c0e11';
      img.src = 'api/thumb/track?folder=' + encodeURIComponent(folder);
      img.onerror = () => { img.style.visibility = 'hidden'; };   // leave it blank
      const cap = el('div');
      cap.style.cssText = 'padding:8px 10px';
      cap.innerHTML = `<b style="font-size:13px">${esc(name)}</b>`
        + `<div class="tiny dim">${esc(folder)}</div>`;
      card.append(img, cap);
      grid.append(card);
    });
  gal.append(grid);
  p.append(gal);

  const c = el('div', 'card');
  c.innerHTML = `<h2>Hostable layouts &middot; ${d.total ?? 0}</h2>`;
  const tb = el('table');
  tb.innerHTML = '<thead><tr><th>#</th><th>Track</th><th>Layout</th>'
    + '<th>Length</th></tr></thead>';
  const body = el('tbody');
  (d.tracks || []).forEach(t => {
    const tr = el('tr');
    tr.innerHTML = `<td class="dim">${t.index}</td><td>${esc(t.label)}</td>`
      + `<td class="tiny dim">${esc(t.layout)}</td>`
      + `<td class="tiny dim">${t.length_m ? Math.round(t.length_m) + ' m' : '—'}</td>`;
    body.append(tr);
  });
  tb.append(body);
  c.append(tb);
  p.append(c);
}

/* -------------------------------------------------------------- backend -- */
async function backendPage() {
  const b = await api('backend');
  const p = $('#page');
  p.innerHTML = '';

  const c = el('div', 'card');
  c.innerHTML = '<h2>Own lobby backend</h2>'
    + '<div class="tiny dim" style="margin-bottom:12px">'
    + 'EVO has no direct connect: the client asks a lobby to resolve a server '
    + 'and authorise the join. So we run the lobby. <b>Proxy</b> relays to Kunos '
    + 'and appends our servers, keeping your account and the public list. '
    + '<b>Standalone</b> replaces it entirely, fully offline.</div>';
  const row = el('div', 'row wrap');
  const proxy = el('button', 'primary', 'Start proxy');
  proxy.onclick = async () => { const r = await api('backend/start', { mode: 'proxy' }); if (r.ok) toast('Proxy up on :' + b.port); backendPage(); };
  const alone = el('button', null, 'Start standalone');
  alone.onclick = async () => { const r = await api('backend/start', { mode: 'standalone' }); if (r.ok) toast('Standalone backend up'); backendPage(); };
  const stop = el('button', 'danger', 'Stop');
  stop.onclick = async () => { await api('backend/stop', {}); toast('Backend stopped'); backendPage(); };
  row.append(proxy, alone, stop);
  c.append(row);
  c.append(el('div', 'row wrap', `<span class="pill ${b.listening ? 'on' : 'off'}">`
    + `<i class="dot"></i>port ${b.port} ${b.listening ? 'listening' : 'closed'}</span>`
    + `<span class="pill ${b.have_cert ? 'on' : 'warn'}"><i class="dot"></i>`
    + `${b.have_cert ? 'TLS keypair present' : 'no TLS keypair — run backend/gencert.sh'}</span>`
    + `<span class="pill ${b.client_patched ? 'on' : 'off'}"><i class="dot"></i>`
    + `${b.client_patched ? 'rdata → local backend' : 'rdata still Kunos'}</span>`));

  const cu = b.client_url || {};
  const red = el('div');
  red.style.marginTop = '12px';
  red.innerHTML = '<div class="tiny dim" style="margin-bottom:8px">'
    + 'Launch already passes <code>-backend=</code>. The rdata write is the '
    + 'fallback when Steam ate the flag. Close the game first. '
    + `Intended: <code>${esc(b.launch_backend || cu.intended || '')}</code>`
    + (cu.slot ? ` &middot; slot ${esc(cu.slot)}` : '')
    + '</div>';
  const rrow = el('div', 'row wrap');
  const ap = el('button', b.client_patched ? '' : 'primary', 'Point client at us');
  ap.onclick = async () => {
    const r = await api('backend/redirect', { action: 'apply' });
    toast(r.ok ? (r.already ? 'Already pointed at us' : 'Client URL patched')
               : (r.error || 'Patch failed'), !r.ok);
    backendPage();
  };
  const rs = el('button', null, 'Restore Kunos URL');
  rs.onclick = async () => {
    if (!confirm('Put the official lobby URL back in the client?')) return;
    const r = await api('backend/redirect', { action: 'restore' });
    toast(r.ok ? (r.already ? 'Already on Kunos' : 'Restored official URL')
               : (r.error || 'Restore failed'), !r.ok);
    backendPage();
  };
  rrow.append(ap, rs);
  red.append(rrow);
  c.append(red);
  p.append(c);

  const ai = el('div', 'card');
  ai.innerHTML = '<h2>Real AI (client Instant Race)</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">'
    + 'Dedicated-server <code>-virtual_ai_cars</code> is a replay along a '
    + 'reference lap (<code>sendCarPhysicsUpdate</code> is not implemented). '
    + 'The real driver, <code>AiDriverEvo</code>, lives in the <b>client</b>. '
    + 'This starts <em>one</em> game process with '
    + '<code>-ai_enable_evo_next</code> and <code>-opponent_count</code>, '
    + 'and points Instant Race at that grid. Not a bot farm.</div>';
  const arow = el('div', 'row wrap');
  const nIn = el('input');
  nIn.type = 'number'; nIn.min = 1; nIn.max = 40; nIn.value = 16;
  nIn.style.width = '4.5em';
  nIn.title = 'Opponent count';
  const go = el('button', 'primary', 'Launch real AI race');
  go.onclick = async () => {
    const r = await api('game/launch_ai', {
      opponents: parseInt(nIn.value, 10) || 16,
      min_strength: 70, max_strength: 95,
    });
    toast(r.ok ? (r.hint || 'Launched') : (r.error || 'Launch failed'), !r.ok);
  };
  arow.append(nIn, go);
  ai.append(arow);
  p.append(ai);

  const l = el('div', 'card');
  l.innerHTML = '<h2>Backend log</h2>';
  const pre = el('pre', 'log', '(nothing yet)');
  l.append(pre);
  p.append(l);
  const r = await api('backend/log?mode=proxy');
  pre.textContent = (r.lines || []).join('\n') || '(nothing yet)';
  pre.scrollTop = pre.scrollHeight;
}

/* ------------------------------------------------------------- settings -- */
async function settingsPage() {
  const cfg = await api('config');
  const p = $('#page');
  p.innerHTML = '';

  // Install to a permanent folder + Start Menu shortcut, so the exe never has
  // to be hunted down again.
  const ins = await api('install');
  const ic = el('div', 'card');
  ic.innerHTML = '<h2>Install</h2>';
  if (!ins.frozen) {
    ic.append(el('div', 'tiny dim', esc(ins.note || '')));
  } else if (ins.running_installed) {
    ic.append(el('div', 'tiny dim',
      'Running the installed copy — <code>' + esc(ins.installed_exe)
      + '</code>'));
  } else {
    ic.append(el('div', 'tiny dim',
      'Running from <code>' + esc(ins.running_exe || '?') + '</code>. Install '
      + 'puts a copy in <code>' + esc(ins.install_dir) + '</code> with a Start '
      + 'Menu shortcut. Your profiles and settings are not touched.'));
  }
  const irow = el('div', 'row wrap');
  if (ins.frozen) {
    const go = el('button', 'primary sm',
      ins.installed ? 'Reinstall / repair shortcuts' : 'Install + make shortcut');
    go.onclick = async () => {
      const r = await api('install/run', { desktop: true });
      toast(r.ok ? ('Installed to ' + r.exe) : (r.error || 'Install failed'),
            !r.ok);
      settingsPage();
    };
    irow.append(go);
    if (ins.installed) {
      const rm = el('button', 'sm danger', 'Remove shortcuts');
      rm.onclick = async () => {
        if (!confirm('Remove the Start Menu and Desktop shortcuts?\n\n'
                     + 'Your profiles and settings are kept.')) return;
        const r = await api('install/remove', {});
        toast(r.ok ? 'Shortcuts removed' : (r.error || 'Failed'), !r.ok);
        settingsPage();
      };
      irow.append(rm);
    }
  }
  irow.append(el('span', 'pill ' + (ins.start_menu ? 'on' : 'off'),
    '<i class="dot"></i>Start Menu'));
  irow.append(el('span', 'pill ' + (ins.desktop ? 'on' : 'off'),
    '<i class="dot"></i>Desktop'));
  ic.append(irow);
  p.append(ic);

  const c = el('div', 'card');
  c.innerHTML = '<h2>Paths &amp; ports</h2>';
  const g = el('div', 'grid g2');
  const draft = {};
  Object.entries(cfg).forEach(([k, v]) => {
    const l = el('label', 'f', `<span>${esc(k)}</span>`);
    const i = el('input');
    i.value = v;
    i.oninput = () => { draft[k] = typeof v === 'number' ? Number(i.value) : i.value; };
    l.append(i);
    g.append(l);
  });
  c.append(g);
  const save = el('button', 'primary', 'Save settings');
  save.onclick = async () => { await api('config', draft); toast('Saved — restart ACECM to apply ports'); };

  // ---- detected locations -----------------------------------------------
  const dc = el('div', 'card');
  dc.innerHTML = '<h2>Detected locations</h2>'
    + '<div class="tiny dim">Found from Steam&rsquo;s own library list and '
    + 'Windows known folders, so extra drives and a relocated '
    + '&ldquo;Saved Games&rdquo; are handled. Leave the matching setting above '
    + 'blank to use detection; anything you type there wins.</div>';
  const drow = el('div', 'row wrap');
  drow.style.margin = '10px 0';
  const redo = el('button', 'sm primary', 'Re-detect');
  const dnote = el('span', 'tiny dim');
  drow.append(redo, dnote);
  const dlist = el('div');
  dc.append(drow, dlist);
  p.append(dc);

  async function showPaths(refresh) {
    dnote.textContent = refresh ? 'scanning…' : '';
    const r = await api('detect' + (refresh ? '?refresh=1' : ''));
    dnote.textContent = `${r.took_ms} ms · steam: ${r.steam || 'not found'}`
      + ` · ${(r.libraries || []).length} librar`
      + ((r.libraries || []).length === 1 ? 'y' : 'ies');
    dlist.innerHTML = '';
    Object.entries(r.paths || {}).forEach(([k, v]) => {
      dlist.append(el('div', 'chk',
        `<span class="name">${esc(k)}`
        + `<div class="tiny dim">${esc(v.path || 'not found')}</div></span>`
        + `<span class="pill ${v.exists ? 'on' : (v.pending ? '' : 'off')}">`
        + `<i class="dot"></i>${v.exists ? v.source
             : (v.pending ? 'created on first use' : 'missing')}</span>`));
    });
  }
  redo.onclick = () => showPaths(true);
  showPaths(false);

  // ---- updates ----------------------------------------------------------
  const uc = el('div', 'card');
  uc.innerHTML = '<h2>Updates</h2>'
    + '<div class="tiny dim">Checks this project&rsquo;s latest GitHub Release '
    + 'for an <code>ACECM.exe</code>, verifies it against the SHA-256 GitHub '
    + 'publishes, and swaps the exe on restart. The old build is kept as '
    + '<code>.old</code>.</div>';
  const urow = el('div', 'row wrap');
  urow.style.margin = '10px 0';
  const vpill = el('span', 'pill off', '<i class="dot"></i>checking version…');
  const chk = el('button', 'sm', 'Check for updates');
  const get = el('button', 'sm primary', 'Download & install');
  get.style.display = 'none';
  const unote = el('div', 'tiny dim');
  urow.append(chk, get, vpill);
  uc.append(urow, unote);
  p.append(uc);

  api('version').then(v => {
    vpill.className = 'pill on';
    vpill.innerHTML = `<i class="dot"></i>v${esc(v.version)}`
      + (v.frozen ? '' : ' (running from source)');
  });
  chk.onclick = async () => {
    unote.textContent = 'checking…';
    const r = await api('update/check');
    if (!r.ok) { unote.innerHTML = `<b>${esc(r.error || 'check failed')}</b>`
      + (r.hint ? '<br>' + esc(r.hint) : ''); return; }
    if (!r.checked) { unote.textContent = r.hint || 'update checks are off'; return; }
    if (r.available) {
      unote.innerHTML = `<b>v${esc(r.latest)}</b> is available `
        + `(you have v${esc(r.current)})`
        + (r.notes ? '<pre class="log" style="max-height:160px">'
                     + esc(r.notes) + '</pre>' : '');
      get.style.display = '';
    } else {
      unote.textContent = `up to date (latest is v${r.latest || '?'})`;
      get.style.display = 'none';
    }
    if (r.error) unote.innerHTML += '<br>' + esc(r.error);
  };
  get.onclick = async () => {
    unote.textContent = 'downloading…';
    const r = await api('update/apply', {});
    unote.textContent = r.ok ? (r.note || 'downloaded') : (r.error || 'failed');
    if (r.ok) {
      toast('Update downloaded — restart to finish', false);
      rst.style.display = '';
    }
  };
  // ⚠ The swap happens when this process EXITS - the downloaded exe cannot
  // replace a running one. Without a restart button the update just sits
  // there looking finished, and people report that it did not apply.
  const rst = el('button', 'primary', 'Restart ACECM to finish');
  rst.style.display = 'none';
  rst.onclick = async () => {
    const r = await api('app/restart', {});
    if (!r.ok) { toast(r.error || 'restart failed', true); return; }
    unote.textContent = 'restarting…';
    // the window this page lives in is about to go away
    setTimeout(() => { document.body.style.opacity = '0.4'; }, 400);
  };
  urow.append(rst);
  c.append(save);
  p.append(c);
}


/* -------------------------------------------------------------- content -- */
let scanned = null;
/* Tracks this machine can hand to a joining player.

   ⚠ There was no way to DO this from the UI. Publishing only ever happened as
   a side effect of deploying a track, yet the dashboard tells you to "publish
   from Content" - so the one instruction the app gives had no matching button.
   Sharing is also the thing that makes a modded server joinable at all, which
   makes it too important to be a side effect. */
async function shareCard(p) {
  const loc = await api('browser/local');
  const reg = await api('registry');
  const shared = {};
  (reg.servers || []).forEach(e => (e.required_tracks || []).forEach(
    t => { shared[t] = e; }));

  const imported = new Set(loc.tracks || []);
  const rows = Object.entries(loc.track_map || {})
    .filter(([, folder]) => imported.has(folder))
    .sort((a, b) => a[0].localeCompare(b[0]));

  const c = el('div', 'card');
  c.innerHTML = '<h2>Shared for download</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">A player who does not '
    + 'have your track cannot join, and the game will not send it to them. '
    + 'Share it here and their ACECM can download it from you. Only tracks you '
    + 'imported are listed — stock tracks everyone already has.</div>';
  if (!rows.length) {
    c.append(el('div', 'empty', 'No imported tracks to share'));
  } else {
    rows.forEach(([name, folder]) => {
      const row = el('div', 'chk');
      const on = !!shared[folder];
      row.innerHTML = `<span class="name"><b>${esc(name)}</b>`
        + `<div class="tiny dim">${esc(folder)}</div></span>`
        + `<span class="pill ${on ? 'on' : 'off'}"><i class="dot"></i>`
        + `${on ? 'shared' : 'not shared'}</span>`;
      const b = el('button', on ? 'sm danger' : 'sm primary',
                   on ? 'Stop sharing' : 'Share');
      b.onclick = async () => {
        if (on) {
          await api('registry/delete', { id: shared[folder].id });
          toast('No longer shared');
        } else {
          const r = await api('registry/save', {
            name: `${name} (hosted here)`,
            description: `Content for ${name}`,
            required_tracks: [folder], public: true,
          });
          toast(r && r.error ? r.error : `${name} is now downloadable`,
                !!(r && r.error));
        }
        contentPage();
      };
      row.append(b);
      c.append(row);
    });
  }

  // ⚠ Cars too. A modded car is as much a reason a join is refused as a
  // missing track - the backend even answers with `missing_cars` - and the
  // delivery side already handled them; there was simply no way to say "share
  // this one", so nobody could ever download a car from you.
  const mods = [...new Set([].concat(
    ((await api('mods?side=client')).mods || []).map(m => m.name),
    ((await api('mods?side=server')).mods || []).map(m => m.name)))].sort();
  const sharedMods = {};
  (reg.servers || []).forEach(e => (e.required_mods || []).forEach(
    m => { sharedMods[m] = e; }));

  c.append(el('div', 'tiny dim', '<b style="color:var(--fg)">Car mods</b>'));
  if (!mods.length) {
    c.append(el('div', 'empty', 'No car mods installed'));
  } else {
    mods.forEach(name => {
      const row = el('div', 'chk');
      const on = !!sharedMods[name];
      row.innerHTML = `<span class="name"><b>${esc(name)}</b></span>`
        + `<span class="pill ${on ? 'on' : 'off'}"><i class="dot"></i>`
        + `${on ? 'shared' : 'not shared'}</span>`;
      const b = el('button', on ? 'sm danger' : 'sm primary',
                   on ? 'Stop sharing' : 'Share');
      b.onclick = async () => {
        if (on) {
          const e = sharedMods[name];
          const keep = (e.required_mods || []).filter(m => m !== name);
          // an entry that shared only this car has nothing left to offer
          if (!keep.length && !(e.required_tracks || []).length) {
            await api('registry/delete', { id: e.id });
          } else {
            await api('registry/save', { ...e, required_mods: keep });
          }
          toast('No longer shared');
        } else {
          const r = await api('registry/save', {
            name: `${name} (car mod)`,
            description: `Car mod ${name} shared by this host`,
            required_mods: [name], public: true,
          });
          toast(r && r.error ? r.error : `${name} is now downloadable`,
                !!(r && r.error));
        }
        contentPage();
      };
      row.append(b);
      c.append(row);
    });
  }
  p.append(c);
}

async function contentPage() {
  const p = $('#page');
  p.innerHTML = '';
  const mods = await api('mods');

  // ⚠ Order matters. Hosting a custom track is why most people open this
  // page, and the dashboard sends them here to publish one - so that comes
  // first. Car mods and the read-only track inventory follow.
  await shareCard(p);
  // --- custom track deploy -------------------------------------------------
  const td = await api('trackdeploy');

  /* Tracks you already imported, deployable straight to the server.

     ⚠ This is the normal case and it had no button anywhere: the list below
     only shows slot-borrow packages, so a track imported in EvoForge could
     not be hosted from the UI at all - while the server refused to start,
     telling you to "deploy it from Content". */
  const ic = el('div', 'card');
  const imported = (td.imported || []).filter(t => t.ok);
  ic.innerHTML = `<h2>Your imported tracks &middot; ${imported.length}</h2>`
    + '<div class="tiny dim" style="margin-bottom:10px">Deploying puts the '
    + 'track\'s logic into the server archive under its own name, and shares '
    + 'it so players missing it can download from you. Stock tracks are not '
    + 'touched.</div>';
  if (!imported.length) {
    ic.append(el('div', 'empty',
      'No imported tracks found in Saved Games\\ACE\\mods'));
  } else {
    imported.forEach(t => {
      const row = el('div', 'chk');
      row.innerHTML = `<span class="name"><b>${esc(t.display_name)}</b>`
        + `<div class="tiny dim">${esc(t.folder)} · layout `
        + `${esc(t.layout || '?')} · ${t.files} file(s)</div></span>`;
      const go = el('button', 'sm primary', 'Deploy to server');
      go.disabled = td.server_running;
      go.title = td.server_running
        ? 'Stop the server first — it holds content.kspkg open'
        : 'Install at its own paths and publish it for download';
      go.onclick = async () => {
        if (!confirm(`Deploy "${t.display_name}" to the server?\n\n`
            + 'It is installed under its own name, so no stock track is '
            + 'overwritten, and it is published so players can download it.'))
          return;
        toast('Deploying — this rewrites a 300 MB archive, please wait…');
        const r = await api('trackdeploy/deploy',
                            { path: t.path, native: 1 });
        toast(r.ok ? `Deployed as "${r.display_name}" — ${r.modes} game modes`
                   : (r.error || 'Deploy failed'), !r.ok);
        contentPage();
      };
      row.append(go);
      ic.append(row);
    });
  }
  p.append(ic);

  const dc = el('div', 'card');
  dc.innerHTML = '<h2>Deploy a custom track</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">'
    // ⚠ This used to say a new path "cannot be found at all". That turned out
    // to be a malformed record header plus a table that must stay sorted, not
    // a limit of the engine - native installs are proven, so the old wording
    // now argues against what the button above it does.
    + 'The track\'s logic files go into the server\'s '
    + '<code>content.kspkg</code>; the art stays with each player. '
    + '<b>Native</b> installs it under its own name, so stock tracks are '
    + 'untouched and anyone who imported the track can join with no extra '
    + 'setup. <b>Borrow a slot</b> is the old way: it overwrites a stock '
    + 'track and every joiner must patch their own game.</div>'
    + `<div class="row wrap" style="margin-bottom:10px">`
    + `<span class="pill ${td.server_running ? 'bad' : 'on'}"><i class="dot"></i>`
    + `server ${td.server_running ? 'RUNNING — stop it first' : 'stopped'}</span>`
    + `<span class="pill ${td.backup ? 'on' : 'off'}"><i class="dot"></i>`
    + `${td.backup ? 'pre-deploy backup exists' : 'no backup yet'}</span>`
    + `<span class="pill off"><i class="dot"></i>${td.size_mb} MB archive</span>`
    + `</div>`
    + `<div class="tiny dim" style="margin-bottom:10px">Target: `
    + `<code>${esc(td.kspkg)}</code><br>${esc(td.note)}</div>`;

  (td.packages || []).forEach(pk => {
    const row = el('div', 'chk');
    // `a || b ? x : y` binds as `(a||b) ? x : y`, so a package WITH a note was
    // showing the missing-files text instead. Prefer the note.
    const why = pk.note ? pk.note
      : ((pk.missing || []).length ? `missing ${pk.missing.join(', ')}` : '');
    row.innerHTML = `<span class="name"><b>${esc(pk.display_name || '(unnamed)')}</b>`
      + `<div class="tiny dim">${esc(pk.folder || '')}`
      + (pk.files ? ` &middot; ${pk.files} files` : '')
      + (pk.host_slots && pk.host_slots.length
          ? ` &middot; borrows ${esc(pk.host_slots[0].split(/[\\/]/).pop())}` : '')
      + `<br>${esc(pk.path)}</div></span>`
      + `<span class="pill ${pk.ok ? 'on' : 'warn'}"><i class="dot"></i>`
      + `${pk.ok ? 'ready' : esc(why || 'incomplete')}</span>`;
    // Native install: the track keeps its own paths, so no stock track is
    // overwritten and joiners need nothing beyond their EvoForge import.
    const nat = el('button', 'sm primary', 'Deploy (native)');
    nat.disabled = !pk.ok || td.server_running;
    nat.title = 'Install at the track\'s own paths. Stock tracks stay intact '
      + 'and clients that already imported this track need no extra install.';
    nat.onclick = async () => {
      if (!confirm('Install "' + (pk.display_name || pk.path)
          + '" at its own paths?\n\nNo stock track is overwritten. Clients '
          + 'need this track imported in EvoForge, under the same folder name '
          + '(' + (pk.folder || '?') + ').\n\nThe archive is backed up first.'))
        return;
      toast('Installing — this rewrites a 300 MB archive, please wait…');
      const r = await api('trackdeploy/deploy', { path: pk.path, native: 1 });
      toast(r.ok ? ('Installed at content\\tracks\\' + r.folder
                    + ' — ' + r.modes + ' game modes')
                 : (r.error || 'Install failed'), !r.ok);
      contentPage();
    };
    row.append(nat);

    const go = el('button', 'sm', 'Deploy (borrow slot)');
    go.disabled = !pk.ok || td.server_running;
    go.title = 'Legacy: overwrites Road Atlanta\'s slots. Every client also '
      + 'needs install_track.py. Kept as a fallback.';
    go.onclick = async () => {
      if (!confirm('Deploy "' + (pk.display_name || pk.path)
          + '" by BORROWING a stock track\'s slots?\n\nRoad Atlanta will show '
          + 'this track until you restore, and every joining client must run '
          + 'install_track.py.\n\nPrefer "Deploy (native)".')) return;
      toast('Deploying — this rewrites a 300 MB archive, please wait…');
      const r = await api('trackdeploy/deploy', { path: pk.path });
      toast(r.ok ? 'Track deployed' : (r.error || 'Deploy failed'), !r.ok);
      contentPage();
    };
    row.append(go);
    dc.append(row);
    if (pk.note) dc.append(el('div', 'tiny dim',
      '⚠ ' + esc(pk.note)));
  });
  if (!(td.packages || []).length)
    dc.append(el('div', 'empty',
      'No track packages found. Build one with build_track_package.py.'));

  const rrow = el('div', 'row');
  rrow.style.marginTop = '10px';
  const rest = el('button', 'sm danger', 'Restore archive');
  rest.disabled = !td.backup;
  rest.onclick = async () => {
    if (!confirm('Restore content.kspkg from the pre-deploy backup?')) return;
    const r = await api('trackdeploy/restore', {});
    toast(r.ok ? 'Archive restored' : (r.error || 'Restore failed'), !r.ok);
    contentPage();
  };
  rrow.append(rest);
  dc.append(rrow);
  p.append(dc);

  // --- installed car mods --------------------------------------------------
  const c = el('div', 'card');
  c.innerHTML = `<h2>Installed car mods &middot; ${mods.total ?? 0} `
    + `(${mods.usable ?? 0} usable)</h2>`
    + `<div class="tiny dim" style="margin-bottom:10px">The dedicated server `
    + `reads mods from your user profile, not its install folder:<br>`
    + `<code>${esc(mods.dir)}</code></div>`;
  if (!(mods.mods || []).length) {
    c.append(el('div', 'empty', 'No car mods installed'));
  } else {
    mods.mods.forEach(m => {
      const row = el('div', 'chk');
      const cars = (m.cars || []).map(x => esc(x.label)).join(', ');
      row.innerHTML =
        `<span class="name"><b>${esc(m.name)}</b>`
        + (cars ? ` <span class="dim">— ${cars}</span>` : '')
        + `<div class="tiny dim">${m.size_mb} MB`
        + (m.why ? ` &middot; <span style="color:var(--gold)">${esc(m.why)}</span>` : '')
        + `</div></span>`
        + `<span class="pill ${m.usable ? 'on' : 'warn'}"><i class="dot"></i>`
        + `${m.usable ? 'ready' : 'incomplete'}</span>`;
      const rm = el('button', 'sm danger', 'Remove');
      rm.onclick = async () => {
        if (!confirm('Remove mod "' + m.name + '"?')) return;
        await api('mods/remove', { name: m.name });
        toast('Removed ' + m.name); contentPage();
      };
      row.append(rm);
      c.append(row);
    });
  }
  p.append(c);

  // --- cross-side audit ----------------------------------------------------
  // Cars need the same .kspkg + .json on BOTH sides. They drift apart easily,
  // and the symptom (car missing from the list) does not point at the cause.
  const au = await api('mods/audit');
  const ac = el('div', 'card');
  ac.innerHTML = `<h2>Server vs client &middot; `
    + `${au.problems ? au.problems + ' problem(s)' : 'all matched'}</h2>`
    + `<div class="tiny dim" style="margin-bottom:10px">`
    + `server <code>${esc(au.server_dir || '')}</code><br>`
    + `client <code>${esc(au.client_dir || '')}</code><br>`
    + `A join needs the same <code>.kspkg</code> + <code>.json</code> on both `
    + `sides. Sync copies the pair; it will not overwrite a different-sized `
    + `file unless you force it. Stop the dedicated server first if a `
    + `.kspkg is locked.</div>`;
  const syncRow = el('div', 'row wrap');
  syncRow.style.marginBottom = '10px';
  const sSrv = el('button', 'primary', 'Sync to server');
  sSrv.title = 'Copy client mods onto the dedicated server';
  sSrv.onclick = async () => {
    const r = await api('mods/sync', { direction: 'to_server' });
    toast(r.ok
      ? `Copied ${(r.copied || []).length} file(s), ${r.problems_after} problem(s) left`
      : (r.error || (r.errors || []).join('; ') || 'Sync failed'), !r.ok);
    contentPage();
  };
  const sCli = el('button', null, 'Sync to client');
  sCli.onclick = async () => {
    const r = await api('mods/sync', { direction: 'to_client' });
    toast(r.ok
      ? `Copied ${(r.copied || []).length} file(s), ${r.problems_after} problem(s) left`
      : (r.error || (r.errors || []).join('; ') || 'Sync failed'), !r.ok);
    contentPage();
  };
  const sBoth = el('button', null, 'Fill gaps both ways');
  sBoth.onclick = async () => {
    const r = await api('mods/sync', { direction: 'both' });
    toast(r.ok
      ? `Copied ${(r.copied || []).length} file(s), ${r.problems_after} problem(s) left`
      : (r.error || (r.errors || []).join('; ') || 'Sync failed'), !r.ok);
    contentPage();
  };
  syncRow.append(sSrv, sCli, sBoth);
  ac.append(syncRow);
  (au.mods || []).forEach(m => {
    const row = el('div', 'chk');
    row.innerHTML = `<span class="name"><b>${esc(m.name)}</b>`
      + `<div class="tiny dim">${m.size_mb} MB`
      + (m.issues || []).map(i => `<br><span style="color:var(--gold)">${esc(i)}</span>`).join('')
      + `</div></span>`
      + `<span class="pill ${m.server_ok ? 'on' : 'off'}"><i class="dot"></i>server</span>`
      + `<span class="pill ${m.client_ok ? 'on' : 'off'}"><i class="dot"></i>client</span>`;
    if (m.fix) {
      const fx = el('button', 'sm primary', 'Fix');
      fx.title = m.fix.what;
      fx.onclick = async () => {
        const r = await api('mods/fix', { name: m.name });
        toast(r.ok ? r.what : (r.error || 'Fix failed'), !r.ok);
        contentPage();
      };
      row.append(fx);
    }
    ac.append(row);
  });
  if (!(au.mods || []).length) ac.append(el('div', 'empty', 'No mods on either side'));
  p.append(ac);

  // --- install from folder / zip -------------------------------------------
  const inst = el('div', 'card');
  inst.innerHTML = '<h2>Install a car mod</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">Point at a folder or a '
    + '.zip containing <code>&lt;mod&gt;.kspkg</code> and '
    + '<code>&lt;mod&gt;.json</code>. Both are required — a mod without its '
    + '.json installs fine and then never appears in the car list.</div>';
  const rowi = el('div', 'row');
  const inp = el('input');
  inp.placeholder = 'C:\path\to\mod-folder  or  mod.zip';
  const scan = el('button', null, 'Scan');
  rowi.append(inp, scan);
  inst.append(rowi);
  const res = el('div');
  res.style.marginTop = '10px';
  inst.append(res);

  scan.onclick = async () => {
    res.innerHTML = '';
    scanned = await api('mods/scan?path=' + encodeURIComponent(inp.value));
    if (!scanned.ok) return;
    if (!scanned.mods.length) { res.append(el('div', 'empty', 'No mods found there')); return; }
    scanned.mods.forEach(m => {
      res.append(el('div', 'chk',
        `<span class="name">${esc(m.name)}</span>`
        + `<span class="pill ${m.complete ? 'on' : 'warn'}"><i class="dot"></i>`
        + `${m.complete ? 'kspkg + json' : (m.kspkg ? 'missing .json' : 'missing .kspkg')}</span>`));
    });
    const go = el('button', 'primary', `Install ${scanned.complete} mod(s)`);
    go.onclick = async () => {
      const r = await api('mods/install', { path: inp.value });
      if (r.ok) toast(r.warning || ('Installed ' + (r.installed || []).length + ' file(s)'),
                      !!r.warning);
      contentPage();
    };
    res.append(go);
  };
  p.append(inst);

  // --- tracks --------------------------------------------------------------
  const t = await api('tracks/installed');
  const tc = el('div', 'card');
  tc.innerHTML = '<h2>Installed tracks</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">AI needs both spline '
    + 'files per layout. The server package ships neither — they must be loose '
    + 'on disk, because content added to a .kspkg is invisible to the engine '
    + '(it resolves paths by hash against the archive index).</div>';
  (t.tracks || []).forEach(tr => {
    tr.layouts.forEach(l => {
      tc.append(el('div', 'chk',
        `<span class="name">${esc(tr.track)} <span class="dim">/ ${esc(l.layout)}</span></span>`
        + `<span class="pill ${l.ai_ready ? 'on' : 'warn'}"><i class="dot"></i>`
        + `${l.ai_ready ? 'AI ready' : (l.ideal_line ? 'no pitlane spline' : 'no ideal line')}</span>`));
    });
  });
  if (!(t.tracks || []).length) tc.append(el('div', 'empty', 'No tracks found'));
  p.append(tc);

}



/* -------------------------------------------------------------- patches -- */
async function patchesPage() {
  const p = $('#page');
  p.innerHTML = '';
  const d = await api('patches');

  const intro = el('div', 'card');
  intro.innerHTML = '<h2>Binary patches</h2>'
    + '<div class="tiny dim">Patches are data, not scripts: each declares the '
    + 'build it was made for and the exact bytes it expects. Applying verifies '
    + '<b>every</b> site first and writes nothing unless all match, so a '
    + 'half-applied patch cannot happen. A patch made for another build refuses '
    + 'to apply rather than corrupting the file.</div>';
  p.append(intro);

  const STATE = {
    applied:      ['on',   'applied'],
    clean:        ['off',  'not applied'],
    mismatch:     ['bad',  'bytes unrecognised — will not write'],
    'wrong-build':['warn', 'built for a different game version'],
    missing:      ['bad',  'target file not found'],
  };

  (d.patches || []).forEach(pt => {
    const st = pt.status || {};
    const [cls, label] = STATE[st.state] || ['off', st.state || '?'];
    const c = el('div', 'card');
    c.innerHTML = `<div class="row"><div class="grow">
        <b>${esc(pt.title || pt.id)}</b>
        <div class="tiny dim">${esc(pt.description || '')}</div>
        <div class="tiny dim">${esc(pt.target || '')}<br>
        ${st.sites || 0} site(s) &middot; built for ${esc((pt.build_md5||'any').slice(0,12))}
        &middot; now ${esc((st.md5||'?').slice(0,12))}</div></div>
        <span class="pill ${cls}"><i class="dot"></i>${esc(label)}</span></div>`;
    const row = el('div', 'row wrap');
    row.style.marginTop = '10px';
    const ap = el('button', 'primary sm', 'Apply');
    ap.disabled = st.state === 'applied' || st.state === 'mismatch'
                  || st.state === 'missing';
    ap.onclick = async () => {
      const r = await api('patches/apply', { id: pt.id });
      toast(r.ok ? 'Applied — backup kept' : (r.error || 'Failed'), !r.ok);
      patchesPage();
    };
    const rs = el('button', 'sm danger', 'Restore');
    rs.disabled = st.state === 'clean' || st.state === 'missing';
    rs.onclick = async () => {
      if (!confirm('Restore the original bytes?')) return;
      const r = await api('patches/restore', { id: pt.id });
      toast(r.ok ? 'Restored' : (r.error || 'Failed'), !r.ok);
      patchesPage();
    };
    row.append(ap, rs);
    if (st.state === 'wrong-build') {
      const f = el('button', 'sm', 'Apply anyway');
      f.title = 'Offsets almost certainly do not match this build';
      f.onclick = async () => {
        if (!confirm('This patch was built for a different game version.\n'
            + 'Offsets will very likely be wrong. Continue?')) return;
        const r = await api('patches/apply', { id: pt.id, force: true });
        toast(r.ok ? 'Applied (forced)' : (r.error || 'Failed'), !r.ok);
        patchesPage();
      };
      row.append(f);
    }
    c.append(row);
    p.append(c);
  });
  if (!(d.patches || []).length)
    p.append(el('div', 'card').appendChild(
      el('div', 'empty', 'No patches registered yet')).parentElement);

  // --- what the binary offers a patch author ------------------------------
  const insp = await api('patches/inspect');
  if (insp.ok) {
    const ic = el('div', 'card');
    ic.innerHTML = `<h2>Patch space &middot; ${esc(insp.md5.slice(0,12))}</h2>`
      + '<div class="tiny dim" style="margin-bottom:9px">'
      + 'Code caves are int3 padding in executable sections. Slack is the free '
      + 'page tail after a writable section — <b>not</b> the BSS gap, which '
      + 'holds the program’s own globals and must never be written.</div>';
    (insp.code_caves || []).forEach(c => ic.append(el('div', 'chk',
      `<span class="name">cave ${esc(c.va)} <span class="dim">${esc(c.section)}</span></span>`
      + `<span class="pill off">${Math.round(c.size/1024)} KB</span>`)));
    (insp.bss_slack || []).forEach(s => ic.append(el('div', 'chk',
      `<span class="name">slack ${esc(s.va)} <span class="dim">${esc(s.section)}</span></span>`
      + `<span class="pill off">${s.size} B</span>`)));
    p.append(ic);
  }
}


/* --------------------------------------------------------- game settings -- */
let gsFile = 'input_settings.inputsettings';
async function gameSettingsPage() {
  const p = $('#page');
  p.innerHTML = '';
  const d = await api('gamesettings');

  if (d.game_running)
    p.append(el('div', 'err', 'The game is <b>running</b>. It rewrites these '
      + 'files when it exits, so changes saved now would be lost. Close it first.'));

  // file picker
  const pick = el('div', 'card');
  pick.innerHTML = '<h2>Settings files</h2>'
    + `<div class="tiny dim" style="margin-bottom:9px">${esc(d.dir || '')}<br>`
    + 'Stored as protobuf; decoded with the schemas extracted from the client, '
    + 'so these are the game&rsquo;s real field names.</div>';
  const sel = el('select');
  (d.files || []).filter(f => f.decodable).forEach(f => {
    const o = el('option', null, esc(f.label ? f.label + ' — ' + f.file : f.file));
    o.value = f.file;
    if (f.file === gsFile) o.selected = true;
    sel.append(o);
  });
  sel.onchange = () => { gsFile = sel.value; gameSettingsPage(); };
  pick.append(sel);
  p.append(pick);

  // ---- backup / share ----------------------------------------------------
  const share = el('div', 'card');
  share.innerHTML = '<h2>Backup &amp; share</h2>'
    + '<div class="tiny dim">Exports your settings as readable JSON rather '
    + 'than raw files: you can diff it, edit it, and it re-encodes against '
    + 'whatever schema the receiving machine has. Every import backs up what '
    + 'it replaces.</div>';
  const srow = el('div', 'row wrap');
  srow.style.margin = '10px 0';
  const bAll = el('button', 'primary sm', 'Export all settings');
  const bOne = el('button', 'sm', 'Export this file only');
  const bImp = el('button', 'sm', 'Import bundle…');
  const devs = el('label', 'tiny dim');
  const devChk = el('input');
  devChk.type = 'checkbox';
  devs.append(devChk, document.createTextNode(' include device bindings'));
  devs.title = 'Bindings reference specific hardware, so they are skipped '
             + 'unless you ask for them';
  const bBack = el('button', 'sm', 'Backups of this file…');
  srow.append(bAll, bOne, bImp, bBack, devs);
  const snote = el('div', 'tiny dim');
  share.append(srow, snote);
  p.append(share);

  function download(name, obj) {
    const blob = new Blob([JSON.stringify(obj, null, 2)],
                          { type: 'application/json' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = name;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 4000);
  }
  const stamp = () => new Date().toISOString().slice(0, 10);

  bAll.onclick = async () => {
    snote.textContent = 'exporting…';
    const b = await api('gamesettings/export');
    download(`ace-settings-${stamp()}.json`, b);
    snote.textContent = `exported ${Object.keys(b.files || {}).length} file(s)`
      + (Object.keys(b.skipped || {}).length
         ? `, skipped ${Object.keys(b.skipped).length}` : '');
  };
  bOne.onclick = async () => {
    const b = await api('gamesettings/export?file=' + encodeURIComponent(gsFile));
    download(`ace-${gsFile.replace(/[^a-z0-9]+/gi, '-')}-${stamp()}.json`, b);
    snote.textContent = 'exported ' + gsFile;
  };
  bImp.onclick = () => {
    const inp = document.createElement('input');
    inp.type = 'file';
    inp.accept = 'application/json,.json';
    inp.onchange = async () => {
      const f = inp.files && inp.files[0];
      if (!f) return;
      let bundle;
      try { bundle = JSON.parse(await f.text()); }
      catch (e) { toast('That file is not valid JSON', true); return; }
      const names = Object.keys(bundle.files || {});
      if (!confirm(`Apply ${names.length} settings file(s) from ${f.name}?

`
                   + 'Whatever they replace is backed up first.')) return;
      const r = await api('gamesettings/import',
        { bundle, include_devices: devChk.checked });
      snote.innerHTML = r.ok
        ? `applied ${(r.applied || []).length} file(s)`
          + ((r.skipped && Object.keys(r.skipped).length)
             ? `, skipped ${Object.keys(r.skipped).length} (device bindings)` : '')
        : `<b>${esc(r.error || 'import failed')}</b>`;
      if (r.failed && Object.keys(r.failed).length)
        snote.innerHTML += '<pre class="log">' + esc(JSON.stringify(r.failed, null, 2)) + '</pre>';
      toast(r.ok ? 'Settings imported' : 'Import failed', !r.ok);
      if (r.ok) setTimeout(gameSettingsPage, 900);
    };
    inp.click();
  };
  bBack.onclick = async () => {
    const r = await api('gamesettings/backups?file=' + encodeURIComponent(gsFile));
    const list = r.backups || [];
    if (!list.length) { snote.textContent = 'no backups of this file yet'; return; }
    snote.innerHTML = '';
    list.forEach(b => {
      const row = el('div', 'chk',
        `<span class="name">${esc(b.name)}<div class="tiny dim">`
        + `${new Date(b.mtime * 1000).toLocaleString()} · ${b.size} bytes</div></span>`);
      const rb = el('button', 'sm', 'Restore');
      rb.onclick = async () => {
        if (!confirm('Restore ' + b.name + '?')) return;
        const res = await api('gamesettings/restore_backup',
                              { file: gsFile, name: b.name });
        toast(res.ok ? 'Restored' : (res.error || 'Failed'), !res.ok);
        if (res.ok) setTimeout(gameSettingsPage, 800);
      };
      row.append(rb);
      snote.append(row);
    });
  };

  const r = await api('gamesettings/read?file=' + encodeURIComponent(gsFile));
  if (!r.ok) { p.append(el('div', 'card').appendChild(
      el('div', 'empty', esc(r.error || 'cannot read'))).parentElement); return; }

  const card = el('div', 'card');
  card.innerHTML = `<h2>${esc(r.message)}</h2>`;
  const draft = JSON.parse(JSON.stringify(r.values || {}));

  // Render nested objects as sections; scalars as typed inputs. Lists and deep
  // structures (controller bindings) are shown read-only as JSON rather than
  // pretending a form can safely edit them.
  const render = (obj, parent, path) => {
    Object.entries(obj).forEach(([k, v]) => {
      const here = path.concat([k]);
      if (v !== null && typeof v === 'object' && !Array.isArray(v)) {
        parent.append(el('div', 'tiny dim',
          `<b style="color:var(--fg)">${esc(k)}</b>`));
        const g = el('div', 'grid g2');
        g.style.margin = '6px 0 12px';
        parent.append(g);
        render(v, g, here);
        return;
      }
      if (Array.isArray(v)) {
        const l = el('label', 'f', `<span>${esc(k)} (${v.length} item(s), read-only)</span>`);
        const ta = el('input');
        ta.value = JSON.stringify(v).slice(0, 120);
        ta.disabled = true;
        l.append(ta);
        parent.append(l);
        return;
      }
      if (typeof v === 'boolean') {
        const l = el('label', 'ctl');
        const cb = el('input'); cb.type = 'checkbox'; cb.checked = v;
        cb.onchange = () => setAt(draft, here, cb.checked);
        l.append(cb, el('span', null, esc(k)));
        parent.append(l);
        return;
      }
      const l = el('label', 'f', `<span>${esc(k)}</span>`);
      const inp = el('input');
      const numeric = typeof v === 'number';
      inp.type = numeric ? 'number' : 'text';
      if (numeric) inp.step = 'any';
      inp.value = v;
      inp.oninput = () => setAt(draft, here, numeric ? Number(inp.value) : inp.value);
      l.append(inp);
      parent.append(l);
    });
  };
  const setAt = (o, path, val) => {
    let cur = o;
    for (let i = 0; i < path.length - 1; i++) cur = cur[path[i]];
    cur[path[path.length - 1]] = val;
  };
  render(draft, card, []);

  const row = el('div', 'row');
  const save = el('button', 'primary', 'Save to game');
  save.disabled = !!d.game_running;
  save.onclick = async () => {
    const res = await api('gamesettings/write', { file: gsFile, values: draft });
    toast(res.ok ? `Saved ${res.bytes} bytes (backup kept)` : (res.error || 'Failed'),
          !res.ok);
  };
  const rest = el('button', 'sm danger', 'Restore backup');
  rest.onclick = async () => {
    if (!confirm('Restore this file from the ACECM backup?')) return;
    const res = await api('gamesettings/restore', { file: gsFile });
    toast(res.ok ? 'Restored' : (res.error || 'Failed'), !res.ok);
    gameSettingsPage();
  };
  const note = el('div', 'tiny dim grow',
    'Every save backs up first, and keeps a timestamped copy.');
  row.append(save, rest, note);
  card.append(row);
  p.append(card);
}


/* -------------------------------------------------------------- browser -- */
let brFilter = '', brSort = 'players';
/* What this machine already has, so a row can say "you are missing this"
   without a round trip per server. */
let brLocal = { tracks: [], cars: [] };

/* Ask the host beside a game server for whatever we lack.

   ⚠ Only an ACECM can answer. The dedicated server's archive holds a track's
   LOGIC - seven scene files - while a playable track is the ~1 GB of art in
   the player's own folder. Those bytes exist only on the host's game install,
   so a stock Kunos server has nothing to give and we say so. */
async function contentFrom(s) {
  toast('Looking for ACECM on ' + s.server_ip + '…');
  const d = await api(`browser/discover?host=${encodeURIComponent(s.server_ip)}`);
  if (!d.ok) { toast(d.error || 'host is not sharing content', true); return; }
  if (!(d.servers || []).length) {
    toast('That host runs ACECM but publishes no content', true); return;
  }
  // ⚠ Ask about EVERYTHING the host publishes, not just the entry matching
  // the track. A host lists one entry per thing - tracks and car mods - so
  // picking one meant the car mods were never fetched, and a server needing
  // both looked half-satisfied.
  toast('Checking what you need…');
  const plans = await Promise.all(d.servers.map(e =>
    api(`browser/plan?base=${encodeURIComponent(d.base)}`
        + `&id=${encodeURIComponent(e.id)}`).then(p => ({ e, p }))));
  const wanted = plans.filter(x => x.p.ok && (x.p.need || []).length);
  if (!wanted.length) {
    toast('You already have everything this host offers'); return;
  }
  const files = wanted.reduce((a, x) => a + x.p.need.length, 0);
  const mb = (wanted.reduce((a, x) => a + (x.p.bytes || 0), 0) / 1e6).toFixed(0);
  if (!confirm(wanted.map(x => '• ' + x.e.name).join('\n')
      + `\n\nMissing ${files} file(s), ${mb} MB.`
      + `\n\nDownload from ${d.base} and install?`)) return;
  const r = await api('browser/install',
                      { base: d.base, ids: wanted.map(x => x.e.id) });
  if (!r.ok) { toast(r.error || 'install failed', true); return; }
  const bar = $('#brprog');
  const poll = setInterval(async () => {
    const st = await api('browser/status');
    if (bar) bar.textContent = st.total
      ? `${st.detail} — ${(st.done / 1e6).toFixed(0)}/${(st.total / 1e6).toFixed(0)} MB`
      : st.detail;
    if (st.state === 'done' || st.state === 'error') {
      clearInterval(poll);
      toast(st.state === 'done' ? 'Content installed — you can join now'
                                : st.detail, st.state === 'error');
      brLocal = await api('browser/local');
      browserPage();
    }
  }, 1000);
}

async function browserPage() {
  const p = $('#page');
  p.innerHTML = '';
  const d = await api('browser');
  brLocal = await api('browser/local');

  // ⚠ An empty browser is never self-explanatory. The list is CAPTURED from
  // the game's own traffic, so it stays empty and silent whenever any link in
  // the chain is missing - most often an unpatched client, which talks
  // straight to Kunos and never passes anything through us. Show the chain.
  if (!d.ok || !(d.servers || []).length) {
    const why = await api('browser/why');
    const c = el('div', 'card');
    c.innerHTML = '<h2>Server browser</h2>'
      + '<div class="tiny dim" style="margin-bottom:10px">The list is not '
      + 'downloaded — it is captured from the game as it asks Kunos, which '
      + 'only works if the client is talking through our backend.</div>';
    (why.steps || []).forEach(s => {
      const row = el('div', 'chk');
      row.innerHTML = `<span class="name">${esc(s.what)}`
        + (s.ok ? '' : `<div class="tiny dim">${esc(s.fix)}</div>`)
        + (s.detail ? `<div class="tiny dim">${esc(s.detail)}</div>` : '')
        + '</span>'
        + `<span class="pill ${s.ok ? 'on' : 'bad'}"><i class="dot"></i>`
        + `${s.ok ? 'ok' : 'missing'}</span>`;
      c.append(row);
    });
    if (why.blocked_on) {
      c.append(el('div', 'warn',
        `<b>Stuck on:</b> ${esc(why.blocked_on)}`));
    }
    if (d.error || d.hint) {
      c.append(el('div', 'tiny dim', esc(d.hint || d.error)));
    }
    p.append(c);
    if (!(d.servers || []).length) return;
  }

  const age = d.captured_at ? Math.round(Date.now()/1000 - d.captured_at) : null;
  const head = el('div', 'card');
  head.innerHTML = `<h2>Public servers &middot; ${d.count || 0}</h2>`
    + '<div class="tiny dim" style="margin-bottom:9px">Captured from the real '
    + 'Kunos list as it passed through the proxy'
    + (age !== null ? ` — ${age < 90 ? age + 's' : Math.round(age/60) + 'm'} ago` : '')
    + '. Open Multiplayer in-game to refresh it.</div>';
  const row = el('div', 'row wrap');
  const search = el('input');
  search.placeholder = 'Filter by name, track or car…';
  search.value = brFilter;
  search.oninput = () => { brFilter = search.value.toLowerCase(); render(); };
  const sort = el('select');
  [['players','Most players'],['ping','Lowest ping'],['name','Name'],
   ['track','Track']].forEach(([v,l]) => {
    const o = el('option', null, l); o.value = v;
    if (v === brSort) o.selected = true; sort.append(o);
  });
  sort.onchange = () => { brSort = sort.value; render(); };
  row.append(search, sort);
  head.append(row);
  p.append(head);

  // ⚠ Works with the game CLOSED. The list is remembered from the last time
  // the in-game browser was open, and content comes from each host's ACECM -
  // neither needs the game running, which is the point: get the mods in place
  // BEFORE you next start it.
  const scan = el('button', 'sm', 'Find downloadable content');
  scan.title = 'Ask the hosts running a track you do not have whether they '
    + 'can send it to you';
  scan.onclick = async () => {
    scan.disabled = true;
    scan.textContent = 'asking hosts…';
    const r = await api('browser/scan', { only_missing: true });
    scan.disabled = false;
    scan.textContent = 'Find downloadable content';
    if (!r.ok) { toast(r.error || 'scan failed', true); return; }
    const box = $('#brprog');
    if (!r.hosts.length) {
      box.textContent = `asked ${r.probed} host(s) — none are sharing content`;
      return;
    }
    box.innerHTML = '';
    r.hosts.forEach(h => {
      const line = el('div', 'row wrap');
      const mb = h.entries.reduce((a, e) => a + (e.bytes || 0), 0) / 1e6;
      // ⚠ Tracks AND cars. This comes from the host's own ACECM, not from the
      // lobby capture, so it is answerable with the game shut - which is the
      // whole point of asking hosts directly.
      const tracks = [...new Set([].concat(
        ...h.entries.map(e => e.tracks || [])))];
      const mods = [...new Set([].concat(
        ...h.entries.map(e => e.mods || [])))];
      const need = [];
      if (tracks.length) need.push(tracks.join(', '));
      if (mods.length) need.push(`${mods.length} car mod`
        + (mods.length === 1 ? '' : 's'));
      line.append(el('span', 'tiny',
        `<b>${esc(h.server || h.ip)}</b> — ` + esc(need.join(' · '))
        + (mb ? ` · ${mb.toFixed(0)} MB` : '')));
      const get = el('button', 'sm primary', 'Download');
      get.onclick = () => contentFrom({ server_ip: h.ip,
                                        server_tcp_port: h.port,
                                        track: '' });
      line.append(get);
      box.append(line);
    });
  };
  row.append(scan);

  const prog = el('div', 'tiny dim');
  prog.id = 'brprog';
  head.append(prog);

  const tblCard = el('div', 'card');
  p.append(tblCard);

  function render() {
    tblCard.innerHTML = '';
    let rows = (d.servers || []).filter(s => {
      if (!brFilter) return true;
      return [s.server_name, s.track, s.layout, s.event_name]
        .some(x => String(x || '').toLowerCase().includes(brFilter));
    });
    const num = v => (typeof v === 'number' ? v : 0);
    rows.sort((a, b) => {
      if (brSort === 'players') return num(b.players) - num(a.players);
      if (brSort === 'ping') return (num(a.ping) || 9999) - (num(b.ping) || 9999);
      if (brSort === 'track') return String(a.track||'').localeCompare(String(b.track||''));
      return String(a.server_name||'').localeCompare(String(b.server_name||''));
    });
    if (!rows.length) { tblCard.append(el('div', 'empty', 'No matches')); return; }

    const t = el('table');
    t.innerHTML = '<thead><tr><th>Server</th><th>Track</th><th>Players</th>'
      + '<th>Ping</th><th>Mode</th><th></th></tr></thead>';
    const body = el('tbody');
    rows.slice(0, 300).forEach(s => {
      const tr = el('tr');
      const locked = s.driver_password ? ' 🔒' : '';
      tr.innerHTML = `<td>${esc(s.server_name || '(unnamed)')}${locked}
          <div class="tiny dim">${esc(s.server_ip||'')}:${s.server_tcp_port||''}</div></td>
        <td>${esc(s.track||'')}<div class="tiny dim">${esc(String(s.layout||'').replace(/^layout_/,''))}</div></td>
        <td>${num(s.players)}/${num(s.max_players)}</td>
        <td class="dim">${num(s.ping) || '—'}</td>
        <td class="tiny dim">${esc(String(s.current_session||''))}</td>`;
      // Do we have this track? Answered against the client's OWN tracks.table
      // (display name -> folder), which is exactly what the game consults, so
      // there is no name guessing and nothing for the user to configure. A
      // track absent from that table cannot be loaded, full stop.
      const want = String(s.track || '').trim();
      const map = brLocal.track_map || {};
      const folder = map[want];
      if (want && !folder) {
        const warn = el('div', 'tiny');
        warn.textContent = '⚠ you do not have this track';
        warn.style.color = 'var(--bad, #e5a13c)';
        tr.children[1].append(warn);
      } else if (folder) {
        tr.children[1].append(el('div', 'tiny dim', esc(folder)));
      }

      // ⚠ Cars too, not just the track. A server can restrict itself to cars
      // you do not own, and the game only tells you at the point of refusal.
      // An empty list means every car, so it is never "missing".
      const wantCars = [].concat(s.allowed_cars_list || [],
                                 s.allowed_cars_list_full || [])
        .filter(x => typeof x === 'string' && x);
      if (wantCars.length && brLocal.car_ids) {
        const have = new Set(brLocal.car_ids);
        const lack = [...new Set(wantCars.filter(c => !have.has(c)))];
        if (lack.length) {
          const w = el('div', 'tiny');
          w.textContent = `⚠ ${lack.length} car`
            + (lack.length === 1 ? '' : 's') + ' you do not have';
          w.title = lack.slice(0, 12).join('\n');
          w.style.color = 'var(--bad, #e5a13c)';
          tr.children[0].append(w);
        }
      }

      const td = el('td');
      const b = el('button', 'sm', 'Copy join');
      b.onclick = async () => {
        const link = `join:${s.server_ip}:${s.server_tcp_port}`;
        try { await navigator.clipboard.writeText(link); } catch (e) {}
        toast('Copied ' + link + ' — use the clipboard button in-game');
      };
      td.append(b);
      // Content can only come from an ACECM running beside that server: the
      // dedicated server holds a track's logic, never its art.
      const get = el('button', 'sm', 'Get content');
      get.title = 'Ask this host\'s ACECM for anything you are missing';
      get.onclick = () => contentFrom(s);
      td.append(get);
      tr.append(td);
      body.append(tr);
    });
    t.append(body);
    tblCard.append(t);
  }
  render();
}


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
    + 'the pits cannot be seen at all.</div>';
  const row = el('div', 'row wrap');
  const st = el('span', 'pill off', '<i class="dot"></i>checking');
  const go = el('button', 'primary sm', 'Start telemetry');
  go.onclick = async () => { const r = await api('telemetry/start', { id: telProfile });
    toast(r.ok ? 'Telemetry started' : (r.error || 'Failed'), !r.ok);
    setTimeout(telemetryPage, 2500); };
  const sp = el('button', 'sm danger', 'Stop');
  sp.onclick = async () => { await api('telemetry/stop', { id: telProfile }); toast('Stopped');
    setTimeout(telemetryPage, 800); };
  row.append(go, sp, st);
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


/* ---------------------------------------------------------------- logs --- */
let logTimer = null, logFollow = true;
async function logsPage() {
  const p = $('#page');
  p.innerHTML = '';
  const card = el('div', 'card');
  card.innerHTML = '<h2>Logs</h2>'
    + '<div class="tiny dim">Everything ACECM does, plus full tracebacks for '
    + 'any error. Newest at the bottom.</div>';
  const row = el('div', 'row wrap');
  row.style.margin = '10px 0';
  const pick = el('select');
  const follow = el('button', 'sm primary', 'Following');
  const refresh = el('button', 'sm', 'Refresh');
  const openDir = el('button', 'sm', 'Show folder');
  const info = el('span', 'tiny dim grow');
  row.append(pick, refresh, follow, openDir, info);
  const pre = el('pre', 'log');
  pre.style.maxHeight = '62vh';
  card.append(row, pre);
  p.append(card);

  const files = await api('logs/files');
  (files.files || []).forEach(f => {
    const o = document.createElement('option');
    o.value = f.name;
    o.textContent = `${f.name}  (${(f.size / 1024).toFixed(0)} KB)`;
    pick.append(o);
  });
  info.textContent = files.dir || '';
  openDir.onclick = () => { navigator.clipboard.writeText(files.dir || '');
    toast('Log folder path copied'); };
  follow.onclick = () => { logFollow = !logFollow;
    follow.textContent = logFollow ? 'Following' : 'Paused';
    follow.className = logFollow ? 'sm primary' : 'sm'; };

  async function pull() {
    const r = await api('logs?lines=500&file=' + encodeURIComponent(pick.value || 'acecm.log'));
    if (!r.ok) { pre.textContent = r.error || 'no log yet'; return; }
    const atBottom = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 40;
    pre.textContent = (r.lines || []).join(String.fromCharCode(10));
    // Colourless but scannable: keep the view pinned unless the user scrolled up.
    if (logFollow && atBottom) pre.scrollTop = pre.scrollHeight;
  }
  refresh.onclick = pull;
  pick.onchange = pull;
  await pull();
  logTimer = setInterval(() => { if (logFollow) pull(); }, 3000);
}

/* ----------------------------------------------------------------- nav --- */
const PAGES = {
  dashboard: ['Dashboard', 'Everything at a glance', dashboard],
  servers: ['Servers', 'Create, configure and run dedicated servers', serversPage],
  cars: ['Cars', 'What the dedicated server can actually load', carsPage],
  content: ['Content', 'Install and deploy cars and tracks', contentPage],
  tracks: ['Tracks', 'Layouts available to host', tracksPage],
  backend: ['Backend', 'Host through our own lobby', backendPage],
  browser: ['Server browser', 'Every public EVO server', browserPage],
  telemetry: ['Telemetry', 'Live car positions from the server', telemetryPage],
  patches: ['Patches', 'Verified, reversible binary patches', patchesPage],
  gamesettings: ['Game settings', 'FFB, graphics, audio and bindings', gameSettingsPage],
  logs: ['Logs', 'What ACECM did, and every error in full', logsPage],
  settings: ['Settings', 'Paths and ports', settingsPage],
};
/* ---- keep the view where the user left it -------------------------------
   Every page rebuilds itself wholesale (`p.innerHTML = ''`) and most actions
   finish by calling their page function again. That threw the scroll back to
   the top mid-task, and blanking to "Loading…" first made it flick as well -
   which is the jumping around: the list you were reading is briefly gone, then
   back at the top.

   So: remember the scroll position per page, restore it after a rebuild, and
   only show the placeholder when actually NAVIGATING somewhere new. A page
   refreshing itself now keeps its old content on screen until the new content
   is ready.                                                                */
const scroller = () => document.scrollingElement || document.documentElement;
const _scrollPos = {};
let _page = '';
addEventListener('scroll', () => {
  if (_page) _scrollPos[_page] = scroller().scrollTop;
}, { passive: true });

// ⚠ One render at a time, per page. Every page function clears #page and then
// awaits its data, so two overlapping calls both clear an empty page and then
// both append - and the whole dashboard appears TWICE. It only became visible
// when a page grew an extra await, but the race was always there.
const _rendering = {};

function keepPlace(name, fn) {
  return async function (...args) {
    if (_rendering[name]) return;          // a render is already in flight
    _rendering[name] = true;
    // a self-refresh should come back to where the user was, a fresh
    // navigation should start at the top
    const want = _page === name ? (_scrollPos[name] || 0) : 0;
    try {
      return await fn.apply(this, args);
    } finally {
      _rendering[name] = false;
      _page = name;
      requestAnimationFrame(() => {
        if (want) scroller().scrollTop = want;
      });
    }
  };
}
Object.entries(PAGES).forEach(([name, spec]) => {
  const orig = spec[2];
  const wrapped = keepPlace(name, orig);
  spec[2] = wrapped;
  // pages call themselves by name after an action, so rebind the global too -
  // otherwise only navigation would be covered and the common case would not
  if (orig.name) window[orig.name] = wrapped;
});

function go(name) {
  if (typeof telTimer !== 'undefined' && telTimer) { clearInterval(telTimer); telTimer = null; }
  if (typeof telRaf !== 'undefined' && telRaf) { cancelAnimationFrame(telRaf); telRaf = null; }
  const [title, sub, fn] = PAGES[name] || PAGES.dashboard;
  $('#ttl').textContent = title;
  $('#sub').textContent = sub;
  document.querySelectorAll('nav a').forEach(a =>
    a.classList.toggle('on', a.dataset.page === name));
  // ⚠ Only blank when the page is actually changing. Blanking on a refresh is
  // what makes the content flick away and come back.
  if (name !== _page) $('#page').innerHTML = '<div class="empty">Loading…</div>';
  fn().catch(e => { $('#page').innerHTML = ''; toast(String(e), true); });
  location.hash = name;
}
document.querySelectorAll('nav a').forEach(a =>
  a.onclick = () => go(a.dataset.page));
go((location.hash || '#dashboard').slice(1));
api('state').then(s => {
  $('#navfoot').textContent = s.server_exe_ok ? 'server ready' : 'server not found';
});
