/* ACECM UI - home. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* ---------------------------------------------------------------- home -- */
/* The landing page: pick up the last session in one click, see and start
   your own servers, and the handful of actions people reach for first.

   ⚠ Real data only. It reads the saved Drive pick and the profiles Drive
   already lists (one /api/drive call) and the live feed - there is no
   "recently driven" history in ACECM, so this page does not pretend there
   is one. */
async function homePage() {
  const d = await api('drive');
  const p = $('#page');
  p.innerHTML = '';
  if (!d || (d.error && !d.cars)) {
    p.append(el('div', 'err', esc((d && d.error) || 'could not load')));
    return;
  }
  const pick = d.pick || {};
  const car = (d.cars || []).find(c => c.id === pick.car) || null;
  const via = pick.via === 'local' || pick.via === 'server' ? pick.via : 'sp';
  const local = via === 'local'
    ? (d.local_servers || []).find(s => s.id === pick.local_id) : null;
  const track = via === 'sp'
    ? ((d.tracks || []).find(t => pick.custom_track
        ? t.custom_track === pick.custom_track
        : t.index === pick.track_index) || null)
    : null;
  const wrap = el('div', 'home');
  p.append(wrap);

  /* ---- continue -------------------------------------------------------- */
  const hero = el('section', 'hero');
  hero.setAttribute('aria-label', 'Continue');
  const trackFolder = (track && track.track) || (local && local.track) || '';
  if (trackFolder) {
    const bg = el('img', 'bg');
    bg.alt = '';
    bg.src = 'api/thumb/track?folder=' + encodeURIComponent(trackFolder);
    bg.onerror = () => bg.remove();
    hero.append(bg);
  }
  const txt = el('div', 'txt');
  const where = via === 'sp'
    ? (track ? (track.label || track.name) : 'Pick a track')
    : (via === 'local' ? (local ? local.name : 'Your server')
                       : (pick.server_ip ? pick.server_ip + ':' + pick.server_tcp_port
                                         : 'A public server'));
  txt.append(el('div', 'kick', car ? 'Pick up where you left off' : 'Ready when you are'));
  txt.append(el('h2', null, car
    ? esc(car.label) + '<br>' + esc(where)
    : 'Choose a car<br>and a track'));
  if (car) {
    const chips = el('div', 'chips');
    const mode = String(pick.game_mode || '').replace(/_/g, ' ').toLowerCase();
    [via === 'sp' ? mode : (via === 'local' ? 'your server' : 'online'),
     via === 'sp' ? String(pick.weather || '').toLowerCase().replace(/_/g, ' ') : '',
     via === 'sp' && pick.tod_hour != null
       ? String(pick.tod_hour).padStart(2, '0') + ':00' : '']
      .filter(Boolean).forEach(t => chips.append(el('span', null, esc(t))));
    txt.append(chips);
  }
  const acts = el('div', 'acts');
  const goBtn = el('button', 'primary bigbtn',
    car ? (via === 'sp' ? 'Drive again' : 'Join again') : 'Choose');
  goBtn.onclick = async () => {
    if (!car) { go('drive'); return; }
    goBtn.disabled = true;
    // the saved pick is exactly what Drive would send; the checklist on
    // Play shows the run, so go there once it has started
    const r = await api('drive', pick);
    goBtn.disabled = false;
    if (!r.ok) { toast(r.error || 'could not start', true); return; }
    liveKick();
    go('drive');
  };
  const change = el('button', null, 'Change car or track');
  change.onclick = () => go('drive');
  acts.append(goBtn);
  if (car) acts.append(change);
  txt.append(acts);
  hero.append(txt);
  if (car) {
    const pic = el('div', 'car');
    const img = el('img');
    img.alt = car.label || '';
    img.src = 'api/thumb/car?id=' + encodeURIComponent(car.model || car.id)
      + '&preset=' + encodeURIComponent(car.id);
    img.onerror = () => pic.remove();
    pic.append(img);
    hero.append(pic);
  }
  wrap.append(hero);

  /* ---- servers + quick actions ----------------------------------------- */
  const grid = el('div', 'hgrid');
  const srv = el('section', 'card');
  srv.setAttribute('aria-label', 'Your servers');
  const head = el('div', 'row');
  head.append(el('h2', 'grow', 'Your servers'));
  const manage = el('button', 'sm', 'Manage');
  manage.onclick = () => go('servers');
  head.append(manage);
  srv.append(head);
  const list = d.local_servers || [];
  if (!list.length) {
    const e = el('div', 'empty', 'No servers yet.');
    const mk = el('button', 'primary', 'Create one');
    mk.onclick = () => go('servers');
    e.append(el('div', null, ''), mk);
    srv.append(e);
  }
  const rows = {};
  for (const s of list) {
    const r = el('div', 'srow');
    const th = el('img');
    th.alt = '';
    th.src = 'api/thumb/track?folder=' + encodeURIComponent(s.track || '');
    th.onerror = () => { th.style.visibility = 'hidden'; };
    const g = el('div', 'grow');
    g.append(el('div', 'nm', esc(s.name)),
             el('div', 'tiny dim', esc([s.track, s.layout].filter(Boolean).join(' ')
               + ' · :' + s.tcp_port)));
    const state = el('span', s.running ? 'live' : 'off', s.running ? 'Live' : 'Stopped');
    const btn = el('button', 'sm', s.running ? 'Join' : 'Start');
    btn.onclick = async () => {
      if (rows[s.id].running) {
        // Play opens on this server; the user still chooses the car there
        _driveSel = { via: 'local', onlineVia: 'local', local_id: s.id };
        go('drive');
        return;
      }
      btn.disabled = true;
      btn.textContent = 'Starting…';
      const res = await api('server/start', { id: s.id });
      if (!res.ok) {
        btn.disabled = false;
        btn.textContent = 'Start';
        toast(res.error || 'could not start', true);
      }
      liveKick();
    };
    r.append(th, g, state, btn);
    srv.append(r);
    rows[s.id] = { state, btn, running: !!s.running };
  }
  // running / stopped follow the live feed, in place
  livePage('home', snap => {
    for (const v of snap.servers || []) {
      const row = rows[v.id];
      if (!row || row.running === !!v.running) continue;
      row.running = !!v.running;
      row.state.className = v.running ? 'live' : 'off';
      row.state.textContent = v.running ? 'Live' : 'Stopped';
      row.btn.disabled = false;
      row.btn.textContent = v.running ? 'Join' : 'Start';
    }
  });

  const qa = el('section', 'card');
  qa.setAttribute('aria-label', 'Quick actions');
  qa.append(el('h2', null, 'Quick actions'));
  const qg = el('div', 'qa');
  const quick = [
    ['Browse public servers', 'Join someone else\'s race', () => {
      _driveSel = { via: 'server', onlineVia: 'server' }; go('drive'); }],
    ['Get content', 'Fetch what a server needs', () => go('content')],
    ['New server', 'Host your own', () => go('servers')],
    ['Game settings', 'FFB, graphics, controls', () => go('gamesettings')],
  ];
  for (const [title, sub, fn] of quick) {
    const b = el('button', null, '<b>' + esc(title) + '</b><span>' + esc(sub) + '</span>');
    b.onclick = fn;
    qg.append(b);
  }
  qa.append(qg);
  grid.append(srv, qa);
  wrap.append(grid);
}
