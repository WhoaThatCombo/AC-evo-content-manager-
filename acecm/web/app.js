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
  const s = await api('state');
  const profs = (await api('profiles')).profiles || [];
  const cars = await api('cars');
  const trk = await api('tracks');
  const b = s.backend || {};
  const p = $('#page');
  p.innerHTML = '';

  if (!s.server_exe_ok)
    p.append(el('div', 'err',
      'Dedicated server executable not found. Set <b>server_dir</b> / '
      + '<b>server_exe</b> in Settings.'));

  const g = el('div', 'grid g3');
  const tiles = [
    [profs.length, 'server profiles'],
    [cars.total ?? '—', `cars (${cars.mods ?? 0} modded)`],
    [trk.total ?? '—', 'track layouts'],
    [b.listening ? 'UP' : 'DOWN', `own backend :${b.port ?? '—'}`],
  ];
  tiles.forEach(([v, k]) => g.append(el('div', 'stat',
    `<b>${esc(v)}</b><span>${esc(k)}</span>`)));
  p.append(g);

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
  const bJoin = el('button', null, 'Join server directly');
  bJoin.title = 'Pushes the client straight into the server, skipping the menus';
  bJoin.onclick = async () => {
    if (!profs.length) { toast('Create a server profile first'); return; }
    const js = await api('join/state');
    if (!js.control) { toast(js.hint || 'Start the proxy backend first', true); return; }
    if (!js.client_connected) { toast('No game client attached — launch the game first', true); return; }
    const r = await api('join', { id: profs[0].server_id || 'local-0000-0000-0000-000000000001' });
    toast(r.ok ? 'Sent go-to-server' : (r.error || 'Join failed'), !r.ok);
  };
  const bGame = el('button', null, 'Launch game');
  bGame.onclick = async () => { const r = await api('game/launch', {}); if (r.ok) toast('Launching game'); };
  row.append(bStart, bStop, bGame, bJoin);
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
  const p = $('#page');
  p.innerHTML = '';

  const head = el('div', 'row');
  const add = el('button', 'primary', '+ New server');
  add.onclick = () => { editing = { ...template }; serversPage(); };
  head.append(add);
  const stopAll = el('button', 'danger', 'Stop all');
  stopAll.onclick = async () => { await api('server/stop', {}); toast('Stopped'); };
  head.append(stopAll);
  p.append(el('div', 'card').appendChild(head).parentElement);

  if (editing) p.append(editor(editing, trk, options || {}));

  if (!profiles.length && !editing) {
    p.append(el('div', 'card').appendChild(
      el('div', 'empty', 'No server profiles yet. Create one to get going.')).parentElement);
  }

  for (const prof of profiles) {
    const card = el('div', 'card');
    const t = trk.find(x => x.index === prof.track_index);
    card.innerHTML = `<div class="row"><div class="grow">
        <b>${esc(prof.name)}</b>
        <div class="tiny dim">${esc(t ? t.label : 'track ' + prof.track_index)}
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
    start.onclick = async () => { await api('server/start', { id: prof.id }); toast('Starting…'); };
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
    row.append(start, edit, logs, del, tOn, tOff, tView, tpill);
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

function editor(prof, trk, opts) {
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

  const section = (title, hint) => {
    c.append(el('div', 'tiny dim', `<b style="color:var(--fg)">${esc(title)}</b>`
      + (hint ? ` — ${hint}` : '')));
    const g = el('div', 'grid g2');
    g.style.margin = '6px 0 12px';
    c.append(g);
    return g;
  };

  let g = section('Identity');
  mk(g, 'name', 'Server name', 'text');
  mk(g, 'track_index', 'Track / layout', 'select',
     trk.map(t => ({ value: t.index, label: t.label })));

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

  g = section('Session pacing', 'both were hardcoded to 10 before');
  mk(g, 'overtime_wait', 'Overtime wait, next session (s)', 'number');
  mk(g, 'max_wait_to_box', 'Max wait to box (s)', 'number');

  g = section('In-game date', 'the clock starts here; multiplier 0 freezes it');
  mk(g, 'tod_year', 'Year', 'number');
  mk(g, 'tod_month', 'Month', 'number');
  mk(g, 'tod_day', 'Day', 'number');
  mk(g, 'tod_second', 'Second', 'number');

  g = section('Visibility & output');
  mk(g, 'no_lobby', 'Private — do NOT list in the server browser', 'bool');
  mk(g, 'write_results', 'Write server results', 'bool');
  mk(g, 'export_json', 'Export season JSON', 'bool');

  g = section('Handicaps', 'per car: name:ballast:restrictor, comma separated');
  mk(g, 'car_handicaps', 'Car handicaps', 'text');

  g = section('Custom track',
    'A custom track borrows a stock track’s slots, so the event keeps the ' +
    'host’s name. Name what is actually deployed there.');
  mk(g, 'track_label', 'Deployed track name (blank = stock)', 'text');

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
  const hint = el('div', 'tiny dim grow',
    'Cars: empty means every Kunos car plus any installed mod cars.');
  row.append(save, cancel, hint);
  c.append(row);
  return c;
}

/* ----------------------------------------------------------------- cars -- */
let carFilter = '';
async function carsPage() {
  const d = await api('cars');
  const p = $('#page');
  p.innerHTML = '';
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
  const c = el('div', 'card');
  c.innerHTML = `<h2>Tracks &middot; ${d.total ?? 0} layouts</h2>`;
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
    + `${b.have_cert ? 'TLS keypair present' : 'no TLS keypair — run backend/gencert.sh'}</span>`));
  p.append(c);

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
  c.append(save);
  p.append(c);
}


/* -------------------------------------------------------------- content -- */
let scanned = null;
async function contentPage() {
  const p = $('#page');
  p.innerHTML = '';
  const mods = await api('mods');

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
    + `client <code>${esc(au.client_dir || '')}</code></div>`;
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

  // --- custom track deploy -------------------------------------------------
  const td = await api('trackdeploy');
  const dc = el('div', 'card');
  dc.innerHTML = '<h2>Deploy a custom track</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">'
    + 'A custom track cannot be dropped in as loose files: the dedicated server '
    + 'has no loose path for track logic, and the engine resolves content by '
    + 'hash against the archive index, so a new path cannot be found at all. '
    + 'A package installs by <b>borrowing an existing track’s slots</b> '
    + 'inside <code>content.kspkg</code>.</div>'
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
    const go = el('button', 'sm primary', 'Deploy');
    go.disabled = !pk.ok || td.server_running;
    go.onclick = async () => {
      if (!confirm('Deploy "' + (pk.display_name || pk.path)
          + '" into the server archive?\n\nThe archive is backed up first.')) return;
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
async function browserPage() {
  const p = $('#page');
  p.innerHTML = '';
  const d = await api('browser');

  if (!d.ok) {
    p.append(el('div', 'card', '<h2>Server browser</h2>'
      + `<div class="empty">${esc(d.hint || d.error || 'unavailable')}</div>`));
    return;
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
      const td = el('td');
      const b = el('button', 'sm', 'Copy join');
      b.onclick = async () => {
        const link = `join:${s.server_ip}:${s.server_tcp_port}`;
        try { await navigator.clipboard.writeText(link); } catch (e) {}
        toast('Copied ' + link + ' — use the clipboard button in-game');
      };
      td.append(b);
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
function go(name) {
  if (typeof telTimer !== 'undefined' && telTimer) { clearInterval(telTimer); telTimer = null; }
  if (typeof telRaf !== 'undefined' && telRaf) { cancelAnimationFrame(telRaf); telRaf = null; }
  const [title, sub, fn] = PAGES[name] || PAGES.dashboard;
  $('#ttl').textContent = title;
  $('#sub').textContent = sub;
  document.querySelectorAll('nav a').forEach(a =>
    a.classList.toggle('on', a.dataset.page === name));
  $('#page').innerHTML = '<div class="empty">Loading…</div>';
  fn().catch(e => { $('#page').innerHTML = ''; toast(String(e), true); });
  location.hash = name;
}
document.querySelectorAll('nav a').forEach(a =>
  a.onclick = () => go(a.dataset.page));
go((location.hash || '#dashboard').slice(1));
api('state').then(s => {
  $('#navfoot').textContent = s.server_exe_ok ? 'server ready' : 'server not found';
});
