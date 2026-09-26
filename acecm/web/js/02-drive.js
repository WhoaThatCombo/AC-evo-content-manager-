/* ACECM UI - drive. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* --------------------------------------------------------------- drive -- */
/* Content Manager's home: pick car + track + mode, then Drive walks the
   client into a dedicated session. EVO has no -car/-track launch flags. */
/* ⚠ The live Drive selection, kept across rebuilds of the page.
   drivePage() reconstructs `sel` from the SAVED pick, and the pick is only
   written when you press Drive/Join - so anything chosen and not yet started
   (sub-mode, server, car) was discarded whenever the page rebuilt itself,
   which it does after a capture finishes. That read as the app "jumping back
   to singleplayer and forgetting the car". */
let _driveSel = null;
let driveFilter = {
  car: '', track: '',
  sort: 'players',
  hideFull: true,
  hideLocked: false,
  hasPlayers: false,
  haveTrack: false,
  haveCar: false,
  acecmOnly: false,
  evoforgeOnly: false,
};
let driveLocal = null;
// the server a typed address or a favourite resolved to - it is deliberately
// not in the captured list, so serverOf falls back to this
let directPicked = null;

async function drivePage() {
  /* ⚠ Declared HERE, at the top, not next to the code that reads them.
     paintModeBar() assigns pullBtn while it builds the Public-servers row,
     and it runs long before the bottom of this function is reached - so a
     `let` further down put the binding in the temporal dead zone and the
     whole page died with "Cannot access 'pullBtn' before initialization",
     but ONLY in the Online > Public servers sub-mode, which is why it
     survived a build. Anything a paint function touches belongs up here. */
  // Refresh-list button, when the Public servers sub-mode has built one.
  let pullBtn = null;
  // bumped whenever something changes what the car LOOKS like
  let thumbBust = 0;
  const d = await api('drive');
  if (d && d.error && !d.cars) {
    $('#page').innerHTML = `<div class="err">${esc(d.error)}</div>`;
    return;
  }
  // Server car ids arrive interned - a pool plus indices into it - because
  // spelling them out for every server was three quarters of that payload.
  // Expand once, so every row/filter/search below still just sees a list of
  // ids. Mapping through the pool hands back the SAME string for each id
  // rather than 35k copies of 143 distinct ones.
  function expandPool(data) {
    const pool = data && data.car_pool;
    if (!pool || !Array.isArray(data.servers)) return;
    for (const s of data.servers) {
      if (Array.isArray(s.cars))
        s.cars = s.cars.map(i => (typeof i === 'number' ? pool[i] : i));
    }
  }
  expandPool(d);
  const pick = d.pick || {};
  const p = $('#page');
  p.innerHTML = '';

  const viaBar = el('div', 'drive-via');
  const wrap = el('div', 'drive');
  const carCol = el('div', 'card drive-col');
  const trkCol = el('div', 'card drive-col');
  const goCol = el('div', 'card drive-col drive-go');
  // Car and Track stack in one left column. As three equal columns they each
  // held a single row while owning two-thirds of the width, so the page was
  // mostly empty space with the session settings squeezed into the rest.
  const leftCol = el('div', 'drive-left');
  leftCol.append(carCol, trkCol);
  wrap.append(leftCol, goCol);
  p.append(viaBar, wrap);

  const sel = {
    /* ⚠ 'local' is a real sub-mode (Online > My servers). The old ternary
       collapsed anything that was not 'server' to 'sp', so a rebuild while
       you were on My servers dumped you into Single player. */
    via: (pick.via === 'server' || pick.via === 'local') ? pick.via : 'sp',
    server_id: pick.server_id || '',
    local_id: pick.local_id || '',
    server_ip: pick.server_ip || '',
    server_tcp_port: pick.server_tcp_port || 0,
    server_udp_port: pick.server_udp_port || 0,
    password: pick.password || '',
    car: pick.car || '',
    track_index: pick.track_index ?? 18,
    custom_track: pick.custom_track || '',
    game_mode: pick.game_mode || 'PRACTICE',
    weather: pick.weather || 'CLEAR',
    grip: pick.grip || 'OPTIMUM',
    tod_hour: pick.tod_hour ?? 13,
    num_opponents: pick.num_opponents ?? 10,
    skill_min: pick.skill_min ?? 80,
    skill_max: pick.skill_max ?? 95,
    aggressiveness: pick.aggressiveness || 'Safe',
    single_make: pick.single_make !== false,
    duration_min: pick.duration_min ?? 90,
    practice_min: pick.practice_min ?? 10,
    quali_min: pick.quali_min ?? 15,
    warmup_min: pick.warmup_min ?? 10,
    race_laps: pick.race_laps ?? 10,
    race_type: pick.race_type || 'LAPS',
    race_minutes: pick.race_minutes ?? 20,
    time_mult: pick.time_mult ?? 1,
    starting_position: pick.starting_position ?? 0,
  };
  /* ⚠ Carry the LIVE selection over a rebuild. The saved pick is only
     written on Drive/Join, so without this everything chosen since then was
     silently reverted whenever the page rebuilt - which it does on its own
     after a capture finishes. Only the fields the user actually picks are
     carried; everything else comes fresh from the server, so a rebuild still
     picks up new content and new servers.
     ⚠ Same object identity afterwards (Object.assign into `sel`, and `sel`
     stays the holder), because every paint closure below captures `sel`. */
  if (_driveSel) {
    for (const k of ['via', 'onlineVia', 'server_id', 'local_id', 'server_ip',
                     'server_tcp_port', 'server_udp_port', 'password', 'car',
                     'livery', 'track_index', 'custom_track', 'game_mode']) {
      if (_driveSel[k] !== undefined && _driveSel[k] !== '' &&
          _driveSel[k] !== null) sel[k] = _driveSel[k];
    }
  }
  _driveSel = sel;
  // ⚠ Must run again when the public list lands, not only here: the list is
  // fetched after this point now, so at page-build time there is nothing to
  // default to and "Join" would have had no server picked.
  function defaultServer() {
    if (sel.via !== 'server' || sel.server_id || sel.server_ip) return;
    const s0 = (d.servers || [])[0];
    if (!s0) return;
    sel.server_id = s0.id;
    sel.server_ip = s0.server_ip;
    sel.server_tcp_port = s0.server_tcp_port;
    sel.server_udp_port = s0.server_udp_port;
  }
  defaultServer();
  const aiModes = d.ai_modes || ['INSTANT_RACE', 'RACE_WEEKEND'];

  function carOf(id) {
    return (d.cars || []).find(c => c.id === id) || null;
  }
  function trackOf(idx) {
    return (d.tracks || []).find(t => t.index === idx) || null;
  }
  function serverOf() {
    const hit = (d.servers || []).find(s =>
      (sel.server_id && s.id === sel.server_id)
      || (sel.server_ip && s.server_ip === sel.server_ip
          && Number(s.server_tcp_port) === Number(sel.server_tcp_port))
    );
    if (hit) return hit;
    // ⚠ A directly-entered address is NOT in the captured list - that is the
    // whole point of it. Without this the card kept saying "Pick a public
    // server" for a server you had just successfully looked up.
    if (directPicked && directPicked.ip === sel.server_ip
        && Number(directPicked.tcp) === Number(sel.server_tcp_port)) {
      const p = directPicked;
      return {
        id: '', name: p.name, server_ip: p.ip,
        server_tcp_port: p.tcp, server_udp_port: p.udp || p.tcp,
        track: p.track || '', layout: p.layout || '',
        players: p.players || 0, max_players: p.max_players || 0,
        locked: !!p.locked, cars: p.cars || [], direct: true,
        note: p.note || '',
      };
    }
    return null;
  }
  function localOf() {
    return (d.local_servers || []).find(s => s.id === sel.local_id) || null;
  }
  function allowedCars() {
    const sv = sel.via === 'local' ? localOf() : serverOf();
    if ((sel.via !== 'server' && sel.via !== 'local') || !sv || !(sv.cars || []).length)
      return null;
    return new Set(sv.cars);
  }
  function carLabel(id) {
    const c = carOf(id);
    if (c && c.label) return c.label;
    return String(id || '')
      .replace(/^preset_/, '')
      .replace(/_mech_\d+$/, '')
      .replace(/_/g, ' ');
  }
  function carIsMod(id) {
    const c = carOf(id);
    if (c) return !!c.mod;
    return !/_mech_\d+$/.test(String(id || ''))
      || String(id || '').indexOf('preset_modded') >= 0;
  }
  function carAllowed(c, allow) {
    if (!allow) return true;
    if (allow.has(c.id) || (c.model && allow.has(c.model))) return true;
    for (const a of allow) {
      if (c.id && (c.id === a || a.indexOf(c.id) >= 0 || c.id.indexOf(a) >= 0))
        return true;
      if (c.model && (a.indexOf(c.model) >= 0 || c.model.indexOf(a) >= 0))
        return true;
    }
    return false;
  }
  function carsLine(s) {
    const cars = s.cars || [];
    if (!cars.length) return 'all cars';
    const names = cars.slice(0, 3).map(carLabel);
    let bit = names.join(', ');
    if (cars.length > 3) bit += ' +' + (cars.length - 3);
    const mods = cars.filter(carIsMod);
    if (mods.length)
      bit += mods.length === cars.length
        ? ' · mods'
        : ' · ' + mods.length + ' mod' + (mods.length === 1 ? '' : 's');
    return bit;
  }

  /* Two top-level modes, not five buttons in a row. "Single player" and
     "Online" are the actual decision; which KIND of online (your own server
     vs the public list) is a second, smaller choice that only appears once
     you are online. The old bar put all three modes plus "Full browser" and
     "Refresh list" on one line, so the first thing the page asked you was a
     five-way question where only two answers were top-level.
     Online remembers the sub-mode you last used, so nothing moves under you. */
  const ONLINE = ['local', 'server'];
  const isOnline = () => ONLINE.includes(sel.via);
  if (!sel.onlineVia) sel.onlineVia = isOnline() ? sel.via : 'local';

  const modeRow = el('div', 'row');
  const subRow = el('div', 'row subrow');

  function setVia(v) {
    sel.via = v;
    if (ONLINE.includes(v)) sel.onlineVia = v;
    paintModeBar();
    paintVia();
  }
  function paintModeBar() {
    modeRow.innerHTML = '';
    subRow.innerHTML = '';
    [['sp', 'Single player'], ['online', 'Online']].forEach(([v, lab]) => {
      const on = v === 'sp' ? sel.via === 'sp' : isOnline();
      const b = el('button', 'sm' + (on ? ' primary' : ''), lab);
      b.onclick = () => setVia(v === 'sp' ? 'sp' : sel.onlineVia);
      modeRow.append(b);
    });
    if (!isOnline()) { subRow.style.display = 'none'; return; }
    subRow.style.display = '';
    [['local', 'My servers'], ['server', 'Public servers']].forEach(([v, lab]) => {
      const b = el('button', 'sm' + (sel.via === v ? ' primary' : ''), lab);
      b.onclick = () => setVia(v);
      subRow.append(b);
    });
    // Actions belong to the sub-mode they act on, rather than sitting on the
    // top row where they read as modes themselves.
    if (sel.via === 'server') {
      const pull = el('button', 'sm', 'Refresh list');
      // ⚠ onDrive() disables this while a job runs, and it lives in a
      // different scope - hand it out through the outer binding.
      pullBtn = pull;
      pull.title = 'Launch the game, open Multiplayer, save the public list, then quit';
      pull.onclick = async () => {
        const r = await api('drive/capture', {});
        if (!r.ok) { toast(r.error || 'Could not start', true); return; }
        liveKick();
      };
      subRow.append(pull);
    }
    // ⚠ no "Full browser" link any more - the server-browser page is gone and
    // its one genuinely useful part (fetch from a host) now lives on Content.
    const manage = el('button', 'sm', 'Manage servers');
    manage.onclick = () => go('servers');
    subRow.append(manage);
  }
  viaBar.append(modeRow, subRow);
  paintModeBar();

  carCol.innerHTML = '<h2>Car</h2>';
  const carHead = el('div', 'drive-pick');
  const carSearch = el('input');
  carSearch.placeholder = 'Filter cars…';
  carSearch.value = driveFilter.car;
  const carList = el('div', 'list drive-list');
  const specsBox = el('div', 'car-specs');
  const liveryBox = el('div');
  liveryBox.style.marginTop = '10px';
  // ⚠ Directly under the SELECTED car, not after the list. The list is a
  // tall scrolling box of ~100 rows, so anything appended after it sits
  // below the fold and is invisible in normal use - which is exactly how
  // it was reported: "where do i pick the livery?"
  // ⚠ The search box and the list are built here but live in the picker
  // dialog. They stay in the DOM (hidden) rather than being created on
  // demand, because paintCars/paintTracks repaint them whether the dialog is
  // open or not - building them lazily would mean painting into nothing.
  carSearch.style.display = 'none';
  carList.style.display = 'none';
  carCol.append(carHead, specsBox, liveryBox, carSearch, carList);

  /* ---- livery -----------------------------------------------------------
     Colours come from the CAR's own design, never from the brand's folder:
     the brand list is a superset, and writing a colour the car does not offer
     crashes the game on load. Everything here is wrapped, because this column
     is the main screen and a fault in a nice-to-have must not take it down. */
  /* Headline numbers for the selected car, read from the game's own physics
     files (see carspecs.py). Power is DERIVED from the engine's torque curve
     and its boost, so it is close but not a manufacturer figure; top speed is
     the AI's hint and ignores a real limiter - both are labelled as such
     rather than presented as exact. A car we have nothing for (a mod, whose
     files live in its own package) simply shows no strip. */
  async function drawSpecs(carId) {
    specsBox.innerHTML = '';
    if (!carId) return;
    let sp = {};
    try {
      const r = await api('car/specs?preset=' + encodeURIComponent(carId));
      sp = (r && r.specs) || {};
    } catch (e) { return; }
    if (!Object.keys(sp).length) return;
    const bits = [];
    /* ⚠ `exact` means these came from the TRIM's own spec sheet in the game
       files, so they need no hedging. Only a car with no sheet (nearly every
       mod) falls back to numbers derived from the torque curve, and those
       keep the "~" and say so. */
    const exact = !!sp.exact;
    if (sp.power_hp) bits.push(['Power', (exact ? '' : '~') + sp.power_hp + ' hp',
      exact ? "The game's own figure for this trim"
        : 'Derived from the engine torque curve'
          + (sp.boost_bar ? ' at ' + sp.boost_bar + ' bar boost' : '')
          + ' - approximate, not a manufacturer figure']);
    if (sp.mass_kg) bits.push(['Weight', sp.mass_kg + ' kg', '']);
    if (sp.accel_s) bits.push(['0-100', sp.accel_s + ' s', "km/h, from the game's spec sheet"]);
    // ⚠ Spell the unit out. "PWR/WT 588 hp/t" reads as jargon and the number
    // looks wrong beside "382 hp" until you know it is scaled to a tonne.
    // "PER TONNE 588 hp" says what it is without needing the abbreviation.
    if (sp.hp_per_tonne) bits.push(['Per tonne', sp.hp_per_tonne + ' hp',
      'Power-to-weight: ' + (sp.power_hp || '?') + ' hp in '
      + (sp.mass_kg || '?') + ' kg, so ' + sp.hp_per_tonne
      + ' hp per 1000 kg. Higher is quicker.']);
    if (sp.top_speed_kmh) {
      bits.push(['Top spd', sp.top_speed_kmh + ' km/h',
        "The game's own figure for this trim"]);
    } else if (sp.top_speed_kmh_est) {
      bits.push(['Top spd', '~' + sp.top_speed_kmh_est,
        "The AI's estimate, in km/h - it ignores a manufacturer limiter"]);
    }
    if (sp.drive) bits.push(['Drive', sp.drive, '']);
    if (sp.gears) bits.push(['Gears', String(sp.gears), '']);
    if (sp.redline_rpm) bits.push(['Redline', Math.round(sp.redline_rpm / 100) / 10 + 'k',
      sp.redline_rpm.toLocaleString() + ' rpm']);
    bits.forEach(([k, v, tip]) => {
      const c = el('span', 'spec');
      c.innerHTML = `<b>${esc(k)}</b>${esc(v)}`;
      if (tip) c.title = tip;
      specsBox.append(c);
    });
  }

  async function drawLivery(carId) {
    liveryBox.innerHTML = '';
    if (!carId) return;
    try {
      const g = await api('livery');
      // the garage is keyed by MODEL; the picker's id is a preset
      const model = (((d.cars || []).find(c => c.id === carId)) || {}).model
                    || carId;
      const owned = ((g && g.cars) || []).find(c => c.model === model);
      // ⚠ SAY SO rather than showing nothing. A livery belongs to an owned
      // car, and only 9 of the 116 cars in this list are owned - so staying
      // silent meant the picker looked missing for almost every car you
      // clicked, which is exactly how it was reported.
      if (!owned) {
        /* ⚠ Offer the fix instead of only naming the problem. A livery is
           stored per owned car, so this used to be a dead end for almost
           every car in the list. ACECM can write the missing record from
           the state the GAME itself ships for that car, which is what this
           button does - for every car at once, not just this one. */
        const why = el('div', 'livery-head livery-none');
        why.textContent = 'no livery — this car has no garage record';
        const fix = el('button', 'sm', 'Add liveries for all cars');
        fix.style.marginTop = '6px';
        fix.title = 'Writes a garage record for every car that has none, '
          + "using the game's own default for that car. Cars you already "
          + 'own are left untouched.';
        fix.onclick = async ev => {
          ev.stopPropagation();
          fix.disabled = true;
          const was = fix.textContent;
          fix.textContent = 'Adding…';
          try {
            const r = await api('livery/populate', {});
            if (!r || !r.ok) {
              toast((r && r.error) || 'could not add liveries', true);
              return;
            }
            toast(`Added ${r.count} car(s) to the garage`);
            drawLivery(carId);
          } finally {
            fix.disabled = false;
            fix.textContent = was;
          }
        };
        liveryBox.append(why, fix);
        return;
      }
      const info = await api('livery?model=' + encodeURIComponent(model));
      const list = ((info && info.allowed) || {})['EXT SKIN'] || [];
      const names = (info && info.names) || {};
      if (!list.length) return;

      /* ⚠ A real <select>, not a disclosure row. The expanding list put a
         thirteen-item column under the car - and even collapsed it left a
         line of its own plus the dead space the card reserved for it. The
         colour is a one-line setting, so it looks like one. */
      const short = p => (p.split('\\').pop() || p)
        .replace(/\.oemmultilayercolor$/, '')
        .replace(/^[a-z0-9]+_paint_/, '').replace(/_/g, ' ');
      const cur = owned.slots['EXT SKIN'] || '';

      const row = el('label', 'livery-row');
      row.append(el('span', 'livery-lab', 'Livery'));
      const sel2 = el('select');
      sel2.title = 'Change this car’s colour';
      list.forEach(pth => {
        const o = document.createElement('option');
        o.value = pth;
        /* ⚠ The paint's OWN name first. Deriving a label from the filename
           works for Kunos ("bmw_paint_blue_zandvoort" -> "blue zandvoort")
           and fails for mods, whose files are ac1_paint_4 - the prefix strip
           left a bare "4" in the dropdown. */
        o.textContent = names[pth] || short(pth) || '(unnamed)';
        if (pth === cur) o.selected = true;
        sel2.append(o);
      });
      const note = el('span', 'tiny dim livery-note');
      sel2.onchange = async () => {
        const pth = sel2.value;
        sel2.disabled = true;
        note.textContent = 'applying…';
        const r = await api('livery/apply', {
          file: owned.file, model: model, slot: 'EXT SKIN', color: pth,
        });
        sel2.disabled = false;
        if (r && r.ok) {
          note.textContent = 'saved — applies next game start';
          // the car is a different colour now: re-render this card's picture
          thumbBust = Date.now();
          paintSelected();
        } else {
          note.textContent = (r && r.error) || 'could not apply that colour';
          if (r && r.error) toast(r.error, true);
        }
      };
      row.append(sel2, note);
      liveryBox.append(row);
    } catch (e) {
      // never let this blank the Drive screen
      console.warn('livery picker unavailable', e);
    }
  }

  trkCol.innerHTML = '<h2>Track</h2>';
  const trkHead = el('div', 'drive-pick');
  const trkSearch = el('input');
  trkSearch.placeholder = 'Filter tracks…';
  trkSearch.value = driveFilter.track;
  const srvFilters = el('div', 'drive-srv-filters');
  srvFilters.style.display = 'none';
  const directBox = el('div', 'drive-direct');
  directBox.style.display = 'none';
  const trkList = el('div', 'list drive-list');
  // same as the car column: these are the picker's contents, parked here.
  // srvFilters is left alone - paintSrvFilters already owns its visibility
  // and only shows it for the public-server list.
  trkSearch.style.display = 'none';
  trkList.style.display = 'none';
  /* ⚠ The preview and the LIST are two different things and they live in
     two different places. The card under the car is only ever a picture of
     what is selected - track in Single player, the server's track online.
     Everything you act on (search, filters, the rows, direct connect) is
     joining, so it belongs beside the Join button on the right. Putting the
     whole lot under the car turned the left column into the busy half and
     left the pane holding one button. */
  const srvBox = el('div', 'drive-srvbox');
  srvBox.append(directBox, trkSearch, srvFilters, trkList);
  trkCol.append(trkHead, srvBox);

  /* ---- direct connect + favourites --------------------------------------
     The captured list only exists after launching the game and opening
     Multiplayer, which is a lot of ceremony for "a friend sent me an IP".
     Joining never needed the list - the address is enough - so this is about
     seeing what you are about to join, and keeping the ones you go back to. */
  let favs = [];

  function selectServer(info) {
    sel.server_id = '';                 // an address, not a captured entry
    sel.server_ip = info.ip;
    sel.server_tcp_port = info.tcp;
    sel.server_udp_port = info.udp || info.tcp;
    directPicked = info;
    paintSelected();
    paintVia();
  }

  /* ⚠ Select FIRST, enrich after. Finding out whether a host runs ACECM
     means waiting on a connection that, for a plain game server, only ends
     when it times out - seconds during which the card still showed the
     previous server and the click looked ignored. The address alone is
     enough to join, so commit to it immediately and fill in the name, track
     and cars if and when they arrive. */
  async function lookupAndSelect(target, quiet) {
    const t = (target || '').trim();
    if (!t) return null;
    const m = t.match(/^\s*(?:\w+:\/\/)?\[?([^\]\/\s]+?)\]?(?::(\d+))?\s*$/);
    if (!m) { toast('type an address, like 1.2.3.4:9700', true); return null; }
    const ip = m[1], tcp = Number(m[2] || 9700);
    selectServer({ ip, tcp, udp: tcp, name: `${ip}:${tcp}`, cars: [],
                   pending: true });

    const r = await api('drive/direct', { target: t });
    if (!r || !r.ok) { toast((r && r.error) || 'could not read that', true); return null; }
    // the user may have moved on while we were waiting
    if (sel.server_ip !== ip || Number(sel.server_tcp_port) !== tcp) return r;
    selectServer(r);
    if (!quiet) {
      toast(r.source === 'list' ? `Found in your list — ${r.name}`
        : r.source === 'acecm' ? `ACECM host — ${r.name}`
        : `Ready to join ${r.ip}:${r.tcp} (details unknown)`);
    }
    return r;
  }

  async function paintDirect() {
    directBox.style.display = sel.via === 'server' ? '' : 'none';
    if (sel.via !== 'server') return;
    if (!directBox.dataset.ready) {
      directBox.dataset.ready = '1';
      const row = el('div', 'row wrap');
      const inp = el('input');
      inp.placeholder = 'Direct connect — 1.2.3.4:9700';
      inp.style.flex = '1';
      inp.style.minWidth = '12em';
      const go = el('button', 'sm primary', 'Connect');
      const star = el('button', 'sm', '☆');
      star.title = 'Save this address to favourites';
      go.onclick = () => lookupAndSelect(inp.value);
      // Enter is what people actually press after typing an address
      inp.onkeydown = e => { if (e.key === 'Enter') go.onclick(); };
      star.onclick = async () => {
        const t = inp.value.trim()
          || (sel.server_ip ? `${sel.server_ip}:${sel.server_tcp_port}` : '');
        if (!t) { toast('type an address first', true); return; }
        const r = await api('drive/fav/add', { target: t });
        if (!r || !r.ok) { toast((r && r.error) || 'could not save', true); return; }
        favs = r.favourites || [];
        toast('Saved to favourites');
        paintFavs();
      };
      row.append(inp, go, star);
      directBox.append(row, el('div', 'fav-list'));
    }
    if (!favs.length) {
      const r = await api('drive/fav');
      favs = (r && r.favourites) || [];
    }
    paintFavs();
  }

  function paintFavs() {
    const box = directBox.querySelector('.fav-list');
    if (!box) return;
    box.innerHTML = '';
    if (!favs.length) return;
    box.append(el('div', 'tiny dim fav-title', 'Favourites'));
    favs.forEach(f => {
      const r = el('div', 'fav-row'
        + (sel.server_ip === f.ip && sel.server_tcp_port === f.tcp ? ' on' : ''));
      const t = el('div', 'grow');
      t.innerHTML = `<div class="name">${esc(f.name)}</div>`
        + `<div class="tiny dim">${esc(f.ip)}:${f.tcp}</div>`;
      // ⚠ look it up again rather than trusting what was saved: the player
      // count and even the name will have moved on since it was pinned
      r.onclick = () => lookupAndSelect(`${f.ip}:${f.tcp}`, true);
      /* Fetch what this server needs, without going to the browser page and
         finding it in a list. Content only comes from a host running ACECM,
         and we cannot know whether it is until we ask - so the button is
         always here and says so plainly when the answer is no. */
      const get = el('button', 'sm', 'Get content');
      get.title = 'Download the tracks and mods this server needs';
      /* ⚠ Go through contentFrom, the SAME path the browser list uses.
         This used to ask drive/direct instead, which answers the question
         "what am I about to join?" - and for any server present in the
         captured list it answers from that list and returns early, without
         ever asking the host whether it runs ACECM. So it came back with no
         base, and a favourite was told its host "is not sharing content
         through ACECM" while the identical server, found by searching the
         browser, downloaded fine. Favouriting a server is how you get it
         into that list, so this failed for essentially every favourite. */
      get.onclick = async ev => {
        ev.stopPropagation();
        get.disabled = true;
        const was = get.textContent;
        get.textContent = 'Checking…';
        try {
          await contentFrom({
            server_ip: f.ip,
            server_tcp_port: f.tcp,
            track: '',
          });
        } finally {
          get.disabled = false;
          get.textContent = was;
        }
      };
      const x = el('button', 'sm', '×');
      x.title = 'Remove from favourites';
      x.onclick = async ev => {
        ev.stopPropagation();
        const res = await api('drive/fav/remove', { id: f.id });
        favs = (res && res.favourites) || [];
        paintFavs();
      };
      r.append(t, get, x);
      box.append(r);
    });
  }

  /* Two panes in the right column rather than one long scroll with a folded
     section at the bottom. Assists is a thing you set, not a thing you read
     past, so it gets equal billing and is always fully open once selected. */
  goCol.innerHTML = '';
  const paneTabs = el('div', 'panetabs');
  const sessionPane = el('div', 'pane');
  const assistPane = el('div', 'pane');
  assistPane.style.display = 'none';
  [['session', 'Session'], ['assists', 'Assists']].forEach(([k, lab]) => {
    const b = el('button', 'sm' + (k === 'session' ? ' primary' : ''), lab);
    b.dataset.pane = k;
    b.onclick = () => {
      paneTabs.querySelectorAll('button').forEach(x =>
        x.classList.toggle('primary', x.dataset.pane === k));
      sessionPane.style.display = k === 'session' ? '' : 'none';
      assistPane.style.display = k === 'assists' ? '' : 'none';
    };
    paneTabs.append(b);
  });
  goCol.append(paneTabs, sessionPane, assistPane);
  const mode = el('select');
  (d.game_modes || []).forEach(m => {
    const o = el('option', null, m.replace(/_/g, ' '));
    o.value = m;
    if (m === sel.game_mode) o.selected = true;
    mode.append(o);
  });
  const weather = el('select');
  (d.weather || []).forEach(m => {
    const o = el('option', null, m.replace(/_/g, ' '));
    o.value = m;
    if (m === sel.weather) o.selected = true;
    weather.append(o);
  });
  /* ⚠ Three steps, not a slider. The game stores track grip as an enum -
     UIInitialGrip_GREEN / _FAST / _OPTIMUM - on the weather preset. The only
     continuous grip in the schema belongs to championship data, which this
     screen never writes, so a 0-100 slider here would be inventing a
     precision the save file cannot hold. */
  const grip = el('select');
  [['OPTIMUM', 'Optimum — rubbered in'],
   ['FAST', 'Fast — some rubber'],
   ['GREEN', 'Green — dusty, low grip']].forEach(([v, lab]) => {
    const o = el('option', null, lab);
    o.value = v;
    if (v === sel.grip) o.selected = true;
    grip.append(o);
  });
  const hour = el('select');
  for (let h = 0; h < 24; h++) {
    const o = el('option', null, String(h).padStart(2, '0') + ':00');
    o.value = String(h);
    if (h === Number(sel.tod_hour)) o.selected = true;
    hour.append(o);
  }
  const mkf = (label, node) => {
    const l = el('label', 'f');
    l.append(el('span', null, label), node);
    return l;
  };
  const extras = el('div', 'drive-fields');
  /* ⚠ EXACTLY the values the game's own slider offers:
       <ks-slider values="[1,2,4,6,12,24,48]" data-setting="time_multiplier">
     Anything else makes the game show "undefined X" and apply no multiplier
     at all - which is what a free number field was doing here. */
  const tmult = el('select');
  [1, 2, 4, 6, 12, 24, 48].forEach(v => {
    const o = el('option', null, v + '×');
    o.value = String(v);
    if (Number(sel.time_mult) === v) o.selected = true;
    tmult.append(o);
  });
  if (![1, 2, 4, 6, 12, 24, 48].includes(Number(sel.time_mult))) {
    tmult.value = '1';
    sel.time_mult = 1;
  }
  tmult.title = 'How fast the clock runs. These are the only values the game '
    + 'accepts.';
  tmult.onchange = () => { sel.time_mult = Number(tmult.value); };
  sessionPane.append(mkf('Game mode', mode), mkf('Weather', weather),
                     mkf('Track grip', grip), mkf('Time', hour),
                     mkf('Time multiplier', tmult), extras);
  const driveBtn = el('button', 'primary go-btn', 'Drive');
  const st = el('div', 'drive-status tiny dim');
  const pwField = el('label', 'f');
  pwField.style.display = 'none';
  const pwInp = el('input');
  pwInp.type = 'password';
  pwInp.placeholder = 'Server password';
  pwInp.value = sel.password || '';
  pwInp.oninput = () => { sel.password = pwInp.value; };
  pwField.append(el('span', null, 'Password'), pwInp);
  const hint = el('div', 'tiny dim',
    'Writes the session, launches the game, and opens the pit menu '
    + 'so you can change setup. Close the game first so the save sticks.');
  // the step checklist sits right under the button it reports on
  const stepsBox = el('div', 'jobsteps');
  stepsBox.hidden = true;
  sessionPane.append(pwField, driveBtn, stepsBox, st, hint);
  // ⚠ computed BEFORE the assists block is added: it selects direct-child
  // label.f, and the assists fields live one level down so they cannot be
  // caught by the single-player show/hide.
  const spFields = sessionPane.querySelectorAll(':scope > label.f');

  /* ---- assists ----------------------------------------------------------
     The game stores these in the profile (Saved Games\ACE\AssistSettings), so
     writing them is exactly what the in-game Assists page does: no patching,
     nothing in content.kspkg, and joining a server is unaffected because the
     server hashes CAR CONTENT, not the profile.
     ⚠ Damage is shown as a percentage because that is what the game shows;
     it is stored offset by one (0% == -1.0). assists.py does the conversion. */
  const asBadge = el('div', 'tiny dim assists-note');
  const asBody = el('div', 'assists-body');
  assistPane.append(asBadge, asBody);

  const penBox = el('div', 'sp-pen');
  assistPane.append(penBox);

  async function loadSpPenalties() {
    penBox.innerHTML = '';
    let pens = null;
    try {
      pens = await api('penalties');
    } catch (e) { return; }
    const info = pens && pens.client;
    if (!info) return;
    const isOff = info.state === 'off';
    const isOn = info.state === 'on';
    const head = el('div', 'sp-pen-head');
    head.innerHTML = '<b>Single player penalties</b>'
      + `<span class="tiny dim">currently ${esc(isOff ? 'OFF' : isOn ? 'ON'
                                                : String(info.state))}</span>`;
    penBox.append(head);
    penBox.append(el('div', 'tiny dim',
      'Cutting, speeding in the pits and so on, for your own offline sessions. '
      + 'This edits the game’s own content, so a Kunos update resets it.'));
    if (isOff || isOn) {
      const b = el('button', 'sm' + (isOn ? ' danger' : ''),
                   isOn ? 'Turn penalties off' : 'Turn penalties on');
      b.onclick = async () => {
        b.disabled = true;
        const r = await api('penalties/set', { side: 'client', off: isOn });
        b.disabled = false;
        toast(r.ok ? `Single player penalties ${(r.state || '').toUpperCase()}`
                   : (r.error || 'Failed'), !r.ok);
        loadSpPenalties();
      };
      penBox.append(b);
    }
    matchPaneHeights();
  }

  async function loadAssists() {
    const r = await api('assists');
    asBody.innerHTML = '';
    if (!r || !r.ok) {
      asBadge.textContent = '';
      asBody.append(el('div', 'tiny dim', (r && r.error) || 'unavailable'));
      return;
    }
    asBadge.textContent = 'Applies to the game itself, not to a session - '
      + 'these are the same settings as the in-game Assists page.';
    const pending = {};
    const grid = el('div', 'assists-grid');
    function mkNum(key, label, val) {
      const l = el('label', 'f');
      const i = el('input');
      i.type = 'number'; i.min = '0'; i.max = '100'; i.value = val;
      i.onchange = () => { pending[key] = i.value; };
      l.append(el('span', null, label), i);
      return l;
    }
    grid.append(mkNum('damage_pct', 'Damage %', r.damage_pct),
                mkNum('stability_control', 'Stability %', r.stability_control));
    (r.toggles || []).forEach(t => {
      const l = el('label', 'f');
      const s = el('select');
      /* ⚠ Off/On only. RacingSetting has a third value, FREE (2), and it is
         the game's own name - but it belongs to the FIXED presets
         (Beginner/Rookie/Expert/Pro), where it means "this preset does not
         dictate this assist, use the driver's own". ACECM only ever writes
         the CUSTOM preset, which IS the driver's own setting, so offering
         Free here asked a question with no meaning. It is still shown if the
         file already holds it, rather than silently rewriting a value we did
         not put there. */
      const opts = [['off', 'Off'], ['on', 'On']];
      if (t.value === 'free') opts.push(['free', 'Free (from a preset)']);
      opts.forEach(([v, lab]) => {
        const o = el('option', null, lab);
        o.value = v;
        if (v === t.value) o.selected = true;
        s.append(o);
      });
      s.onchange = () => { pending[t.name] = s.value; };
      l.append(el('span', null, t.label), s);
      grid.append(l);
    });
    const save = el('button', 'sm primary', 'Save assists');
    save.onclick = async () => {
      if (!Object.keys(pending).length) { toast('Nothing changed'); return; }
      save.disabled = true;
      const res = await api('assists', pending);
      save.disabled = false;
      if (!res || !res.ok) { toast((res && res.error) || 'Could not save', true); return; }
      toast('Assists saved');
      loadAssists();
    };
    asBody.append(grid, save);
    /* ⚠ Single-player penalties belong HERE, not on the Servers page. They
       are a property of your own client - "affects only your own
       singleplayer" - so sitting beside the dedicated-server switch made them
       read as a server setting. The server half stays on Servers, where it
       does belong. Appended last: it is a separate concern from the assists
       above it, and it writes to content.kspkg rather than the profile. */
    loadSpPenalties();
    matchPaneHeights();
  }

  /* ⚠ Both panes claim the same height, so switching Session <-> Assists does
     not resize the card under them - that jump is what made the UI feel like
     it morphed. Measured rather than a magic number in the CSS: the panes are
     different heights on different windows, and a guessed value is wrong on
     every window except the one it was guessed on. */
  /* ⚠ The grid's height is measured, never assumed. What sits above it is
     not a constant: the header can carry an attention banner, and the mode
     bar wraps to a second row in a narrow window - so every hard-coded
     subtraction was right at one size and wrong at the next, which is what
     made the page overflow on first open and while resizing. */
  function fitDrive() {
    // ⚠ _wanted too: _page is only set AFTER the page finishes building, so
    // every fit during the build bailed out here and the grid ran on the
    // calc() fallback - the page was never actually fitted except on resize
    if ((_page !== 'drive' && _wanted !== 'drive') || !wrap.isConnected) return;
    const top = wrap.getBoundingClientRect().top;
    // the .page bottom padding is the only thing below the grid
    const pad = parseFloat(getComputedStyle(p).paddingBottom) || 0;
    const h = Math.max(340, Math.round(innerHeight - top - pad));
    document.documentElement.style.setProperty('--drive-h', h + 'px');
  }
  // ⚠ Re-fit whenever anything ABOVE the grid changes size, not just the
  // window: the activity strip and the attention banner come and go on
  // their own, and a stale fit left the page scrolling by their height.
  // One observer per build, replacing the last - the old
  // addEventListener('resize') here added another listener every rebuild.
  if (drivePage._ro) drivePage._ro.disconnect();
  if (typeof ResizeObserver === 'function') {
    const ro = new ResizeObserver(() => fitDrive());
    ['header', '#activity', '#attention'].forEach(q => { const n = $(q); if (n) ro.observe(n); });
    ro.observe(viaBar);
    drivePage._ro = ro;
  }
  if (!drivePage._onResize) {
    drivePage._onResize = true;
    addEventListener('resize', () => drivePage._fit && drivePage._fit());
  }
  drivePage._fit = fitDrive;
  requestAnimationFrame(fitDrive);

  function matchPaneHeights() {
    sessionPane.style.minHeight = '';
    assistPane.style.minHeight = '';
    const hidden = assistPane.style.display === 'none';
    if (hidden) { assistPane.style.visibility = 'hidden'; assistPane.style.display = ''; }
    const h = Math.max(sessionPane.offsetHeight, assistPane.offsetHeight);
    if (hidden) { assistPane.style.display = 'none'; assistPane.style.visibility = ''; }
    if (h > 0) {
      sessionPane.style.minHeight = h + 'px';
      assistPane.style.minHeight = h + 'px';
    }
  }
  loadAssists();

  function numInp(key, min, max) {
    const n = el('input');
    n.type = 'number';
    n.min = String(min);
    n.max = String(max);
    n.value = String(sel[key]);
    n.oninput = () => {
      const v = Number(n.value);
      if (!Number.isNaN(v)) sel[key] = v;
    };
    return n;
  }
  function paintExtras() {
    extras.innerHTML = '';
    const m = sel.game_mode;
    /* ⚠ The game's RACE TYPE toggle. A race is LAPS or TIME and the duration
       box has to follow it - we used to write LAPS unconditionally, so a
       timed race could not be set up here at all. */
    const raceBits = () => {
      const t = el('select');
      [['LAPS', 'Laps'], ['TIME', 'Time']].forEach(([v, lab]) => {
        const o = el('option', null, lab);
        o.value = v;
        if (v === sel.race_type) o.selected = true;
        t.append(o);
      });
      t.onchange = () => { sel.race_type = t.value; paintExtras(); };
      return [mkf('Race type', t),
              sel.race_type === 'TIME'
                ? mkf('Race (minutes)', numInp('race_minutes', 1, 600))
                : mkf('Race (laps)', numInp('race_laps', 1, 200))];
    };
    const span = (label, node) => {
      const l = mkf(label, node);
      l.classList.add('span2');
      return l;
    };
    if (m === 'PRACTICE' || m === 'HOTLAP' || m === 'HOTSTINT' || m === 'TEST_DRIVE') {
      extras.append(span('Duration (minutes)', numInp('duration_min', 1, 600)));
    }
    if (aiModes.includes(m)) {
      extras.append(
        // the game's slider is min=1 max=32
        mkf('AI cars', numInp('num_opponents', 1, 32)),
        mkf('Same car as you', (() => {
          const c = el('select');
          [['true', 'Yes'], ['false', 'No']].forEach(([v, lab]) => {
            const o = el('option', null, lab);
            o.value = v;
            if (String(sel.single_make) === v) o.selected = true;
            c.append(o);
          });
          c.onchange = () => { sel.single_make = c.value === 'true'; };
          return c;
        })()),
        // ⚠ the game's skill slider starts at 80, not 0 - anything lower has
        // no position on it
        mkf('AI skill min', numInp('skill_min', 80, 100)),
        mkf('AI skill max', numInp('skill_max', 80, 100)),
      );
      const agg = el('select');
      (d.aggressiveness || ['Safe', 'Normal', 'Competitive']).forEach(a => {
        const o = el('option', null, a);
        o.value = a;
        if (a === sel.aggressiveness) o.selected = true;
        agg.append(o);
      });
      agg.onchange = () => { sel.aggressiveness = agg.value; };
      extras.append(span('AI behaviour', agg));
    }
    if (m === 'RACE_WEEKEND') {
      extras.append(
        mkf('Practice (min)', numInp('practice_min', 1, 240)),
        mkf('Qualifying (min)', numInp('quali_min', 1, 120)),
        mkf('Warmup (min)', numInp('warmup_min', 0, 60)),
        ...raceBits(),
      );
    }
    if (m === 'INSTANT_RACE') {
      extras.append(
        ...raceBits(),
        mkf('Grid position (0=auto)', numInp('starting_position', 0, 40)),
      );
    }
  }

  /* The chosen car/track as a card that opens its picker.

     ⚠ `open` is re-bound on EVERY paint. paintHead runs again whenever the
     selection changes, so a handler attached once outside would be dropped
     along with the innerHTML it was attached to - the card would open the
     picker until you picked something, and then never again. */
  function paintHead(box, imgSrc, title, sub, open, row) {
    box.innerHTML = '';
    const has = !!imgSrc && !/[?&](id|folder)=(&|$)/.test(imgSrc);
    box.classList.toggle('row-pick', !!row);
    box.classList.toggle('empty-pick', !has && !row);
    if (has) {
      const img = el('img');
      img.alt = '';
      img.src = imgSrc;
      // a car with no render must not leave a broken-image glyph on the card
      img.onerror = () => {
        img.remove();
        if (!row) box.classList.add('empty-pick');
      };
      box.append(img);
    }
    const bar = el('div', 'dp-bar');
    const t = el('div', 'grow');
    t.innerHTML = `<b>${esc(title || 'Nothing selected')}</b>`
      + (sub ? `<div class="dp-sub">${esc(sub)}</div>` : '');
    bar.append(t);
    if (open) bar.append(el('span', 'dp-kebab', '⋮'));
    box.append(bar);
    box.onclick = open || null;
    box.title = open ? 'Click to choose' : '';
  }

  /* One picker, used by both columns. It does NOT rebuild the list: it moves
     the existing search box and list into a dialog and puts them back on
     close. That keeps every behaviour they already have - the variant
     grouping, the server's allowed-cars filter, the incremental repaint -
     instead of a second implementation that would drift from the first. */
  function openPicker(title, search, list, extra, wide) {
    const veil = el('div', 'pk-veil');
    const box = el('div', 'pk-box' + (wide ? ' pk-wide' : ''));
    const head = el('div', 'pk-head');
    const h = el('h3', null, title);
    const x = el('button', 'pk-x', '×');
    const body = el('div', 'pk-body');

    const home = list.parentNode;
    const mark = document.createComment('picker');
    home.insertBefore(mark, search);

    // they sit hidden in the column between openings; the dialog is the only
    // place they are meant to be seen
    const wasSearch = search.style.display, wasList = list.style.display;
    const wasExtra = extra ? extra.style.display : null;
    search.style.display = '';
    list.style.display = '';
    if (extra) extra.style.display = '';
    head.append(h, search, x);
    if (extra) body.append(extra);
    body.append(list);
    box.append(head, body);
    veil.append(box);
    document.body.append(veil);
    search.focus();

    const close = () => {
      // ⚠ put them back where they were, in order, or the next open finds
      // them detached and the column is left empty.
      // ⚠ The placeholder can itself be DETACHED by the time we close: the
      // Drive poll repaints while the picker is open, and Online mode moves
      // the server box between columns, either of which takes the comment
      // node with it. Reading .parentNode blind threw here, which aborted
      // close() and left the search box and list stranded inside a dialog
      // that was already gone - the column then came back empty. Fall back
      // to the container we borrowed them from.
      const back = mark.parentNode || home;
      const at = mark.parentNode ? mark : null;
      back.insertBefore(search, at);
      if (extra) back.insertBefore(extra, at);
      back.insertBefore(list, at);
      mark.remove();
      search.style.display = wasSearch;
      list.style.display = wasList;
      // ⚠ Not the value captured when the picker OPENED - the mode may have
      // changed while it was open, and restoring the old one is how the
      // server filters ended up visible in Single player.
      if (extra === srvFilters) {
        srvFilters.style.display = sel.via === 'server' ? '' : 'none';
      } else if (extra) {
        extra.style.display = wasExtra;
      }
      veil.remove();
      document.removeEventListener('keydown', onKey);
    };
    const onKey = e => { if (e.key === 'Escape') close(); };
    document.addEventListener('keydown', onKey);
    x.onclick = close;
    veil.onclick = e => { if (e.target === veil) close(); };
    /* Picking something is the point of the dialog - shut it afterwards.

       ⚠ NOT keyed on row.dataset.id. Only the car rows set it; track and
       server rows never did, so tracks selected but the dialog stayed open
       and had to be dismissed by hand. The one row that must NOT close is a
       car's variant expander, and that is the only row with a caret in it -
       so ask about the caret rather than about an id that most rows lack.
       Buttons inside a row (List in browser) stopPropagation, so they never
       reach this. */
    list.addEventListener('click', e => {
      const row = e.target.closest && e.target.closest('.drive-row');
      if (row && !row.querySelector('.drive-caret')) setTimeout(close, 0);
    });
    return close;
  }

  function paintCars() {
    const q = (carSearch.value || '').toLowerCase();
    driveFilter.car = carSearch.value;
    const allow = allowedCars();
    const allowKey = allow ? [...allow].sort().join('|') : '*';
    if (carList.dataset.built !== '1' || carList.dataset.allow !== allowKey) {
      carList.innerHTML = '';
      carList.dataset.built = '1';
      carList.dataset.allow = allowKey;
      const pool = (d.cars || []).filter(c => !allow || carAllowed(c, allow));
      if (!pool.length) {
        carList.append(el('div', 'empty',
          allow ? 'No cars allowed on this server' : 'No cars'));
      } else {
        /* ⚠ One row per CAR, not per preset. The AE86 ships four presets
           and the Gr86 three, so a flat list repeated the same car name over
           and over and buried the rest - 116 rows for 85 actual cars. A car
           with variants expands to show them; a car with one is just a row. */
        const groups = new Map();
        pool.forEach(c => {
          const key = c.model || c.id;
          if (!groups.has(key)) groups.set(key, []);
          groups.get(key).push(c);
        });

        const mkRow = (c, variant) => {
          const r = el('div', 'drive-row' + (variant ? ' drive-variant' : '')
                              + (c.id === sel.car ? ' on' : ''));
          r.dataset.id = c.id;
          r.dataset.blob = `${c.label} ${c.id} ${c.brand || ''}`.toLowerCase();
          if (!variant) {
            const img = el('img');
            img.loading = 'lazy';
            img.alt = '';
            // ⚠ preset too: trims share a model folder, so id alone showed
            // the same picture for every trim and never your livery.
            img.src = 'api/thumb/car?id=' + encodeURIComponent(c.model || c.id)
                    + '&preset=' + encodeURIComponent(c.id);
            img.onerror = () => { img.style.visibility = 'hidden'; };
            r.append(img);
          }
          const t = el('div', 'grow');
          /* ⚠ Head rows name the CAR, variant rows name only the VARIANT.
             The base cars ship up to four mechanical presets and every one of
             them used to render as the same full label, so the list showed
             "BMW M2 Coupe" twice with nothing but "_mech_1" / "_mech_2" to
             tell them apart. The game names its own variants (Performance,
             Weissach, Kouki, 450); base_label / variant come from it. */
          const shown = variant ? (c.variant || c.label)
                                : (c.base_label || c.label);
          t.innerHTML = `<div class="name">${esc(shown)}</div>`
            + `<div class="tiny dim">${esc(c.id)}</div>`;
          r.append(t);
          if (c.mod) r.append(el('span', 'pill warn', 'mod'));
          r.onclick = () => { sel.car = c.id; paintCars(); paintSelected(); };
          return r;
        };

        groups.forEach(list => {
          const first = list[0];
          if (list.length === 1) {
            carList.append(mkRow(first, false));
            return;
          }
          const head = mkRow(first, false);
          // the head row selects nothing on its own - it opens the variants
          head.dataset.id = '';
          head.onclick = null;
          const count = el('span', 'drive-count', String(list.length));
          const caret = el('span', 'drive-caret', '▸');
          head.append(count, caret);
          const kids = el('div', 'drive-kids');
          list.forEach(c => kids.append(mkRow(c, true)));
          // searching must still reach a variant, so match on any of them
          head.dataset.blob = list
            .map(c => `${c.label} ${c.id} ${c.brand || ''}`).join(' ')
            .toLowerCase();
          head.onclick = () => {
            const open = kids.classList.toggle('on');
            caret.textContent = open ? '▾' : '▸';
          };
          if (list.some(c => c.id === sel.car)) kids.classList.add('on');
          carList.append(head, kids);
        });
      }
    }
    let vis = 0;
    carList.querySelectorAll('.drive-row').forEach(r => {
      const show = !q || (r.dataset.blob || '').includes(q);
      r.style.display = show ? '' : 'none';
      r.classList.toggle('on', r.dataset.id === sel.car);
      if (show) vis++;
    });
    let empty = carList.querySelector('.empty');
    if (!vis && !empty) {
      empty = el('div', 'empty', q ? 'No matching cars' : 'No cars');
      carList.append(empty);
    } else if (empty && vis) {
      empty.remove();
    } else if (empty && !vis && q) {
      empty.textContent = 'No matching cars';
    }
  }

  function ownTrack(s) {
    const map = (driveLocal && driveLocal.track_map) || {};
    const name = String(s.track || '').trim();
    return !name || !!map[name];
  }
  function ownCarOn(s) {
    const ids = new Set((driveLocal && driveLocal.car_ids) || []);
    const cars = s.cars || [];
    if (!cars.length) return true;
    return cars.some(id => {
      if (ids.has(id)) return true;
      const c = carOf(id);
      return !!(c && ((c.model && ids.has(c.model)) || ids.has(c.id)));
    });
  }
  function isAcecm(s) {
    const ip = s.server_ip || '';
    if (brTags[ip] && brTags[ip].hosted) return true;
    return /\[ACECM\]/i.test(String(s.name || ''));
  }
  /* A server the EvoForge directory told us about. Unlike isAcecm this is
     not guesswork from the name: drive sets source='evoforge' on the row when
     the directory matched it by address, and only those rows carry share_url
     - the operator's own http share for the modded track they run. */
  function isEvoforge(s) {
    return String(s.source || '') === 'evoforge';
  }
  function tagDriveIps(rows) {
    const ips = [...new Set(rows.map(s => s.server_ip).filter(Boolean))];
    const need = ips.filter(ip => !brTags[ip]);
    if (!need.length) return;
    need.slice(0, 72).forEach(ip => { brTags[ip] = { pending: true }; });
    (async () => {
      const batch = need.slice(0, 72);
      for (let i = 0; i < batch.length; i += 24) {
        const chunk = batch.slice(i, i + 24);
        try {
          const qs = chunk.map(ip => 'ip=' + encodeURIComponent(ip)).join('&');
          const r = await fetch('/api/browser/tag?' + qs).then(x => x.json());
          Object.assign(brTags, (r && r.hosts) || {});
        } catch (e) {}
        if (sel.via === 'server') paintServers();
        if (sel.via === 'local') paintLocal();
      }
    })();
  }

  function paintSrvFilters() {
    // shown with the server list, which is inline - only the car and the
    // single-player track use the card-and-dialog form
    srvFilters.style.display = sel.via === 'server' ? '' : 'none';
    if (sel.via !== 'server' || srvFilters.dataset.ready) return;
    srvFilters.dataset.ready = '1';
    const sort = el('select');
    [['players', 'Most players'], ['ping', 'Lowest ping'],
     ['name', 'Name'], ['track', 'Track']].forEach(([v, lab]) => {
      const o = el('option', null, lab);
      o.value = v;
      if (v === driveFilter.sort) o.selected = true;
      sort.append(o);
    });
    sort.onchange = () => { driveFilter.sort = sort.value; paintServers(); };
    const chk = (key, lab) => {
      const l = el('label', 'ctl');
      const c = el('input');
      c.type = 'checkbox';
      c.checked = !!driveFilter[key];
      c.onchange = () => { driveFilter[key] = c.checked; paintServers(); };
      l.append(c, el('span', null, lab));
      return l;
    };
    srvFilters.append(sort,
      chk('hideFull', 'Hide full'),
      chk('hideLocked', 'Hide password'),
      chk('hasPlayers', 'Has players'),
      chk('haveTrack', 'Track I have'),
      chk('haveCar', 'Car I have'),
      chk('acecmOnly', 'ACECM only'),
      chk('evoforgeOnly', 'EvoForge only'));
    if (!driveLocal) {
      api('browser/local').then(l => { driveLocal = l || {}; paintServers(); });
    }
    // The public list is fetched separately so the rest of Drive can draw
    // straight away - same late-load shape as browser/local above. Guarded
    // so the filter checkboxes repainting cannot start a second fetch.
    if (d.servers_pending && !d._srvFetch) {
      d._srvFetch = true;
      api('drive/servers').then(r => {
        if (!r || r.error) {
          d.servers_pending = false;
          d.servers_meta = { error: (r && r.error) || 'could not load list' };
        } else {
          expandPool(r);
          d.servers = r.servers || [];
          d.servers_meta = r.servers_meta || {};
          d.servers_pending = false;
        }
        // the user may have clicked away, or switched to a different source
        if (_page !== 'drive') return;
        defaultServer();
        paintServers();
        if (sel.via === 'server') paintVia();
      });
    }
  }

  /* A server row carries the same track photo the Single player track list
     uses - the track is how you recognise a server at a glance, and a wall of
     text rows was the thing that made this list hard to read. */
  function trackThumb(track) {
    const img = el('img');
    img.loading = 'lazy';
    img.alt = '';
    img.src = 'api/thumb/track?folder=' + encodeURIComponent(track || '');
    img.onerror = () => { img.style.visibility = 'hidden'; };
    return img;
  }

  function paintServers() {
    paintSrvFilters();
    delete trkList.dataset.built;
    trkCol.querySelector('h2').textContent = 'Public servers';
    trkSearch.placeholder = 'Filter name, track, car…';
    const q = (trkSearch.value || '').toLowerCase();
    driveFilter.track = trkSearch.value;
    trkList.innerHTML = '';
    const meta = d.servers_meta || {};
    if (!(d.servers || []).length) {
      // ⚠ "still coming" and "there are none" are different things to say.
      // Showing the empty-state while the fetch is in flight read as though
      // the list had failed, a moment before it appeared.
      trkList.append(el('div', 'empty',
        d.servers_pending ? 'Loading public servers…'
        : (meta.hint || meta.error
           || 'No public list yet. Use Refresh list, or open Multiplayer '
             + 'in-game once.')));
      return;
    }
    const num = v => (typeof v === 'number' ? v : 0);
    let rows = (d.servers || []).filter(s => {
      if (driveFilter.hideFull && num(s.players) >= num(s.max_players)
          && num(s.max_players) > 0) return false;
      if (driveFilter.hideLocked && s.locked) return false;
      if (driveFilter.hasPlayers && num(s.players) < 1) return false;
      if (driveFilter.haveTrack && !ownTrack(s)) return false;
      if (driveFilter.haveCar && !ownCarOn(s)) return false;
      if (driveFilter.acecmOnly && !isAcecm(s)) return false;
      if (driveFilter.evoforgeOnly && !isEvoforge(s)) return false;
      if (!q) return true;
      const blob = [s.name, s.track, s.layout, s.game_mode, s.server_ip]
        .concat(s.cars || []).join(' ').toLowerCase();
      return blob.includes(q);
    });
    rows.sort((a, b) => {
      if (driveFilter.sort === 'ping')
        return (num(a.ping) || 9999) - (num(b.ping) || 9999);
      if (driveFilter.sort === 'track')
        return String(a.track || '').localeCompare(String(b.track || ''));
      if (driveFilter.sort === 'name')
        return String(a.name || '').localeCompare(String(b.name || ''));
      return num(b.players) - num(a.players);
    });
    if (!rows.length) {
      trkList.append(el('div', 'empty',
        'No matches — loosen the filters above'));
      return;
    }
    const shown = rows.slice(0, 250);
    if (rows.length > shown.length) {
      trkList.append(el('div', 'tiny dim',
        `Showing ${shown.length} of ${rows.length}`));
    }
    shown.forEach(s => {
      const on = (sel.server_id && s.id === sel.server_id)
        || (sel.server_ip && s.server_ip === sel.server_ip
            && Number(s.server_tcp_port) === Number(sel.server_tcp_port));
      const r = el('div', 'drive-row' + (on ? ' on' : ''));
      r.dataset.ip = s.server_ip || '';
      const t = el('div', 'grow');
      const name = el('div', 'name');
      name.append(document.createTextNode(
        (s.name || '(unnamed)') + (s.locked ? ' 🔒' : '')));
      if (isAcecm(s)) {
        name.append(el('span', 'pill acecm', '<i class="dot"></i>ACECM'));
      }
      if (isEvoforge(s)) {
        const pill = el('span', 'pill evo', 'EvoForge');
        pill.title = s.share_url
          ? ('shares its content at ' + s.share_url)
          : 'listed in the EvoForge directory';
        name.append(pill);
      }
      const sub = el('div', 'tiny dim',
        `${esc(s.track || '—')} · ${esc(s.layout || '')}`
        + ` · ${s.players || 0}/${s.max_players || 0}`
        + ` · ${esc(carsLine(s))}`
        + (s.ping ? ` · ${s.ping}ms` : ''));
      t.append(name, sub);
      const get = el('button', 'sm', 'Get content');
      get.title = s.share_url
        ? ('Download this track from ' + s.share_url)
        : 'Ask this host\'s ACECM for anything you are missing';
      get.onclick = ev => {
        ev.stopPropagation();
        contentFrom({
          server_ip: s.server_ip,
          server_tcp_port: s.server_tcp_port,
          track: s.track || '',
          share_url: s.share_url || '',
          name: s.name || '',
        });
      };
      r.append(trackThumb(s.track), t, get);
      r.onclick = () => {
        sel.server_id = s.id;
        sel.server_ip = s.server_ip;
        sel.server_tcp_port = s.server_tcp_port;
        sel.server_udp_port = s.server_udp_port;
        const allow = allowedCars();
        if (allow && sel.car && !allow.has(sel.car)) sel.car = '';
        paintServers();
        paintCars();
        paintSelected();
        paintVia();
  fitDrive();
      };
      trkList.append(r);
    });
    tagDriveIps(shown);
  }

  function paintLocal() {
    srvFilters.style.display = 'none';
    delete trkList.dataset.built;
    trkCol.querySelector('h2').textContent = 'My servers';
    trkSearch.placeholder = 'Filter your servers…';
    const q = (trkSearch.value || '').toLowerCase();
    trkList.innerHTML = '';
    const be = d.backend || {};
    const rows = (d.local_servers || []).filter(s => {
      if (!q) return true;
      const blob = [s.name, s.track, s.layout, s.game_mode]
        .concat(s.cars || []).join(' ').toLowerCase();
      return blob.includes(q);
    });
    if (!rows.length) {
      trkList.append(el('div', 'empty',
        'No ACECM server profiles yet. Open Servers and create one, then come back.'));
      return;
    }
    if (!sel.local_id && rows[0]) sel.local_id = (rows.find(s => s.running) || rows[0]).id;
    const note = el('div', 'tiny dim');
    note.style.padding = '6px 4px 10px';
    note.innerHTML = 'Join starts the host if it is stopped, writes the lobby '
      + 'row (track, cars, name), and opens Multiplayer so it shows as '
      + '<b>[ACECM]</b>. '
      + (be.listening
        ? 'Lobby proxy is on.'
        : 'Lobby proxy starts with Join.');
    trkList.append(note);
    rows.forEach(s => {
      const on = s.id === sel.local_id;
      const r = el('div', 'drive-row' + (on ? ' on' : ''));
      const t = el('div', 'grow');
      const name = el('div', 'name');
      name.append(document.createTextNode(s.name || '(unnamed)'));
      name.append(el('span', 'pill acecm', '<i class="dot"></i>ACECM'));
      if (s.running) name.append(el('span', 'pill on', 'running'));
      if (s.no_lobby) name.append(el('span', 'pill warn', 'private'));
      const sub = el('div', 'tiny dim',
        `${esc(s.track || '—')} · ${esc(s.layout || '')}`
        + ` · :${s.tcp_port}`
        + ` · ${s.clients != null ? s.clients + '/' + (s.max_players || 0) : '—'}`
        + ` · ${esc(carsLine(s))}`);
      t.append(name, sub);
      const listB = el('button', 'sm' + (s.listed ? '' : ' primary'),
                       s.listed ? 'Listed' : 'List in browser');
      listB.title = 'Write the lobby advertisement and start the proxy so this '
        + 'row appears in Multiplayer';
      listB.onclick = async ev => {
        ev.stopPropagation();
        const r = await api('drive/list', { id: s.id });
        toast(r.ok
          ? ((r.ad && r.ad.track)
            ? `Listed "${r.ad.name}" · ${r.ad.track}`
            : 'Listed in the lobby')
          : (r.error || 'Could not list'), !r.ok);
        if (r.ok) drivePage();
      };
      r.append(trackThumb(s.track), t, listB);
      r.onclick = () => {
        sel.local_id = s.id;
        const allow = allowedCars();
        if (allow && sel.car && !carAllowed(carOf(sel.car) || {id: sel.car}, allow))
          sel.car = '';
        paintLocal();
        paintCars();
        paintSelected();
        paintVia();
      };
      trkList.append(r);
    });
  }

  function paintTracks() {
    if (sel.via === 'server') { paintServers(); return; }
    if (sel.via === 'local') { paintLocal(); return; }
    srvFilters.style.display = 'none';
    trkCol.querySelector('h2').textContent = 'Track';
    trkSearch.placeholder = 'Filter tracks…';
    const q = (trkSearch.value || '').toLowerCase();
    driveFilter.track = trkSearch.value;
    if (trkList.dataset.built !== 'tracks') {
      trkList.innerHTML = '';
      trkList.dataset.built = 'tracks';
      const pool = d.tracks || [];
      if (!pool.length) {
        trkList.append(el('div', 'empty', d.tracks_error || 'No tracks'));
      } else {
        pool.forEach(t => {
          const r = el('div', 'drive-row');
          r.dataset.index = String(t.index);
          r.dataset.custom = t.custom_track || '';
          r.dataset.blob = `${t.label} ${t.track} ${t.layout} ${t.name}`.toLowerCase();
          const img = el('img');
          img.loading = 'lazy';
          img.alt = '';
          img.src = 'api/thumb/track?folder=' + encodeURIComponent(t.track || '');
          img.onerror = () => { img.style.visibility = 'hidden'; };
          const cap = el('div', 'grow');
          cap.innerHTML = `<div class="name">${esc(t.label || t.name)}</div>`
            + `<div class="tiny dim">#${t.index} · ${esc(t.layout || '')}</div>`;
          r.append(img, cap);
          if (t.mod) r.append(el('span', 'pill warn', 'mod'));
          r.onclick = () => {
            sel.track_index = t.index;
            sel.custom_track = t.custom_track || '';
            paintTracks();
            paintSelected();
          };
          trkList.append(r);
        });
      }
    }
    let vis = 0;
    trkList.querySelectorAll('.drive-row').forEach(r => {
      const show = !q || (r.dataset.blob || '').includes(q);
      r.style.display = show ? '' : 'none';
      const on = (sel.custom_track && r.dataset.custom === sel.custom_track)
        || (!sel.custom_track && Number(r.dataset.index) === sel.track_index);
      r.classList.toggle('on', on);
      if (show) vis++;
    });
    let empty = trkList.querySelector('.empty');
    if (!vis && !empty) {
      empty = el('div', 'empty', q ? 'No matching tracks' : (d.tracks_error || 'No tracks'));
      trkList.append(empty);
    } else if (empty && vis) {
      empty.remove();
    } else if (empty && !vis && q) {
      empty.textContent = 'No matching tracks';
    }
  }

  function paintSelected() {
    const c = carOf(sel.car);
    // ⚠ here, not on a row's onclick: the car can change through several
    // paths (row click, server filter clearing it, a restored pick) and only
    // this runs for all of them - hooking one click handler meant the picker
    // never appeared at all.
    if (paintSelected._lastCar !== sel.car) {
      paintSelected._lastCar = sel.car;
      drawLivery(sel.car);
      drawSpecs(sel.car);
    }
    /* ⚠ make=1 and a bust token, for this ONE card only. A livery change
       asks for a picture that has never been rendered, so without make=1 the
       server falls back to the plain model render and you keep looking at the
       old colour under the words "saved". The token is what gets past the
       browser's own cache, since the URL is otherwise identical. */
    paintHead(carHead,
      'api/thumb/car?make=1&id=' + encodeURIComponent((c && (c.model || c.id)) || '')
        + '&preset=' + encodeURIComponent((c && c.id) || '')
        + (thumbBust ? '&t=' + thumbBust : ''),
      c ? c.label : 'Pick a car',
      c ? c.id : '',
      () => { paintCars(); openPicker('Choose a car', carSearch, carList); });
    if (sel.via === 'server') {
      const s = serverOf();
      paintHead(trkHead,
        'api/thumb/track?folder=' + encodeURIComponent((s && s.track) || ''),
        s ? s.name : 'Pick a public server',
        // ⚠ A direct address we know nothing about must not borrow the
        // shape of a real entry - "0/0 · all cars" reads as an empty server
        // that allows everything, when the truth is we have not been told.
        s ? ((s.direct && !s.track && !s.max_players)
             ? `${s.server_ip}:${s.server_tcp_port} · details unknown`
             : `${s.track || ''} · ${s.players || 0}/${s.max_players || 0}`
               + ` · ${carsLine(s)}`
               + (s.locked ? ' · password' : '')) : '',
        () => {
          paintServers();
          openPicker('Public servers', trkSearch, trkList, srvFilters, true);
        });
      showTrackList(true);
      return;
    }
    if (sel.via === 'local') {
      const s = localOf();
      paintHead(trkHead,
        'api/thumb/track?folder=' + encodeURIComponent((s && s.track) || ''),
        s ? s.name : 'Pick your server',
        s ? `${s.track || ''} · :${s.tcp_port}`
          + ` · ${carsLine(s)}`
          + (s.running ? ' · running' : ' · stopped')
          + (s.no_lobby ? ' · private' : '') : '',
        () => {
          paintLocal();
          openPicker('My servers', trkSearch, trkList);
        });
      showTrackList(true);
      return;
    }
    const t = trackOf(sel.track_index);
    paintHead(trkHead,
      'api/thumb/track?folder=' + encodeURIComponent((t && t.track) || ''),
      t ? (t.label || t.name) : 'Pick a track',
      t ? `#${t.index} · ${t.layout || ''}` : '',
      () => { paintTracks(); openPicker('Choose a track', trkSearch, trkList); });
    showTrackList(false);
  }

  /* Servers keep the old inline list; a track uses the card + dialog.

     ⚠ Both live in the same column and the same three nodes, so whichever
     mode paints last decides what is on screen - leaving this to the picker
     alone meant switching from Public servers back to Single player left the
     server list sitting under the track card. */
  function showTrackList(inline) {
    srvBox.classList.toggle('has-list', !!inline);
    /* ⚠ The public-server filters are re-asserted HERE, on every repaint,
       not just where they are first hidden. Their visibility was set at four
       scattered points (paintTracks, paintLocal, paintSrvFilters, and
       openPicker restoring whatever it captured on open), so any path that
       repainted in a different order could leave "Most players / Hide full /
       ACECM only" sitting under the TRACK card in Single player. This runs
       for every mode on every repaint, so the rule is stated once. */
    srvFilters.style.display = sel.via === 'server' ? '' : 'none';
    trkSearch.style.display = inline ? '' : 'none';
    trkList.style.display = inline ? '' : 'none';
    trkSearch.placeholder = sel.via === 'server'
      ? 'Filter name, track, car…'
      : (sel.via === 'local' ? 'Filter your servers…' : 'Filter tracks…');
  }

  function paintVia() {
    paintDirect();
    const on = sel.via === 'server' || sel.via === 'local';
    spFields.forEach(n => { n.style.display = on ? 'none' : ''; });
    extras.style.display = on ? 'none' : '';
    /* The preview card never moves - it is the left column's second tile in
       every mode. Only the list travels: online it sits above the Join
       button, in Single player it goes back under its own preview so the
       picker dialog can borrow it. */
    if (trkCol.parentNode !== leftCol) leftCol.append(trkCol);
    if (on) {
      if (srvBox.parentNode !== sessionPane) sessionPane.prepend(srvBox);
    } else if (srvBox.parentNode !== trkCol) {
      trkCol.append(srvBox);
    }
    if (typeof matchPaneHeights === 'function') matchPaneHeights();
    if (sel.via === 'server') {
      const s = serverOf();
      const meta = d.servers_meta || {};
      hint.textContent = s
        ? `Sets your car to one this server allows, then joins `
          + `${s.server_ip}:${s.server_tcp_port}.`
        : (meta.hint || 'Pick a public server. The list is captured when you '
          + 'open Multiplayer in-game.');
      driveBtn.textContent = 'Join';
      pwField.style.display = (s && s.locked) ? '' : 'none';
    } else if (sel.via === 'local') {
      const s = localOf();
      hint.textContent = s
        ? ((s.running ? 'Joins ' : 'Starts then joins ')
          + `127.0.0.1:${s.tcp_port} as [ACECM] ${s.name}. `
          + 'The lobby row uses this profile\'s track and cars.')
        : 'Create a server on the Servers tab, then pick it here.';
      driveBtn.textContent = s && !s.running ? 'Start & Join' : 'Join';
      pwField.style.display = (s && s.locked) ? '' : 'none';
    } else {
      pwField.style.display = 'none';
      hint.textContent = 'Writes the session, launches the game, and opens '
        + 'the pit menu so you can change setup. Close the game first so '
        + 'the save sticks.';
      driveBtn.textContent = 'Drive';
      paintExtras();
    }
    paintTracks();
    paintCars();
    paintSelected();
  }

  let carSearchT = 0, trkSearchT = 0;
  carSearch.oninput = () => {
    clearTimeout(carSearchT);
    carSearchT = setTimeout(paintCars, 80);
  };
  trkSearch.oninput = () => {
    clearTimeout(trkSearchT);
    trkSearchT = setTimeout(paintTracks, 80);
  };
  mode.onchange = () => { sel.game_mode = mode.value; paintExtras(); };
  weather.onchange = () => { sel.weather = weather.value; };
  grip.onchange = () => { sel.grip = grip.value; };
  hour.onchange = () => { sel.tod_hour = Number(hour.value); };
  paintVia();

  function idleLabel() {
    if (sel.via === 'local')
      return (localOf() && !localOf().running) ? 'Start & Join' : 'Join';
    return sel.via === 'server' ? 'Join' : 'Drive';
  }
  async function stopJob() {
    const x = await api('drive/cancel', {});
    if (!x.ok) toast(x.error || 'Could not stop', true);
    liveKick();
  }
  /* Everything the job reports arrives through the live feed now, so there
     is no Drive timer to arm, disarm or lose (see the live feed notes). */
  function onDrive(r) {
    if (!r) return;
    const phase = r.phase || 'idle';
    const busy = jobBusy(r);
    driveBtn.disabled = busy;
    if (pullBtn) pullBtn.disabled = busy;
    driveBtn.textContent = busy ? (JOB_STEP[phase] || r.hint || phase) + '…'
                                : idleLabel();
    paintJobSteps(stepsBox, r, {
      onStop: stopJob,
      onRetry: async () => {
        if (r.kind === 'capture') {
          const x = await api('drive/capture', {});
          if (!x.ok) toast(x.error || 'Could not start', true);
          liveKick();
        } else driveBtn.onclick();
      },
    });
    // the one-line status is only for "nothing running": the checklist
    // carries everything about a job
    if (busy || !stepsBox.hidden) st.textContent = '';
    else if (r.game_running) st.textContent = sel.via === 'server' || sel.via === 'local'
      ? 'Game is running — Join will set the car and push into the server.'
      : 'Game is running. Close it, then Drive.';
    else st.textContent = sel.via === 'local'
      ? 'Pick your ACECM server and an allowed car, then Join.'
      : (sel.via === 'server'
        ? 'Pick a public server and an allowed car, then Join.'
        : 'Pick a car and track, then Drive.');
    // a finished list capture brings new servers: rebuild once for it
    if (r.kind === 'capture' && phase === 'launched' && (r.captured || 0) > 0
        && drivePage._reloadedFor !== r.started) {
      drivePage._reloadedFor = r.started;
      setTimeout(() => { if (_page === 'drive') drivePage(); }, 500);
    }
  }

  driveBtn.onclick = async () => {
    if (!sel.car) { toast('Pick a car first', true); return; }
    if (sel.via === 'server' && !sel.server_ip && !sel.server_id) {
      toast('Pick a public server first', true); return;
    }
    if (sel.via === 'local' && !sel.local_id) {
      toast('Pick one of your ACECM servers first', true); return;
    }
    driveBtn.disabled = true;
    driveBtn.textContent = 'Starting…';
    const r = await api('drive', {
      via: sel.via,
      local_id: sel.local_id,
      server_id: sel.server_id,
      server_ip: sel.server_ip,
      server_tcp_port: sel.server_tcp_port,
      server_udp_port: sel.server_udp_port,
      password: sel.password,
      car: sel.car,
      track_index: sel.track_index,
      custom_track: sel.custom_track || '',
      game_mode: sel.game_mode,
      weather: sel.weather,
      grip: sel.grip,
      tod_hour: sel.tod_hour,
      num_opponents: sel.num_opponents,
      skill_min: sel.skill_min,
      skill_max: sel.skill_max,
      aggressiveness: sel.aggressiveness,
      single_make: sel.single_make,
      duration_min: sel.duration_min,
      practice_min: sel.practice_min,
      quali_min: sel.quali_min,
      warmup_min: sel.warmup_min,
      race_laps: sel.race_laps,
      race_type: sel.race_type,
      race_minutes: sel.race_minutes,
      time_mult: sel.time_mult,
      starting_position: sel.starting_position,
    });
    if (!r.ok) {
      driveBtn.disabled = false;
      driveBtn.textContent = sel.via === 'server' ? 'Join' : 'Drive';
      st.innerHTML = `<b style="color:var(--red)">${esc(r.error || 'failed')}</b>`;
      return;
    }
    liveKick();
  };
  // a capture that finished BEFORE this page was built must not trigger
  // the rebuild-once below on first paint
  if (live.snap && live.snap.drive && drivePage._reloadedFor === undefined)
    drivePage._reloadedFor = live.snap.drive.started;
  livePage('drive', s => onDrive(s.drive));
}
