/* Assetto Corsa EVO Content Manager - UI */
const $ = (s, r) => (r || document).querySelector(s);
const el = (t, c, h) => { const e = document.createElement(t);
  if (c) e.className = c; if (h != null) e.innerHTML = h; return e; };
const esc = s => String(s ?? '').replace(/[&<>"]/g,
  c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

async function api(path, body) {
  const opt = body ? { method: 'POST', body: JSON.stringify(body) } : {};
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), 12000);
  try {
    const r = await fetch('/api/' + path, { ...opt, signal: ctrl.signal });
    const j = await r.json().catch(() => ({ error: 'bad response' }));
    if (j && j.error) toast(j.error, true);
    return j;
  } catch (e) {
    const msg = (e && e.name === 'AbortError')
      ? (path + ' timed out') : String(e && e.message || e);
    return { error: msg };
  } finally {
    clearTimeout(t);
  }
}
let toastT;
function toast(msg, bad) {
  const t = $('#toast');
  if (!t) return;
  t.textContent = msg;
  t.style.borderLeftColor = bad ? 'var(--red)' : 'var(--accent)';
  t.classList.add('show');
  clearTimeout(toastT);
  toastT = setTimeout(() => t.classList.remove('show'), 3200);
}

/* --------------------------------------------------------------- drive -- */
/* Content Manager's home: pick car + track + mode, then Drive walks the
   client into a dedicated session. EVO has no -car/-track launch flags. */
let driveTimer = null;
let driveReloadT = null;
let driveFilter = {
  car: '', track: '',
  sort: 'players',
  hideFull: true,
  hideLocked: false,
  hasPlayers: false,
  haveTrack: false,
  haveCar: false,
  acecmOnly: false,
};
let driveLocal = null;

function stopDrivePoll() {
  if (driveTimer) { clearInterval(driveTimer); driveTimer = null; }
  if (driveReloadT) { clearTimeout(driveReloadT); driveReloadT = null; }
}

async function drivePage() {
  stopDrivePoll();
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
  wrap.append(carCol, trkCol, goCol);
  p.append(viaBar, wrap);

  const sel = {
    via: pick.via === 'server' ? 'server' : 'sp',
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
    starting_position: pick.starting_position ?? 0,
  };
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
    return (d.servers || []).find(s =>
      (sel.server_id && s.id === sel.server_id)
      || (sel.server_ip && s.server_ip === sel.server_ip
          && Number(s.server_tcp_port) === Number(sel.server_tcp_port))
    ) || null;
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

  [['sp', 'Single player'], ['local', 'My server'], ['server', 'Public servers']].forEach(([v, lab]) => {
    const b = el('button', 'sm' + (sel.via === v ? ' primary' : ''), lab);
    b.dataset.via = v;
    b.onclick = () => {
      sel.via = v;
      viaBar.querySelectorAll('button[data-via]').forEach(x => {
        x.classList.toggle('primary', x.dataset.via === v);
      });
      paintVia();
    };
    viaBar.append(b);
  });
  const manage = el('button', 'sm', 'Full browser');
  manage.onclick = () => go('browser');
  const pull = el('button', 'sm', 'Refresh list');
  pull.title = 'Launch the game, open Multiplayer, save the public list, then quit';
  pull.onclick = async () => {
    const r = await api('drive/capture', {});
    if (!r.ok) { toast(r.error || 'Could not start', true); return; }
    stopDrivePoll();
    driveTimer = setInterval(poll, 1200);
    poll();
  };
  viaBar.append(manage, pull);

  carCol.innerHTML = '<h2>Car</h2>';
  const carHead = el('div', 'drive-pick');
  const carSearch = el('input');
  carSearch.placeholder = 'Filter cars…';
  carSearch.value = driveFilter.car;
  const carList = el('div', 'list drive-list');
  const liveryBox = el('div');
  liveryBox.style.marginTop = '10px';
  // ⚠ Directly under the SELECTED car, not after the list. The list is a
  // tall scrolling box of ~100 rows, so anything appended after it sits
  // below the fold and is invisible in normal use - which is exactly how
  // it was reported: "where do i pick the livery?"
  carCol.append(carHead, liveryBox, carSearch, carList);

  /* ---- livery -----------------------------------------------------------
     Colours come from the CAR's own design, never from the brand's folder:
     the brand list is a superset, and writing a colour the car does not offer
     crashes the game on load. Everything here is wrapped, because this column
     is the main screen and a fault in a nice-to-have must not take it down. */
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
        const why = el('div', 'livery-head livery-none');
        why.textContent = 'no livery — you do not own this car';
        why.title = 'Liveries are stored per owned car. Buy or unlock it '
          + 'in-game and its colours appear here.';
        liveryBox.append(why);
        return;
      }
      const info = await api('livery?model=' + encodeURIComponent(model));
      const list = ((info && info.allowed) || {})['EXT SKIN'] || [];
      if (!list.length) return;

      /* ⚠ Collapsed by default. Thirteen colours listed under every car
         pushes the track and session columns off the screen, and the colour
         is not what you came to this page to choose - so show only which
         livery is on, and open the list when asked. */
      const short = p => (p.split('\\').pop() || p)
        .replace(/\.oemmultilayercolor$/, '')
        .replace(/^[a-z0-9]+_paint_/, '').replace(/_/g, ' ');
      const cur = owned.slots['EXT SKIN'] || '';

      const head = el('div', 'livery-head');
      const caret = el('span', 'livery-caret', '▸');
      const label = el('span', 'livery-name', esc(short(cur) || 'default'));
      head.append(caret, label);
      head.title = 'Change this car’s colour';

      const body = el('div', 'livery-body');
      const note = el('div', 'tiny dim');

      let open = false;
      head.onclick = () => {
        open = !open;
        body.classList.toggle('on', open);
        caret.textContent = open ? '▾' : '▸';
        if (open && !body.dataset.built) {
          body.dataset.built = '1';
          list.forEach(pth => {
            const row = el('a', 'livery-opt' + (pth === cur ? ' on' : ''),
                           esc(short(pth)));
            row.onclick = async () => {
              if (row.classList.contains('busy')) return;
              row.classList.add('busy');
              note.textContent = 'applying…';
              const r = await api('livery/apply', {
                file: owned.file, model: model,
                slot: 'EXT SKIN', color: pth,
              });
              row.classList.remove('busy');
              if (r && r.ok) {
                [...body.children].forEach(c => c.classList.remove('on'));
                row.classList.add('on');
                label.textContent = short(pth);
                note.textContent = 'Saved — applies next time the game starts';
              } else {
                note.textContent = (r && r.error) || 'could not apply that colour';
                if (r && r.error) toast(r.error, true);
              }
            };
            body.append(row);
          });
        }
      };
      liveryBox.append(head, body, note);
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
  const trkList = el('div', 'list drive-list');
  trkCol.append(trkHead, trkSearch, srvFilters, trkList);

  goCol.innerHTML = '<h2>Session</h2>';
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
  goCol.append(mkf('Game mode', mode), mkf('Weather', weather), mkf('Time', hour), extras);
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
  goCol.append(pwField, driveBtn, st, hint);
  const spFields = goCol.querySelectorAll(':scope > label.f');

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
        mkf('AI cars', numInp('num_opponents', 1, 40)),
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
        mkf('AI skill min', numInp('skill_min', 0, 100)),
        mkf('AI skill max', numInp('skill_max', 0, 100)),
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
        mkf('Race (laps)', numInp('race_laps', 1, 200)),
      );
    }
    if (m === 'INSTANT_RACE') {
      extras.append(
        mkf('Race (laps)', numInp('race_laps', 1, 200)),
        mkf('Start position (0=auto)', numInp('starting_position', 0, 40)),
      );
    }
  }

  function paintHead(box, imgSrc, title, sub) {
    box.innerHTML = '';
    const img = el('img');
    img.alt = '';
    img.src = imgSrc;
    img.onerror = () => { img.style.visibility = 'hidden'; };
    const t = el('div');
    t.innerHTML = `<b>${esc(title || 'Nothing selected')}</b>`
      + `<div class="tiny dim">${esc(sub || '')}</div>`;
    box.append(img, t);
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
            img.src = 'api/thumb/car?id=' + encodeURIComponent(c.model || c.id);
            img.onerror = () => { img.style.visibility = 'hidden'; };
            r.append(img);
          }
          const t = el('div', 'grow');
          t.innerHTML = `<div class="name">${esc(c.label)}</div>`
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
      chk('acecmOnly', 'ACECM only'));
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
      const sub = el('div', 'tiny dim',
        `${esc(s.track || '—')} · ${esc(s.layout || '')}`
        + ` · ${s.players || 0}/${s.max_players || 0}`
        + ` · ${esc(carsLine(s))}`
        + (s.ping ? ` · ${s.ping}ms` : ''));
      t.append(name, sub);
      const get = el('button', 'sm', 'Get content');
      get.title = 'Ask this host\'s ACECM for anything you are missing';
      get.onclick = ev => {
        ev.stopPropagation();
        contentFrom({
          server_ip: s.server_ip,
          server_tcp_port: s.server_tcp_port,
          track: s.track || '',
        });
      };
      r.append(t, get);
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
      r.append(t, listB);
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
    }
    paintHead(carHead,
      'api/thumb/car?id=' + encodeURIComponent((c && (c.model || c.id)) || ''),
      c ? c.label : 'Pick a car',
      c ? c.id : '');
    if (sel.via === 'server') {
      const s = serverOf();
      paintHead(trkHead,
        'api/thumb/track?folder=' + encodeURIComponent((s && s.track) || ''),
        s ? s.name : 'Pick a public server',
        s ? `${s.track || ''} · ${s.players || 0}/${s.max_players || 0}`
          + ` · ${carsLine(s)}`
          + (s.locked ? ' · password' : '') : '');
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
          + (s.no_lobby ? ' · private' : '') : '');
      return;
    }
    const t = trackOf(sel.track_index);
    paintHead(trkHead,
      'api/thumb/track?folder=' + encodeURIComponent((t && t.track) || ''),
      t ? (t.label || t.name) : 'Pick a track',
      t ? `#${t.index} · ${t.layout || ''}` : '');
  }

  function paintVia() {
    const on = sel.via === 'server' || sel.via === 'local';
    spFields.forEach(n => { n.style.display = on ? 'none' : ''; });
    extras.style.display = on ? 'none' : '';
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
  hour.onchange = () => { sel.tod_hour = Number(hour.value); };
  paintVia();

  async function poll() {
    if (_page !== 'drive') {
      stopDrivePoll();
      return;
    }
    try {
      const r = await fetch('/api/drive/status').then(x => x.json());
      if (_page !== 'drive') {
        stopDrivePoll();
        return;
      }
      const phase = r.phase || 'idle';
      const busy = ['writing', 'launching_game', 'starting_backend',
                    'waiting_for_menu', 'waiting_for_session',
                    'entering', 'selecting_car', 'starting_session',
                    'starting_server', 'joining',
                    'capturing_list', 'quitting_game'].includes(phase);
      driveBtn.disabled = busy;
      pull.disabled = busy;
      driveBtn.textContent = busy ? (r.hint || phase)
        : (sel.via === 'local'
          ? ((localOf() && !localOf().running) ? 'Start & Join' : 'Join')
          : (sel.via === 'server' ? 'Join' : 'Drive'));
      if (r.fault) {
        st.innerHTML = `<b style="color:var(--red)">${esc(r.fault)}</b>`;
      } else if (phase === 'launched') {
        st.innerHTML = `<b style="color:var(--ok)">${esc(r.hint || 'Launched')}</b>`;
        if ((r.captured || 0) > 0 && sel.via === 'server') {
          stopDrivePoll();
          driveReloadT = setTimeout(() => {
            driveReloadT = null;
            if (_page === 'drive') drivePage();
          }, 500);
        }
      } else if (busy) {
        st.textContent = r.hint || phase;
      } else if (r.game_running) {
        st.textContent = sel.via === 'server' || sel.via === 'local'
          ? 'Game is running — Join will set the car and push into the server.'
          : 'Game is running. Close it, then Drive.';
      } else {
        st.textContent = sel.via === 'local'
          ? 'Pick your ACECM server and an allowed car, then Join.'
          : (sel.via === 'server'
            ? 'Pick a public server and an allowed car, then Join.'
            : 'Pick a car and track, then Drive.');
      }
      if (!busy && driveTimer) {
        clearInterval(driveTimer);
        driveTimer = null;
      }
    } catch (e) {}
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
      starting_position: sel.starting_position,
    });
    if (!r.ok) {
      driveBtn.disabled = false;
      driveBtn.textContent = sel.via === 'server' ? 'Join' : 'Drive';
      st.innerHTML = `<b style="color:var(--red)">${esc(r.error || 'failed')}</b>`;
      return;
    }
    stopDrivePoll();
    driveTimer = setInterval(poll, 1200);
    poll();
  };
  poll();
}

/* ------------------------------------------------------------- shared -- */
/* _pill and _setText are shared: they write only when a value actually
   differs, which is what keeps a refresh from touching the DOM needlessly.
   They outlived the dashboard that introduced them. */
function _pill(node, on, text, offClass) {
  const cls = 'pill ' + (on ? 'on' : (offClass || 'off'));
  if (node.className !== cls) node.className = cls;
  const want = '<i class="dot"></i>' + esc(text);
  if (node.innerHTML !== want) node.innerHTML = want;
}

function _setText(node, text) {
  const s = String(text);
  if (node.textContent !== s) node.textContent = s;
}

/* ---- what needs attention ----------------------------------------------
   The one thing the dashboard did that nothing else did: say what is wrong.
   It lived on a page nobody landed on, so a broken server dir or an occupied
   port stayed invisible until you went looking. It is now a strip under the
   header on EVERY page - silent when there is nothing to say. */
let _attnKey = '';
async function refreshAttention() {
  const host = $('#attention');
  if (!host) return;
  const ov = await api('overview');
  // ⚠ PROBLEMS only. `attention` also carries level:"info" tips, and this
  // strip is on every page permanently - a standing tip rendered as a box
  // would be a warning that never goes away, which is how people learn to
  // ignore the strip entirely. Info items are explanatory, not actionable.
  const att = ((ov && ov.attention) || []).filter(a => a.level !== 'info');
  const key = JSON.stringify(att.map(a => [a.level, a.what, a.do]));
  if (key === _attnKey) return;          // nothing changed - leave the DOM be
  _attnKey = key;
  host.innerHTML = '';
  att.forEach(a => {
    const box = el('div', a.level === 'bad' ? 'err' : 'warn');
    box.innerHTML = '<b>' + esc(a.what) + '</b> ' + esc(a.do);
    host.append(box);
  });
}

/* -------------------------------------------------------------- servers -- */
let editing = null;
async function serversPage() {
  const [pr, trkWrap, worker] = await Promise.all([
    api('profiles'), api('tracks'), api('game/worker'),
  ]);
  const { profiles, template, options, telemetry: telState } = pr || {};
  const trk = (trkWrap && trkWrap.tracks) || [];
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

  // Penalties: install-wide, not per-profile - the trigger list lives inside
  // content.kspkg itself, so every profile hosted from this server shares
  // one on/off state. Shown here rather than in the per-profile editor
  // because that is exactly what it is not scoped to.
  const pc = el('div', 'card');
  const pens = await api('penalties');
  pc.innerHTML = '<h2>Penalties</h2>';
  const pdim = el('div', 'tiny dim',
    'A Kunos content update replaces content.kspkg wholesale, which silently '
    + 'resets this to ON along with everything else in the archive - if it '
    + 'looks flipped after an update, that is why, not a bug.');
  pc.append(pdim);
  const prow = el('div', 'row');
  for (const side of ['server', 'client']) {
    const info = pens && pens[side];
    if (!info) continue;
    const label = side === 'server' ? 'Dedicated server (affects everyone who joins)'
                                    : 'This client (affects only your own singleplayer)';
    const box = el('div', 'card');
    box.style.flex = '1';
    const isOff = info.state === 'off';
    const isOn = info.state === 'on';
    const stateText = isOff ? 'OFF' : isOn ? 'ON' : info.state;
    box.innerHTML = `<div><b>${esc(label)}</b></div>
      <div class="tiny dim">${esc(info.path)}</div>
      <div style="margin:6px 0">state: <b>${esc(stateText)}</b></div>`;
    if (isOff || isOn) {
      const btn = el('button', isOff ? 'sm' : 'sm danger',
        isOff ? 'Turn penalties ON' : 'Turn penalties OFF');
      btn.onclick = async () => {
        const r = await api('penalties/set', { side, off: isOn });
        toast(r.ok ? `Penalties now ${(r.state || '').toUpperCase()} (${side})`
                   : (r.error || 'Failed'), !r.ok);
        serversPage();
      };
      box.append(btn);
    }
    prow.append(box);
  }
  pc.append(prow);
  p.append(pc);

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
    // ⚠ refresh: content changes often (imports, updates, deletions) and a
  // cached map shows tracks that are gone or misses ones just added.
  const loc = await api('browser/local?refresh=1');
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
        &middot; ${esc((prof.car_policy && prof.car_policy.label) || 'all cars')}
        &middot; ${prof.max_players} slots
        ${prof.driver_password ? '&middot; 🔒' : ''}
        &middot; :${prof.tcp_port}</div></div>
        <span class="pill off" data-st="${prof.id}"><i class="dot"></i>checking</span>
      </div>`;
    const row = el('div', 'row wrap');
    row.style.marginTop = '10px';
    const start = el('button', 'primary sm', 'Start');
    start.dataset.start = prof.id;
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
      setTimeout(() => {
        clearInterval(tick);
        start.disabled = false; start.textContent = was;
        serversRefresh();
      }, 31000);
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
      setTimeout(serversRefresh, 900);
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
    tpill.dataset.tp = prof.id;
    const tOn = el('button', 'sm', ts.running ? 'Restart telemetry' : 'Start telemetry');
    tOn.dataset.ton = prof.id;
    tOn.onclick = async () => {
      const r = await api('telemetry/start', { id: prof.id });
      toast(r.ok ? `Telemetry on port ${r.port}` : (r.error || 'Failed'), !r.ok);
      setTimeout(serversRefresh, 1800);
    };
    const tOff = el('button', 'sm danger', 'Stop telemetry');
    tOff.dataset.toff = prof.id;
    tOff.disabled = !ts.running;
    tOff.onclick = async () => {
      await api('telemetry/stop', { id: prof.id });
      toast('Telemetry stopped'); setTimeout(serversRefresh, 800);
    };
    const tView = el('button', 'sm primary', 'View map');
    tView.dataset.tview = prof.id;
    tView.disabled = !ts.running;
    tView.onclick = () => { telProfile = prof.id; telTrack = null; go('telemetry'); };
    const tLink = el('button', 'sm', 'Copy live link');
    tLink.onclick = async () => {
      const r = await api('live/link');
      let url = (r && r.url) || (r && r.local_url) || '';
      if (url && prof.id) url += (url.includes('?') ? '&' : '?') + 'id=' + encodeURIComponent(prof.id);
      if (!url) { toast('No share address yet', true); return; }
      try { await navigator.clipboard.writeText(url); } catch (e) {}
      toast('Copied ' + url);
    };
    const wAI = el('button', 'sm', 'Attach AI worker');
    wAI.title = 'One client joins this server with -ai_player_car (AiDriverEvo), not vAI ghosts';
    wAI.onclick = async () => {
      const r = await api('game/attach_worker', { id: prof.id, ai_player: true });
      toast(r.ok ? (r.hint || 'Worker launching') : (r.error || 'Failed'), !r.ok);
      setTimeout(serversRefresh, 2000);
    };
    const more = el('details');
    more.style.marginTop = '8px';
    more.append(el('summary', 'tiny dim', 'More — logs, telemetry, AI worker, delete'));
    const extra = el('div', 'row wrap');
    extra.style.marginTop = '8px';
    extra.append(logs, tOn, tOff, tView, tLink, wAI, tpill, del);
    more.append(extra, pre);
    row.append(start, stop, edit);
    card.append(row, more);
    p.append(card);

  }
  // ⚠ ONE overview call, not a server/status request per profile. This page
  // fired N requests on every visit, each doing its own pid and port checks,
  // so it got slower with every server added - and the same numbers are
  // already in /api/overview, which the dashboard fetches anyway.
  serversRefresh();
}

/* Update the live parts of the servers page WITHOUT rebuilding it.
   A rebuild collapsed every open "More" panel, closed any log pane you were
   reading and scrolled the editor away - all to change a pill from "starting"
   to "running". Actions now patch the few elements that changed. */
async function serversRefresh() {
  const anchor = document.querySelector('[data-st]');
  if (!anchor) return;                    // not on this page any more
  const [ov, pr] = await Promise.all([api('overview'), api('profiles')]);
  const tel = (pr && pr.telemetry) || {};
  ((ov && ov.servers) || []).forEach(sv => {
    const pill = document.querySelector('[data-st="' + sv.id + '"]');
    if (pill) {
      pill.className = 'pill ' + (sv.running ? 'on' : 'off');
      pill.innerHTML = '<i class="dot"></i>' + (sv.running
        ? 'running' + (sv.clients != null ? ' · ' + sv.clients + ' clients' : '')
        : 'stopped');
    }
    // A start that has finished must give the button back, otherwise it stays
    // stuck on "starting… 0s" forever now that nothing rebuilds it.
    const start = document.querySelector('[data-start="' + sv.id + '"]');
    if (start && sv.running) {
      start.disabled = false;
      start.textContent = 'Start';
    }
  });
  Object.entries(tel).forEach(([id, ts]) => {
    const tp = document.querySelector('[data-tp="' + id + '"]');
    if (tp) {
      tp.className = 'pill ' + (ts.running ? 'on' : 'off');
      tp.innerHTML = '<i class="dot"></i>telemetry '
        + (ts.running ? 'on :' + ts.port : 'off');
    }
    const on = document.querySelector('[data-ton="' + id + '"]');
    if (on) on.textContent = ts.running ? 'Restart telemetry' : 'Start telemetry';
    const off = document.querySelector('[data-toff="' + id + '"]');
    if (off) off.disabled = !ts.running;
    const view = document.querySelector('[data-tview="' + id + '"]');
    if (view) view.disabled = !ts.running;
  });
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
      if (kind === 'number' && extra) {
        inp.step = extra.step || 1;
        if (extra.min != null) inp.min = extra.min;
        if (extra.max != null) inp.max = extra.max;
      }
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

  g = section('AI', 'leave at 0 for a player-only server. Skill spread stops them clumping');
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
  mk(g, 'tod_year', 'Year', 'number', { min: 2020, max: 2035 });
  mk(g, 'tod_month', 'Month', 'number');
  mk(g, 'tod_day', 'Day', 'number');
  mk(g, 'tod_second', 'Second', 'number');

  g = section('Visibility & output', 'private servers skip the public browser', true);
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

  // Picking a mod used to start a whitelist of THAT car only, which banned
  // every Kunos car. Default is: all stock cars stay allowed; tiles add mods.
  if (prof.allow_kunos == null)
    prof.allow_kunos = !((prof.cars || []).length) ||
      (prof.cars || []).every(id => {
        const m = ((extra && extra.cat && extra.cat.cars) || [])
          .find(x => x.id === id);
        return !m || m.mod;
      });
  g = section('Cars allowed',
    'All Kunos stays on unless you switch to “only the cars I pick”.');
  const cars = ((extra && extra.cat && extra.cat.cars) || []);
  const chosen = new Set(prof.cars || []);
  const box = el('div');
  box.style.cssText = 'grid-column:1/-1';
  const policy = el('div', 'row wrap');
  policy.style.marginBottom = '8px';
  const chips = el('div', 'row wrap');
  const setPolicy = (kind) => {
    if (kind === 'all') {
      prof.allow_kunos = true;
      chosen.clear();
    } else if (kind === 'kunos_plus') {
      prof.allow_kunos = true;
      [...chosen].forEach(id => {
        const m = cars.find(x => x.id === id);
        if (m && !m.mod) chosen.delete(id);
      });
    } else {
      prof.allow_kunos = false;
    }
    prof.cars = [...chosen];
    redraw();
  };
  const redraw = () => {
    policy.innerHTML = '';
    const kind = prof.allow_kunos
      ? (chosen.size ? 'kunos_plus' : 'all')
      : 'only';
    [['all', 'All cars'],
     ['kunos_plus', 'All Kunos + the mods I pick'],
     ['only', 'Only the cars I pick']].forEach(([k, lab]) => {
      const b = el('button', 'sm' + (kind === k ? ' primary' : ''), lab);
      b.onclick = () => setPolicy(k);
      policy.append(b);
    });
    chips.innerHTML = '';
    if (kind === 'all') {
      chips.append(el('span', 'tiny dim',
        'Every stock car and every installed mod is allowed.'));
    } else {
      chips.append(el('span', 'tiny dim',
        kind === 'kunos_plus'
          ? 'Stock cars are allowed. Click mods below to add them.'
          : 'Only the highlighted cars can join.'));
      [...chosen].forEach(id => {
        const meta = cars.find(x => x.id === id);
        const b = el('button', 'sm',
          `${esc(meta ? meta.label : id)} ✕`);
        b.title = id;
        b.onclick = () => { chosen.delete(id); prof.cars = [...chosen]; redraw(); };
        chips.append(b);
      });
    }
    paints.forEach(fn => fn());
  };
  /* Pick cars by looking at them.

     ⚠ A <select> cannot show a picture, and an id like preset_695b_mech_1
     tells you nothing about which car it is. These are the same Vulkan renders
     the gallery uses, keyed by MODEL - the catalogue is keyed by preset, and
     several presets share one model, so the thumbnail comes from x.model.

     Modded and Kunos are deliberately separate lists: mods are the ones you
     choose on purpose, and 11 of them were previously lost among 97. */
  const paints = [];
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
        const kind = prof.allow_kunos ? (chosen.size ? 'kunos_plus' : 'all') : 'only';
        if (kind === 'all') {
          prof.allow_kunos = !!x.mod;
          chosen.clear();
          chosen.add(x.id);
        } else if (on()) {
          chosen.delete(x.id);
        } else {
          chosen.add(x.id);
        }
        prof.cars = [...chosen];
        redraw();
      };
      paints.push(paint);
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

  /* ---- what this server pulls in ---------------------------------------
     Shown where the track and cars are chosen, because that is where the
     consequence belongs: pick modded content and it is shared for joiners and
     copied to the server automatically. The only thing worth a button is the
     one job that is slow and deliberate - putting a track into the server's
     own package. */
  const need = el('div', 'card');
  need.innerHTML = '<h2>Content this server needs</h2>';
  need.append(el('div', 'tiny dim',
    'Modded track and cars are shared for joiners automatically, and car mods '
    + 'are copied to the server when it starts. Stock content is never shared '
    + '— everyone already has it.'));
  const needBody = el('div');
  needBody.style.marginTop = '8px';
  need.append(needBody);
  (async () => {
    const info = await api('share/auto');
    const me = ((info && info.servers) || []).find(x => x.id === prof.id);
    needBody.innerHTML = '';
    if (!me) {
      needBody.append(el('div', 'tiny dim',
        'Save the server once and this will fill in.'));
      return;
    }
    const n = me.needs || {}, gaps = me.gaps || {};
    const bits = [...(n.tracks || []), ...(n.mods || [])];
    needBody.append(el('div', 'tiny dim', bits.length
      ? 'Modded content: ' + bits.map(esc).join(', ')
      : 'Nothing modded — this server needs no downloads.'));
    if ((gaps.mods || []).length) {
      needBody.append(el('div', 'warn',
        '<b>Not on the server yet:</b> ' + (gaps.mods || []).map(esc).join(', ')
        + '<br>These are copied over automatically when you start it.'));
    }
    if (gaps.track) {
      // ⚠ No button. The user asked for this to be automatic, and a button
      // that deploys is a button that can be forgotten - which is exactly how
      // a server ends up advertising a track it cannot host. Starting the
      // server puts it into the package.
      const box = el('div', 'warn');
      box.innerHTML = '<b>' + esc(gaps.track) + ' is not in the server '
        + 'content package yet.</b><br>It is added automatically when you '
        + 'start this server. That rewrites the package index, so the first '
        + 'start after adding a track takes longer than usual.';
      needBody.append(box);
    }
  })();
  // ⚠ into the editor card, NOT a page-level `p` - there is no such variable
  // in this function, and referencing it threw before anything was appended,
  // so the whole Servers page rendered blank after opening an editor.
  c.append(need);

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
  const [viewer, st, cat, pr] = await Promise.all([
    api('viewer/cars'), api('thumbs/status'), api('cars'), api('profiles'),
  ]);
  const cars = (viewer && viewer.cars) || [];
  const have = new Set((st && st.have) || []);
  const profs = (pr && pr.profiles) || [];

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

  async function savePolicy(allowKunos, list) {
    prof.allow_kunos = !!allowKunos;
    prof.cars = list;
    const r = await api('profiles/save', prof);
    if (r && r.error) { toast(r.error, true); return; }
    toast(allowKunos
      ? (list.length ? 'All Kunos + ' + list.length + ' extra' : 'Every car allowed')
      : (list.length ? list.length + ' cars only' : 'All Kunos'));
    draw();
  }
  async function saveAllowed(list) {
    return savePolicy(prof.allow_kunos !== false, list);
  }
  const all = el('button', 'sm', 'Allow all cars');
  all.onclick = () => prof && savePolicy(true, []);
  const plusKunos = el('button', 'sm', 'Allow all Kunos');
  plusKunos.title = 'Stock cars stay allowed. Mods you already picked stay too.';
  plusKunos.onclick = () => {
    if (!prof) return;
    const mods = (prof.cars || []).filter(id => {
      const m = (cat.cars || []).find(x => x.id === id);
      return !m || m.mod;
    });
    savePolicy(true, mods);
  };
  const onlyMods = el('button', 'sm', 'Only mods');
  onlyMods.onclick = () => prof && savePolicy(false,
    (cat.cars || []).filter(x => x.mod).map(x => x.id));
  row.append(all, plusKunos, onlyMods);
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
          const wideOpen = !cur.size && prof.allow_kunos !== false;
          const on = wideOpen || presets.every(q => cur.has(q.id));
          const t = el('button', on ? 'sm primary' : 'sm',
                       on ? 'Allowed' : 'Allow');
          t.title = wideOpen
            ? 'Every car is allowed. Click a mod to keep all Kunos and add it.'
            : '';
          t.onclick = () => {
            if (wideOpen) {
              const isMod = presets.some(q => q.mod);
              savePolicy(!!isMod, presets.map(q => q.id));
              return;
            }
            const set = allowed();
            presets.forEach(q => on ? set.delete(q.id) : set.add(q.id));
            savePolicy(prof.allow_kunos !== false, [...set]);
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

/* ---- cars ---------------------------------------------------------------
   Master-detail, after Content Manager: one dense list of everything on the
   left, one car's details on the right.

   ⚠ This page used to stack FOUR views of the same 114 cars - a picture
   gallery, a searchable text list, a "models seen on this server" list and a
   note about mods - so you scrolled past the same cars three times before
   reaching anything new. One list, one detail pane, filters instead of
   sections.

   ⚠ Two different ids, and the difference is invisible if you get it wrong.
   The list and its renders are keyed by MODEL (ks_abarth_695_biposto); a
   server's allowed-cars list is keyed by PRESET (preset_695b_mech_1), and one
   model usually has several presets. Allowing "a car" therefore means allowing
   every preset of that model.                                               */
let carSel = null;          // selected model id
let carScope = 'all';       // all | kunos | mods
let carThumbBuild = false;  // only ever kick one build off per session

/* Fill in car pictures that were never rendered.

   Renders are keyed by MODEL, the same id thumbs/status reports, so the two
   lists can be compared directly - comparing against preset ids finds
   everything missing and would rebuild the world. Runs at most once per
   session, and only when something is actually absent. */
async function buildMissingCarThumbs(models) {
  if (carThumbBuild) return;
  const st = await api('thumbs/status');
  if (!st || st.error) return;
  const have = new Set((st.have || []).filter(h => !h.endsWith('@big')));
  const missing = models.filter(m => m.model && !have.has(m.model));
  if (!missing.length || st.state === 'running') return;
  carThumbBuild = true;
  toast(`Rendering ${missing.length} missing car picture`
        + (missing.length === 1 ? '' : 's') + ' in the background…');
  const r = await api('thumbs/build', {});
  if (!r || r.error) { carThumbBuild = false; return; }
  const poll = setInterval(async () => {
    const j = await api('thumbs/status');
    if (!j || j.state === 'running') return;
    clearInterval(poll);
    // repaint so the new pictures actually appear, but only if the user is
    // still looking at this page
    if (j.made) {
      toast(`${j.made} car picture${j.made === 1 ? '' : 's'} rendered`);
      if (_page === 'cars') carsPage();
    }
  }, 2000);
}

async function carsPage() {
  const [cat, viewer, pr] = await Promise.all([
    api('cars'), api('viewer/cars'), api('profiles'),
  ]);
  const p = $('#page');
  p.innerHTML = '';

  const cars = cat.cars || [];
  const renderable = new Set(((viewer && viewer.cars) || []).map(c => c.id || c));
  const profs = (pr && pr.profiles) || [];

  // one entry per MODEL, carrying its presets
  const models = new Map();
  cars.forEach(c => {
    const key = c.model || c.id;
    if (!models.has(key)) {
      models.set(key, { model: key, label: c.label, brand: c.brand || '',
                        kunos: !!c.kunos, mod: !!c.mod, presets: [] });
    }
    const m = models.get(key);
    m.presets.push(c);
    if (c.mod) m.mod = true;
    if (!c.kunos) m.kunos = false;
  });
  const all = [...models.values()].sort((a, b) =>
    (a.brand || '').localeCompare(b.brand || '') || a.label.localeCompare(b.label));

  // ⚠ The list's own image request is make=False on purpose - rendering
  // inline used to launch evoview per row and steal focus while typing. So a
  // car with no cached render just shows nothing, for ever, and there was no
  // sign that a render was all it needed. Build the missing ones once, in the
  // background: render_car returns immediately for anything already cached,
  // so this costs only the cars that are actually missing.
  buildMissingCarThumbs(all);

  const wrap = el('div', 'split');
  const leftCol = el('div', 'split-list');
  const rightCol = el('div', 'split-detail');
  wrap.append(leftCol, rightCol);
  p.append(wrap);

  // ---- filters ----------------------------------------------------------
  const bar = el('div', 'filterbar');
  const search = el('input');
  search.placeholder = 'Filter cars…';
  search.value = carFilter;
  search.oninput = () => { carFilter = search.value.toLowerCase(); drawList(); };
  const chips = el('div', 'chips');
  [['all', 'All'], ['kunos', 'Kunos'], ['mods', 'Mods']].forEach(([k, lbl]) => {
    const a = el('a', 'chip' + (carScope === k ? ' on' : ''), lbl);
    a.onclick = () => { carScope = k; drawList(); };
    chips.append(a);
  });
  bar.append(search, chips);
  leftCol.append(bar);

  const list = el('div', 'rows');
  leftCol.append(list);
  const count = el('div', 'tiny dim');
  leftCol.append(count);

  function match(m) {
    if (carScope === 'kunos' && !m.kunos) return false;
    if (carScope === 'mods' && !m.mod) return false;
    if (!carFilter) return true;
    return (m.label + ' ' + m.model + ' ' + m.brand).toLowerCase()
      .includes(carFilter);
  }

  function drawList() {
    list.innerHTML = '';
    const rows = all.filter(match);
    count.textContent = rows.length + ' of ' + all.length + ' cars'
      + ' · ' + cat.kunos + ' Kunos · ' + cat.mods + ' modded';
    if (!rows.length) { list.append(el('div', 'empty', 'No matches')); return; }
    let brand = null;
    rows.forEach(m => {
      if (m.brand !== brand) {
        brand = m.brand;
        list.append(el('div', 'rowhead', esc(brand || 'Other')));
      }
      const r = el('a', 'rowitem' + (carSel === m.model ? ' on' : ''));
      r.innerHTML = '<span class="name">' + esc(m.label) + '</span>'
        + (m.mod ? '<span class="tag">mod</span>' : '')
        + (m.presets.length > 1
            ? '<span class="tag dim">' + m.presets.length + '</span>' : '');
      r.onclick = () => { carSel = m.model; drawList(); drawDetail(); };
      list.append(r);
    });
    // keep the chip row honest after a scope change
    [...chips.children].forEach(c =>
      c.classList.toggle('on', c.textContent.toLowerCase() ===
        (carScope === 'all' ? 'all' : carScope === 'kunos' ? 'kunos' : 'mods')));
  }

  // ---- detail -----------------------------------------------------------
  function drawDetail() {
    rightCol.innerHTML = '';
    const m = models.get(carSel);
    if (!m) {
      rightCol.append(el('div', 'empty', 'Pick a car on the left'));
      return;
    }
    const head = el('div', 'dhead');
    head.innerHTML = '<div class="dbrand">' + esc(m.brand || '') + '</div>'
      + '<h2 class="dname">' + esc(m.label) + '</h2>'
      + '<div class="tiny dim">' + esc(m.model) + '</div>';
    rightCol.append(head);

    const shot = el('div', 'dshot');
    const img = el('img');
    img.loading = 'lazy';
    img.alt = m.label;
    // ⚠ big=1: the list thumbnail is 480px and this pane is far wider, so
    // reusing it here shows an upscaled, soft picture. This renders the one
    // car being looked at at full size, and the request may take a moment
    // the first time because evoview has to draw it.
    const shotUrl = (force) => 'api/thumb/car?big=1&id='
      + encodeURIComponent(m.model) + (force ? '&force=1&t=' + Date.now() : '');
    img.src = shotUrl(false);
    // ⚠ A car with no render must not leave a broken-image icon sitting in
    // the middle of the pane - say what is missing instead.
    img.onerror = () => {
      shot.innerHTML = '';
      shot.append(el('div', 'empty', renderable.has(m.model)
        ? 'Render not built yet' : 'No 3D model available for this car'));
    };
    shot.append(img);
    rightCol.append(shot);

    const acts = el('div', 'row wrap');
    const view = el('button', 'sm primary', 'View in 3D');
    view.disabled = !renderable.has(m.model);
    if (view.disabled) view.title = 'The viewer has no folder for this car';
    view.onclick = async () => {
      view.disabled = true;
      const r = await api('viewer/open', { id: m.model });
      view.disabled = false;
      toast(r && r.ok ? 'Opening ' + m.label + ' in the viewer'
                      : ((r && r.error) || 'could not open the viewer'),
            !(r && r.ok));
    };
    const shoot = el('button', 'sm', 'Re-render photo');
    shoot.title = 'Draw this car again with evoview, replacing the cached image';
    shoot.onclick = async () => {
      shoot.disabled = true;
      const was = shoot.textContent;
      shoot.textContent = 'rendering…';
      // the browser will not re-fetch an identical URL, hence the timestamp
      await new Promise(done => {
        const probe = new Image();
        probe.onload = probe.onerror = done;
        probe.src = shotUrl(true);
      });
      img.src = shotUrl(true);
      shoot.disabled = false;
      shoot.textContent = was;
      toast('Re-rendered ' + m.label);
    };
    acts.append(view, shoot);
    rightCol.append(acts);

    // ---- allowed on a server -------------------------------------------
    const pol = el('div', 'card');
    pol.innerHTML = '<h2>Allowed on a server</h2>';
    const psel = el('select');
    const none = el('option', null, 'Pick a server profile…');
    none.value = '';
    psel.append(none);
    profs.forEach(x => {
      const o = el('option', null, x.name);
      o.value = x.id;
      if (x.id === galProfile) o.selected = true;
      psel.append(o);
    });
    psel.onchange = () => { galProfile = psel.value; drawDetail(); };
    pol.append(psel);

    const prof = profs.find(x => x.id === galProfile) || null;
    if (prof) {
      const allowed = new Set(prof.cars || []);
      const ids = m.presets.map(x => x.id);
      const on = ids.every(id => allowed.has(id));
      const state = el('div', 'tiny dim');
      state.style.margin = '8px 0';
      const extra = (prof.cars || []).length;
      state.textContent = prof.allow_kunos
        ? (extra ? 'All Kunos cars are allowed, plus ' + extra + ' extra'
                 : 'All Kunos cars are allowed')
        : (extra ? extra + ' car(s) allowed explicitly'
                 : 'No cars allowed explicitly yet');
      pol.append(state);
      const t = el('button', 'sm ' + (on ? 'danger' : 'primary'),
                   on ? 'Remove from this server' : 'Allow on this server');
      t.onclick = async () => {
        const set = new Set(prof.cars || []);
        // every preset of the model moves together - a half-allowed car looks
        // identical to an allowed one in the game's list
        ids.forEach(id => on ? set.delete(id) : set.add(id));
        prof.cars = [...set];
        const r = await api('profiles/save', prof);
        if (r && r.error) { toast(r.error, true); return; }
        toast(on ? 'Removed ' + m.label : 'Allowed ' + m.label);
        drawDetail();
      };
      pol.append(t);
    }
    rightCol.append(pol);

    if (m.presets.length) {
      const pc = el('div', 'card');
      pc.innerHTML = '<h2>Presets &middot; ' + m.presets.length + '</h2>';
      const rows = el('div', 'rows');
      m.presets.forEach(x => {
        const r = el('div', 'rowitem');
        r.innerHTML = '<span class="name">' + esc(x.label) + '</span>'
          + '<span class="id">' + esc(x.id) + '</span>';
        rows.append(r);
      });
      pc.append(rows);
      rightCol.append(pc);
    }
  }

  drawList();
  drawDetail();

  // Installing and removing car mods is a real job that belongs on this page,
  // so it stays - below the browser rather than competing with it.
  await modStrip(p);
}

/* --------------------------------------------------------------- tracks -- */
async function tracksPage() {
  const [d, loc, st] = await Promise.all([
    api('tracks'), api('browser/local?refresh=1'), api('thumbs/status'),
  ]);
  const p = $('#page');
  p.innerHTML = '';

  // Every track the game can load, with the cover art it ships. Only some
  // tracks have one; the rest show a blank tile rather than a placeholder
  // pretending to be a photo.
  const map = (loc && loc.track_map) || {};
  const gal = el('div', 'card');
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
      const view = el('button', 'sm', 'View 3D');
      view.style.marginTop = '6px';
      view.onclick = async (ev) => {
        ev.stopPropagation();
        view.disabled = true;
        const r = await api('viewer/open_track', { folder });
        view.disabled = false;
        toast(r && r.ok ? ('Opening ' + name + ' in the viewer')
                        : (r && r.error || 'could not open the viewer'),
              !(r && r.ok));
      };
      cap.append(view);
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
    + `${b.client_patched ? 'rdata → local backend' : 'rdata still Kunos'}</span>`
    + `<span class="pill ${b.inspector_patched ? 'on' : 'off'}"><i class="dot"></i>`
    + `${b.inspector_patched ? 'menu inspector :' + (b.inspector_port || 9444)
                             : 'menu inspector off — Drive cannot Start'}</span>`));

  const seen = (b.game_backend || {}).url;
  if (seen) {
    const local = /localhost|127\.0\.0\.1/.test(seen);
    c.append(el('div', local ? 'tiny dim' : 'warn',
      local
        ? `Game log: talking to <code>${esc(seen)}</code>`
        : `Game log: still talking to <code>${esc(seen)}</code> — `
          + 'Steam dropped <code>-backend=</code>. Close the game and Launch '
          + 'from ACECM so the lobby URL is rewritten.'));
  }

  const cu = b.client_url || {};
  const red = el('div');
  red.style.marginTop = '12px';
  red.innerHTML = '<div class="tiny dim" style="margin-bottom:8px">'
    + '<b>Required on a fresh install:</b> rewrite the lobby URL in the exe '
    + 'and turn on the Gameface inspector. Steam relaunches the game with no '
    + 'arguments (<code>Arguments: 1</code>), so <code>-backend=</code> never '
    + 'arrives, and stock EVO leaves the inspector off — Drive then launches '
    + 'and sits on the home menu. Launch from ACECM does both rewrites if the '
    + 'game is closed, and starts Steam first if it is not already up. '
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

  const pc = el('div', 'card');
  pc.innerHTML = '<h2>Lobby proxy</h2>'
    + '<div class="tiny dim" style="margin-bottom:8px">The game talks to Kunos '
    + 'through this process. Drive\'s public server list and Join need it.</div>';
  const pl = el('label', 'ctl');
  const pbox = el('input');
  pbox.type = 'checkbox';
  pbox.checked = cfg.auto_proxy !== false;
  pbox.onchange = async () => {
    const on = pbox.checked;
    await api('config', { auto_proxy: on });
    if (on) {
      const r = await api('backend/start', { mode: 'proxy' });
      toast(r.ok
        ? 'Proxy will start with ACECM and stop when you close the window'
        : (r.error || 'Saved, but the proxy did not start'), !r.ok);
    } else {
      toast('Proxy is manual — it will not start or stop with ACECM');
    }
  };
  pl.append(pbox, el('span', null,
    'Start the lobby proxy when ACECM opens, and stop it when ACECM closes'));
  pc.append(pl);
  p.append(pc);

  const c = el('div', 'card');
  c.innerHTML = '<h2>Paths &amp; ports</h2>';
  const g = el('div', 'grid g2');
  const draft = {};
  Object.entries(cfg).forEach(([k, v]) => {
    if (k === 'auto_proxy' || k === 'update_token_set' || typeof v === 'boolean') return;
    const l = el('label', 'f', `<span>${esc(k)}</span>`);
    const i = el('input');
    if (k === 'update_token' && cfg.update_token_set) {
      i.placeholder = '(token saved — leave blank to keep it)';
    }
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
  // ⚠ refresh: content changes often (imports, updates, deletions) and a
  // cached map shows tracks that are gone or misses ones just added.
  const loc = await api('browser/local?refresh=1');
  const reg = await api('registry');
  const shared = {};
  (reg.servers || []).forEach(e => (e.required_tracks || []).forEach(
    t => { shared[t] = e; }));

  const imported = new Set(loc.tracks || []);
  const rows = Object.entries(loc.track_map || {})
    .filter(([, folder]) => imported.has(folder))
    .sort((a, b) => a[0].localeCompare(b[0]));

  const share = await api('share');
  // which servers pull in which track, so each row can say WHY it is shared
  const autoInfo = await api('share/auto');
  const autoWho = {};
  ((autoInfo && autoInfo.servers) || []).forEach(sv => {
    ((sv.needs && sv.needs.tracks) || []).forEach(t => {
      (autoWho[t] = autoWho[t] || []).push(sv.name || 'a server');
    });
  });
  const c = el('div', 'card');
  c.innerHTML = '<h2>Shared for download</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">A player who does not '
    + 'have your track cannot join, and the game will not send it to them. '
    + 'Share it here, copy the link, and they paste it into '
    + '<b>Server browser → Fetch from ACECM</b>. Only tracks you imported '
    + 'are listed — stock tracks everyone already has.</div>';
  // ⚠ TWO links, labelled by who they are for. Handing out one LAN address and
  // telling people to "swap in your public IP" is how a share link that cannot
  // possibly work gets sent to a friend on another network - the server list
  // reaches them through the lobby without touching this PC, so everything
  // looks fine right up until the download.
  function linkRow(label, url, note, primary) {
    const wrap = el('div');
    wrap.style.marginBottom = '10px';
    wrap.append(el('div', 'tiny dim', label));
    const box = el('div', 'row wrap');
    const inp = el('input');
    inp.readOnly = true;
    inp.value = url;
    inp.style.minWidth = '18em';
    inp.onclick = () => inp.select();
    const copy = el('button', 'sm' + (primary ? ' primary' : ''), 'Copy');
    copy.onclick = async () => {
      try { await navigator.clipboard.writeText(url); } catch (e) {}
      toast('Copied ' + url);
    };
    box.append(inp, copy);
    wrap.append(box);
    if (note) wrap.append(el('div', 'tiny dim', note));
    return wrap;
  }

  if (share.public_url) {
    c.append(linkRow('For anyone outside your network', share.public_url,
      esc(share.public_note || ''), true));
  } else if (share.public_note) {
    c.append(el('div', 'tiny dim', esc(share.public_note)));
  }
  if (share.lan_url) {
    c.append(linkRow('For someone on your own network', share.lan_url,
      `They need <b>TCP ${share.port}</b> open in Windows Firewall. `
      + 'ACECM stays open while they download.', !share.public_url));
  }
  if (share.lan_url || share.public_url) {
    c.append(el('div', 'tiny dim',
      `Joining also needs the game server itself: <b>TCP+UDP 9700</b>. `
      + `From the internet, forward <b>${share.port}</b> and <b>9700</b> to `
      + 'this PC.'));
  }
  if (!rows.length) {
    c.append(el('div', 'empty', 'No imported tracks yet'));
  } else {
    /* ⚠ No Share / Stop sharing buttons any more. Working out which of your
       content a joining player is missing is a question ACECM can answer from
       the server profiles, so sharing follows what you host instead of being a
       list to keep in step by hand. This is the READ-OUT of that. */
    rows.forEach(([name, folder]) => {
      const row = el('div', 'chk');
      const auto = (autoWho[folder] || []);
      const on = auto.length || !!shared[folder];
      const why = auto.length
        ? 'shared for ' + auto.join(', ')
        : (shared[folder] ? 'shared manually' : 'not needed by any server yet');
      row.innerHTML = `<span class="name"><b>${esc(name)}</b>`
        + `<div class="tiny dim">${esc(folder)} — ${esc(why)}</div></span>`
        + `<span class="pill ${on ? 'on' : 'off'}"><i class="dot"></i>`
        + `${on ? 'shared' : 'idle'}</span>`;
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

async function copyText(text, label) {
  const t = String(text || '');
  if (!t) { toast('Nothing to copy', true); return; }
  try {
    await navigator.clipboard.writeText(t);
    toast('Copied ' + (label || t));
  } catch (e) {
    toast('Could not copy', true);
  }
}

async function apiLong(path, body, ms) {
  const opt = body ? { method: 'POST', body: JSON.stringify(body) } : {};
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), ms || 30 * 60 * 1000);
  try {
    const r = await fetch('/api/' + path, { ...opt, signal: ctrl.signal });
    const j = await r.json().catch(() => ({ error: 'bad response' }));
    if (j && j.error && !j.need_confirm) toast(j.error, true);
    return j;
  } catch (e) {
    const msg = (e && e.name === 'AbortError')
      ? (path + ' timed out') : String(e && e.message || e);
    return { error: msg };
  } finally {
    clearTimeout(t);
  }
}

function dropId() {
  if (crypto.randomUUID) return crypto.randomUUID();
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, c => {
    const r = Math.random() * 16 | 0;
    return (c === 'x' ? r : (r & 0x3 | 0x8)).toString(16);
  });
}

function overwritePrompt(r) {
  const name = (r && (r.label || r.name)) || 'this';
  if (r && r.same) {
    return confirm('"' + name + '" is already installed and these files '
      + 'are the same.\n\nOverwrite it anyway?');
  }
  return confirm('"' + name + '" is already installed, but these files '
    + 'are different.\n\nOverwrite the installed copy?');
}

async function finishIngest(payload) {
  let r = await apiLong('drop', payload);
  if (r && r.need_confirm) {
    if (!overwritePrompt(r)) {
      if (payload.id) await api('drop', { id: payload.id, cancel: true });
      toast('Install cancelled');
      return { ok: false, cancelled: true };
    }
    r = await apiLong('drop', { ...payload, overwrite: true });
  }
  return r;
}

/* ---- drop progress ------------------------------------------------------
   A dropped track is a gigabyte or more. It used to show one toast and then
   nothing at all for minutes, which is indistinguishable from the app having
   hung - so people drop the file again, and now two installs are running.

   ⚠ fetch() cannot report UPLOAD progress; only XMLHttpRequest exposes
   upload.onprogress. That is the whole reason this is not a fetch call.      */
function dropProgress() {
  let box = $('#dropprog');
  if (!box) {
    box = el('div');
    box.id = 'dropprog';
    box.innerHTML = '<div class="dp-what"></div>'
      + '<div class="dp-track"><div class="dp-bar"></div></div>'
      + '<div class="dp-detail tiny dim"></div>';
    document.body.append(box);
  }
  const bar = box.querySelector('.dp-bar');
  return {
    show(what) {
      box.classList.add('on');
      box.querySelector('.dp-what').textContent = what;
      bar.style.width = '0%';
      bar.classList.remove('indet');
      box.querySelector('.dp-detail').textContent = '';
    },
    set(frac, detail) {
      bar.classList.remove('indet');
      bar.style.width = Math.max(0, Math.min(1, frac)) * 100 + '%';
      if (detail != null) box.querySelector('.dp-detail').textContent = detail;
    },
    // the server side gives no byte-level feedback, so say so honestly with a
    // moving bar rather than a percentage we would be inventing
    busy(what, detail) {
      box.querySelector('.dp-what').textContent = what;
      bar.style.width = '100%';
      bar.classList.add('indet');
      if (detail != null) box.querySelector('.dp-detail').textContent = detail;
    },
    hide() { box.classList.remove('on'); },
  };
}

function mb(n) { return (n / 1048576).toFixed(1) + ' MB'; }

function putFile(url, file, onProgress) {
  return new Promise(resolve => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', url);
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    xhr.upload.onprogress = e => {
      if (e.lengthComputable) onProgress(e.loaded, e.total);
    };
    xhr.onload = () => {
      let j;
      try { j = JSON.parse(xhr.responseText); }
      catch (e) { j = { error: 'upload failed (' + xhr.status + ')' }; }
      resolve(j);
    };
    xhr.onerror = () => resolve({ error: 'upload failed - connection lost' });
    xhr.onabort = () => resolve({ error: 'upload cancelled' });
    xhr.send(file);
  });
}

async function uploadDropped(files) {
  const id = dropId();
  const prog = dropProgress();
  const total = files.reduce((n, f) => n + (f.size || 0), 0);
  let done = 0;
  const started = Date.now();
  prog.show('Copying ' + files.length + ' file'
            + (files.length === 1 ? '' : 's') + '…');
  try {
    for (const f of files) {
      const r = await putFile(
        '/api/drop/part?id=' + encodeURIComponent(id)
        + '&name=' + encodeURIComponent(f.name),
        f,
        (loaded) => {
          const secs = Math.max(0.001, (Date.now() - started) / 1000);
          const rate = (done + loaded) / secs;
          const left = rate > 0 ? (total - done - loaded) / rate : 0;
          prog.set(total ? (done + loaded) / total : 0,
            `${f.name} — ${mb(done + loaded)} of ${mb(total)}`
            + (rate ? ` · ${mb(rate)}/s` : '')
            + (left > 2 ? ` · ${Math.ceil(left)}s left` : ''));
        });
      if (!r.ok) { prog.hide(); return r; }
      done += f.size || 0;
    }
    // ⚠ Unpacking is the SLOW half for a big track - thousands of files and
    // several GB - and it used to show a bare "Installing…" for minutes,
    // which reads as a hang. The drop request runs for that whole time, so
    // ask the server separately what it is up to.
    prog.busy('Installing…', 'unpacking and registering the content');
    const watch = setInterval(async () => {
      const st = await api('drop/status');
      if (!st || st.state !== 'running') return;
      if (st.total) {
        prog.set(st.done / st.total,
                 `${st.detail} — ${mb(st.done)} of ${mb(st.total)}`);
      } else {
        prog.busy('Installing…', st.detail || '');
      }
    }, 600);
    try {
      return await finishIngest({ id });
    } finally {
      clearInterval(watch);
    }
  } finally {
    prog.hide();
  }
}

function collectDropped(dt) {
  return new Promise(resolve => {
    const items = dt && dt.items ? [...dt.items] : [];
    const files = [];
    let pending = 0;
    const finish = () => { if (!pending) resolve(files); };
    const walk = (entry) => {
      if (!entry) return;
      if (entry.isFile) {
        pending++;
        entry.file(f => { files.push(f); pending--; finish(); },
                   () => { pending--; finish(); });
      } else if (entry.isDirectory) {
        const reader = entry.createReader();
        pending++;
        reader.readEntries(ents => {
          pending--;
          if ((ents || []).length > 40) files.tooMany = true;
          else (ents || []).forEach(walk);
          finish();
        }, () => { pending--; finish(); });
      }
    };
    const entries = items
      .map(i => i.webkitGetAsEntry && i.webkitGetAsEntry())
      .filter(Boolean);
    if (entries.length) {
      entries.forEach(walk);
      finish();
    } else {
      resolve([...(dt.files || [])]);
    }
  });
}

function refreshOpenPage() {
  if (_page === 'drive') drivePage();
  else if (_page === 'content') contentPage();
  else if (_page === 'cars') carsPage();
  else if (_page === 'tracks') tracksPage();
  else if (_page === 'servers') serversPage();
}

function ingestToast(r) {
  if (r && r.cancelled) return;
  if (r && r.need_confirm) return;
  if (!r || r.error) {
    toast((r && (r.error || r.warning)) || 'Install failed', true);
    return;
  }
  if (r.kind === 'track') {
    toast(r.warning
      ? `Installed ${r.display_name || r.folder} — ${r.warning}`
      : `Installed track ${r.display_name || r.folder}`, !!r.warning);
    refreshOpenPage();
    return;
  }
  const n = (r.installed || []).length;
  toast(r.warning || `Installed ${n} file(s) on client and server`, !!r.warning);
  refreshOpenPage();
}

function bindDropAnywhere() {
  const veil = $('#dropveil');
  if (!veil || veil.dataset.bound) return;
  veil.dataset.bound = '1';
  let depth = 0;
  const hasFiles = (e) => {
    const t = e.dataTransfer;
    return !!(t && [...(t.types || [])].includes('Files'));
  };
  document.addEventListener('dragenter', e => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    depth++;
    veil.classList.add('on');
  });
  document.addEventListener('dragover', e => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
  });
  document.addEventListener('dragleave', e => {
    if (!hasFiles(e)) return;
    depth = Math.max(0, depth - 1);
    if (!depth) veil.classList.remove('on');
  });
  document.addEventListener('drop', async e => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.stopPropagation();
    depth = 0;
    veil.classList.remove('on');
    const files = await collectDropped(e.dataTransfer);
    if (files.tooMany || files.length > 40) {
      toast('Unpacked tracks are too large to drop as a folder — '
        + 'export a .tar from Content and drop that', true);
      return;
    }
    if (!files.length) { toast('Nothing to install', true); return; }
    toast('Installing ' + files.map(f => f.name).join(', ') + '…');
    const r = await uploadDropped(files);
    ingestToast(r);
    if (r && r.ok && _page === 'content') contentPage();
  });
}

function libRow(item, shareUrl) {
  const row = el('div', 'chk');
  const cars = (item.cars || []).map(x => esc(x.label || x.id)).join(', ');
  const sub = item.kind === 'car'
    ? ((item.size_mb || 0) + ' MB'
      + (cars ? ' · ' + cars : '')
      + (item.client_ok ? '' : ' · not on client')
      + (item.server_ok ? '' : ' · not on server'))
    : ((item.size_mb ? item.size_mb + ' MB · ' : '')
      + esc(item.folder || item.name)
      + (item.layout ? ' · ' + esc(item.layout) : '')
      + (item.files ? ' · ' + item.files + ' files' : ''));
  const ready = item.kind === 'car' ? !!item.ok : !!item.ok;
  row.innerHTML =
    `<span class="name"><b>${esc(item.label || item.name)}</b>`
    + ` <span class="pill ${item.kind === 'track' ? 'acecm' : 'off'}">`
    + `${item.kind}</span>`
    + (item.shared ? ' <span class="pill on">shared</span>' : '')
    + `<div class="tiny dim">${sub}`
    + (item.issues || []).map(i =>
      `<br><span style="color:var(--gold)">${esc(i)}</span>`).join('')
    + (item.error
      ? `<br><span style="color:var(--gold)">${esc(item.error)}</span>` : '')
    + `</div></span>`
    + `<span class="pill ${ready ? 'on' : 'warn'}"><i class="dot"></i>`
    + `${ready ? 'ready' : 'incomplete'}</span>`;
  const act = el('div', 'lib-act');
  const exp = el('button', 'sm', 'Export');
  exp.title = item.kind === 'track'
    ? 'Save the multiplayer .tar (same file Get content downloads)'
    : 'Save a zip of .kspkg + .json';
  exp.onclick = async (ev) => {
    ev.stopPropagation();
    toast('Exporting — a track pack can take a minute…');
    const r = await apiLong('library/export',
                            { kind: item.kind, name: item.name });
    if (r && r.ok) {
      toast('Saved ' + (r.path || 'to Downloads'));
      copyText(r.path, r.path);
    }
  };
  const copy = el('button', 'sm', 'Copy');
  copy.title = 'Copy the name friends use (folder / mod id)';
  copy.onclick = async (ev) => {
    ev.stopPropagation();
    const r = await api('library/clip?kind=' + encodeURIComponent(item.kind)
      + '&name=' + encodeURIComponent(item.name));
    const v = (r && r.variants) || {};
    // name first — that is what a pack / share entry uses
    await copyText(v.name || item.name, v.name || item.name);
  };
  const pathB = el('button', 'sm', 'Path');
  pathB.title = 'Copy the install path';
  pathB.onclick = async (ev) => {
    ev.stopPropagation();
    const r = await api('library/clip?kind=' + encodeURIComponent(item.kind)
      + '&name=' + encodeURIComponent(item.name));
    await copyText((r && r.path) || item.path, 'path');
  };
  if (item.shared && shareUrl) {
    const link = el('button', 'sm', 'Link');
    link.title = 'Copy the Get content share URL';
    link.onclick = (ev) => {
      ev.stopPropagation();
      copyText(shareUrl, shareUrl);
    };
    act.append(link);
  }
  const rm = el('button', 'sm danger', 'Delete');
  rm.onclick = async (ev) => {
    ev.stopPropagation();
    const what = item.kind === 'track'
      ? `Delete track "${item.label}"?\n\nRemoves the imported files. `
        + 'Stock tracks are not touched.'
      : `Delete car "${item.name}" from client and server?`;
    if (!confirm(what)) return;
    const r = await api('library/remove',
                        { kind: item.kind, name: item.name });
    toast(r && r.ok ? 'Removed ' + item.name : (r.error || 'Delete failed'),
          !(r && r.ok));
    contentPage();
  };
  act.append(exp, copy, pathB, rm);
  row.append(act);
  return row;
}

async function libraryCard(p) {
  const lib = await api('library');
  const c = el('div', 'card');
  const cars = lib.cars || [];
  const tracks = lib.tracks || [];
  c.innerHTML = `<h2>Installed mods · ${lib.total ?? (cars.length + tracks.length)}</h2>`
    + '<div class="tiny dim" style="margin-bottom:10px">Drag a car zip '
    + '(<code>.kspkg</code> + <code>.json</code>) or a track pack '
    + '(<code>.tar</code> — the same file multiplayer Get content uses) onto '
    + '<b>any</b> page. Export writes that same file to Downloads.</div>';
  const tools = el('div', 'row wrap lib-tabs');
  const pick = el('button', 'primary', 'Install from file');
  const inp = el('input');
  inp.type = 'file';
  inp.multiple = true;
  inp.accept = '.zip,.tar,.kspkg,.json,.tgz';
  inp.style.display = 'none';
  pick.onclick = () => inp.click();
  inp.onchange = async () => {
    const files = [...(inp.files || [])];
    if (!files.length) return;
    toast('Installing ' + files.map(f => f.name).join(', ') + '…');
    const r = await uploadDropped(files);
    ingestToast(r);
    contentPage();
  };
  let filter = 'all';
  const tabAll = el('button', 'sm primary', `All · ${cars.length + tracks.length}`);
  const tabCars = el('button', 'sm', `Cars · ${cars.length}`);
  const tabTr = el('button', 'sm', `Tracks · ${tracks.length}`);
  const list = el('div');
  const setTab = (name) => {
    filter = name;
    tabAll.className = name === 'all' ? 'sm primary' : 'sm';
    tabCars.className = name === 'car' ? 'sm primary' : 'sm';
    tabTr.className = name === 'track' ? 'sm primary' : 'sm';
    list.innerHTML = '';
    const rows = name === 'car' ? cars
      : name === 'track' ? tracks
      : cars.concat(tracks);
    if (!rows.length) {
      list.append(el('div', 'empty',
        name === 'track' ? 'No imported tracks'
        : name === 'car' ? 'No car mods' : 'Nothing installed yet — drop a pack'));
      return;
    }
    rows.forEach(it => list.append(libRow(it, lib.share_url)));
  };
  tabAll.onclick = () => setTab('all');
  tabCars.onclick = () => setTab('car');
  tabTr.onclick = () => setTab('track');
  tools.append(pick, inp, tabAll, tabCars, tabTr);
  c.append(tools, list);
  p.append(c);
  setTab('all');
}

async function contentPage() {
  const p = $('#page');
  p.innerHTML = '';

  // Library first: install, export, delete, copy. Host/share stay below.
  await libraryCard(p);
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
  ic.innerHTML = `<h2>Host a custom track &middot; ${imported.length}</h2>`
    + '<div class="tiny dim" style="margin-bottom:10px">Tracks you already '
    + 'imported (EvoForge or Get content). Deploy writes the logic into the '
    + 'server archive under the track\'s own name — stock tracks stay '
    + 'untouched — and shares it so friends can download it.</div>'
    + `<div class="row wrap" style="margin-bottom:10px">`
    + `<span class="pill ${td.server_running ? 'bad' : 'on'}"><i class="dot"></i>`
    + `server ${td.server_running ? 'RUNNING — stop it first' : 'stopped'}</span>`
    + `<span class="pill ${td.backup ? 'on' : 'off'}"><i class="dot"></i>`
    + `${td.backup ? 'backup ready' : 'no backup yet'}</span></div>`;
  if (!imported.length) {
    ic.append(el('div', 'empty',
      'No imported tracks in Saved Games\\ACE\\mods'));
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
  const rrow = el('div', 'row');
  rrow.style.marginTop = '10px';
  const rest = el('button', 'sm danger', 'Undo last deploy');
  rest.disabled = !td.backup;
  rest.title = 'Put the server archive back the way it was before the last deploy';
  rest.onclick = async () => {
    if (!confirm('Restore the server archive from the backup taken before deploy?'))
      return;
    const r = await api('trackdeploy/restore', {});
    toast(r.ok ? 'Archive restored' : (r.error || 'Restore failed'), !r.ok);
    contentPage();
  };
  rrow.append(rest);
  const redec = el('button', 'sm', 'Redeclare wiped tracks');
  redec.title = 'A Kunos content update replaces content.kspkg wholesale, '
    + 'which erases tracks.table rows for every custom track even though the '
    + "track's own files survive. This re-registers whatever got wiped, "
    + 'without needing EvoForge.';
  redec.onclick = async () => {
    toast('Checking for tracks wiped by the last update…');
    const dry = await api('trackdeploy/redeclare', { dry_run: 1 });
    if (!dry.ok && !dry.candidates) {
      toast(dry.error || 'Could not check tracks.table', true);
      return;
    }
    const n = (dry.candidates || []).length;
    if (n === 0) {
      toast('Nothing to redeclare — every loose track is already registered');
      return;
    }
    const names = dry.candidates.map(c => c.display_name).join(', ');
    if (!confirm(`${n} track(s) missing from tracks.table:\n\n${names}\n\n`
        + 'Re-register them now? This rewrites a 300 MB archive.'))
      return;
    toast('Redeclaring — please wait…');
    const r = await api('trackdeploy/redeclare', {});
    const okN = (r.redeclared || []).length;
    const failN = (r.failed || []).length;
    toast(failN === 0
      ? `Redeclared ${okN} track(s)`
      : `Redeclared ${okN}, ${failN} failed — see log`, failN > 0);
    contentPage();
  };
  rrow.append(redec);
  const redecC = el('button', 'sm', 'Redeclare on client too');
  redecC.title = 'The client keeps its own separate content.kspkg with its '
    + "own tracks.table - fixing the server's copy does not fix single-"
    + 'player Practice/Custom Session, which reads this one instead.';
  redecC.onclick = async () => {
    toast('Checking the client archive for tracks wiped by the last update…');
    const dry = await api('trackdeploy/redeclare_client', { dry_run: 1 });
    if (!dry.ok && !dry.candidates) {
      toast(dry.error || 'Could not check the client tracks.table', true);
      return;
    }
    const n = (dry.candidates || []).length;
    if (n === 0) {
      toast('Nothing to redeclare — every loose track is already registered on the client');
      return;
    }
    if (!confirm(`${n} track(s) missing from the client's tracks.table:\n\n`
        + `${dry.candidates.join(', ')}\n\nRe-register them now?`))
      return;
    toast('Redeclaring on the client — please wait…');
    const r = await api('trackdeploy/redeclare_client', {});
    const okN = (r.redeclared || []).length;
    const failN = (r.failed || []).length;
    toast(failN === 0
      ? `Redeclared ${okN} track(s) on the client`
      : `Redeclared ${okN}, ${failN} failed — see log`, failN > 0);
  };
  rrow.append(redecC);
  ic.append(rrow);
  p.append(ic);

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

  // --- install from a path (folders cannot be uploaded) --------------------
  const inst = el('div', 'card');
  inst.innerHTML = '<h2>Install from a folder or file path</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">If a drop will not '
    + 'work, paste a path. Cars: a folder or zip with '
    + '<code>.kspkg</code> + <code>.json</code>. Tracks: a multiplayer '
    + '<code>.tar</code> or an unpacked track folder.</div>';
  const rowi = el('div', 'row');
  const inp = el('input');
  inp.placeholder = 'C:\\path\\to\\mod.zip   or   track.tar   or   folder';
  const goPath = el('button', 'primary', 'Install');
  rowi.append(inp, goPath);
  inst.append(rowi);
  goPath.onclick = async () => {
    const r = await finishIngest({ path: inp.value });
    ingestToast(r);
    contentPage();
  };
  p.append(inst);

  // --- AI lines / telemetry maps ------------------------------------------
  const sp = await api('splines');
  const sc = el('div', 'card');
  sc.innerHTML = `<h2>AI lines for telemetry &middot; `
    + `${sp.ready_folders ?? 0}/${sp.folders ?? 0} tracks ready</h2>`
    + '<div class="tiny dim" style="margin-bottom:10px">The dedicated server '
    + 'does not ship racing-line files. Stock layouts are inside the game '
    + 'archive; custom tracks (Barber, Highlands) keep theirs in the import '
    + 'folder. Ship copies both next to the server so the live map and vAI '
    + 'work on every hosted track.</div>'
    + `<div class="row wrap" style="margin-bottom:10px">`
    + `<span class="pill ${sp.server ? 'on' : 'off'}"><i class="dot"></i>`
    + `${sp.server || 0} on server</span>`
    + `<span class="pill"><i class="dot"></i>${sp.imported || 0} in imports</span>`
    + `<span class="pill"><i class="dot"></i>${sp.archive || 0} in game archive</span>`
    + `<span class="pill ${sp.missing ? 'warn' : 'on'}"><i class="dot"></i>`
    + `${sp.missing || 0} not copied yet</span></div>`;
  const shipAll = el('button', 'primary',
    sp.missing ? `Copy ${sp.missing} missing line(s) to server` : 'Re-copy AI lines');
  shipAll.onclick = async () => {
    toast('Copying AI lines — first run reads the game archive…');
    const r = await apiLong('splines/ship', { all: 1 });
    toast(r.ok
      ? `Copied ${r.copied_n || 0}, ${r.skipped_n || 0} already there`
      : (r.error || (r.errors || []).join('; ') || 'Ship failed'), !r.ok);
    contentPage();
  };
  sc.append(shipAll);
  if ((sp.missing_imports || []).length) {
    sc.append(el('div', 'tiny dim',
      'Imports still missing on the server: '
      + sp.missing_imports.map(esc).join(', ')));
  }
  p.append(sc);

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
let brTags = {};
let brAcecmOnly = false;
let brHideFull = true;
let brHideLocked = false;
let brHasPlayers = false;
let brRenderT;

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
  // ⚠ This used to be one line of text in the page, which is why a download
  // that was working looked like nothing was happening: no bar, no rate, no
  // idea whether a 4 GB track was moving or stuck. Reuse the same progress
  // component a window-drop already uses, so both ways of getting content
  // look and behave the same.
  const bar = $('#brprog');
  const prog = dropProgress();
  prog.show('Downloading content…');
  const started = Date.now();
  let lastDone = 0, lastAt = Date.now(), rate = 0;
  const poll = setInterval(async () => {
    const st = await api('browser/status');
    const done = st.done || 0, total = st.total || 0;
    // smooth the rate: per-file completions arrive in lumps, and a raw
    // instantaneous figure jumps between 0 and silly numbers
    const now = Date.now();
    if (done > lastDone) {
      const inst = (done - lastDone) / Math.max(0.001, (now - lastAt) / 1000);
      rate = rate ? rate * 0.7 + inst * 0.3 : inst;
      lastDone = done; lastAt = now;
    }
    const left = (rate > 0 && total > done) ? (total - done) / rate : 0;
    const detail = [
      st.detail,
      total ? `${mb(done)} of ${mb(total)}` : null,
      rate > 0 ? `${mb(rate)}/s` : null,
      left > 2 ? `${Math.ceil(left)}s left` : null,
    ].filter(Boolean).join(' · ');
    if (total) prog.set(done / total, detail);
    else prog.busy('Installing…', st.detail || '');
    if (bar) bar.textContent = detail;
    if (st.state === 'done' || st.state === 'error') {
      clearInterval(poll);
      prog.hide();
      const secs = Math.round((Date.now() - started) / 1000);
      toast(st.state === 'done'
        ? `Content installed in ${secs}s — you can join now`
        : st.detail, st.state === 'error');
      brLocal = await api('browser/local');
      browserPage();
    }
  }, 700);
}

function fetchHostCard() {
  const c = el('div', 'card');
  c.innerHTML = '<h2>Fetch from a hosted ACECM</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">If someone is hosting '
    + 'a modded server, they copy a share link from their Content page. Paste '
    + 'it here — you do not need the in-game list for this. '
    + '<code>http://192.168.1.10:8092</code> or just the IP.</div>';
  const row = el('div', 'row wrap');
  const inp = el('input');
  inp.placeholder = 'http://host:8092  or  1.2.3.4';
  inp.style.minWidth = '18em';
  const go = el('button', 'primary', 'Fetch content');
  go.onclick = async () => {
    const host = inp.value.trim();
    if (!host) { toast('Paste the host’s ACECM link first', true); return; }
    await contentFrom({ server_ip: host });
  };
  inp.onkeydown = e => { if (e.key === 'Enter') go.onclick(); };
  row.append(inp, go);
  c.append(row);
  const prog = el('div', 'tiny dim');
  prog.id = 'brprog';
  c.append(prog);
  return c;
}

async function browserPage() {
  const p = $('#page');
  p.innerHTML = '';
  p.append(fetchHostCard());
  const [d, loc] = await Promise.all([api('browser'), api('browser/local')]);
  brLocal = loc || {};

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
  search.oninput = () => {
    brFilter = search.value.toLowerCase();
    clearTimeout(brRenderT);
    brRenderT = setTimeout(render, 120);
  };
  const sort = el('select');
  [['players','Most players'],['ping','Lowest ping'],['name','Name'],
   ['track','Track']].forEach(([v,l]) => {
    const o = el('option', null, l); o.value = v;
    if (v === brSort) o.selected = true; sort.append(o);
  });
  sort.onchange = () => { brSort = sort.value; render(); };
  const only = el('button', brAcecmOnly ? 'sm primary' : 'sm',
                  'ACECM only');
  only.title = 'Servers whose ACECM is reachable on TCP 8092 and is sharing content';
  only.onclick = () => { brAcecmOnly = !brAcecmOnly; browserPage(); };
  const mkTog = (flag, lab) => {
    const b = el('button', flag.get() ? 'sm primary' : 'sm', lab);
    b.onclick = () => { flag.set(!flag.get()); browserPage(); };
    return b;
  };
  const pullList = el('button', 'sm primary', 'Refresh list');
  pullList.title = 'Launch the game, open Multiplayer, save the list, then quit';
  pullList.onclick = async () => {
    const r = await api('drive/capture', {});
    if (!r.ok) { toast(r.error || 'Could not start', true); return; }
    pullList.disabled = true;
    pullList.textContent = r.hint || 'pulling…';
    const tick = setInterval(async () => {
      const s = await api('drive/status');
      const phase = (s && s.phase) || '';
      pullList.textContent = (s && s.hint) || phase || 'pulling…';
      if (phase === 'launched' || phase === 'failed' || phase === 'idle') {
        clearInterval(tick);
        pullList.disabled = false;
        pullList.textContent = 'Refresh list';
        if (s && s.fault) toast(s.fault, true);
        else if (s && s.captured) toast('Captured ' + s.captured + ' servers');
        browserPage();
      }
    }, 1200);
  };
  row.append(search, sort, only,
    mkTog({ get: () => brHideFull, set: v => { brHideFull = v; } }, 'Hide full'),
    mkTog({ get: () => brHideLocked, set: v => { brHideLocked = v; } }, 'Hide password'),
    mkTog({ get: () => brHasPlayers, set: v => { brHasPlayers = v; } }, 'Has players'),
    pullList);
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

  const prog = $('#brprog') || el('div', 'tiny dim');
  prog.id = 'brprog';
  head.append(prog);

  const tblCard = el('div', 'card');
  p.append(tblCard);

  function render() {
    tblCard.innerHTML = '';
    const acecmOf = s => {
      const ip = s.server_ip || '';
      if (brTags[ip] && brTags[ip].hosted) return true;
      return /\[ACECM\]/i.test(String(s.server_name || ''));
    };
    const num = v => (typeof v === 'number' ? v : 0);
    let rows = (d.servers || []).filter(s => {
      if (brAcecmOnly && !acecmOf(s)) return false;
      const npl = num(s.players), nmax = num(s.max_players);
      if (brHideFull && nmax > 0 && npl >= nmax) return false;
      if (brHideLocked && s.driver_password) return false;
      if (brHasPlayers && npl < 1) return false;
      if (!brFilter) return true;
      return [s.server_name, s.track, s.layout, s.event_name]
        .some(x => String(x || '').toLowerCase().includes(brFilter));
    });
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
      tr.dataset.ip = s.server_ip || '';
      const locked = s.driver_password ? ' 🔒' : '';
      const tag = acecmOf(s)
        ? ' <span class="pill acecm"><i class="dot"></i>ACECM</span>' : '';
      tr.innerHTML = `<td>${esc(s.server_name || '(unnamed)')}${locked}${tag}
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

  // Tag after the table is on screen. Probing 8092 on every row up front
  // would stall the page; this fills ACECM pills as replies come back.
  (async () => {
    const ips = [...new Set((d.servers || []).map(s => s.server_ip).filter(Boolean))];
    for (let i = 0; i < Math.min(ips.length, 120); i += 24) {
      const chunk = ips.slice(i, i + 24).filter(ip => !brTags[ip] || brTags[ip].pending);
      if (!chunk.length) continue;
      chunk.forEach(ip => { if (!brTags[ip]) brTags[ip] = { pending: true }; });
      try {
        const qs = chunk.map(ip => 'ip=' + encodeURIComponent(ip)).join('&');
        const r = await fetch('/api/browser/tag?' + qs).then(x => x.json());
        Object.assign(brTags, (r && r.hosts) || {});
      } catch (e) {}
      document.querySelectorAll('#page tr[data-ip]').forEach(tr => {
        const info = brTags[tr.dataset.ip];
        if (!(info && info.hosted)) return;
        const cell = tr.querySelector('td');
        if (cell && !cell.querySelector('.pill.acecm')) {
          const span = document.createElement('span');
          span.className = 'pill acecm';
          span.innerHTML = '<i class="dot"></i>ACECM';
          const dim = cell.querySelector('.tiny.dim');
          if (dim) dim.before(span);
          else cell.append(span);
        }
      });
    }
  })();
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
  drive: ['Drive', 'Single player or a local server — same car picker', drivePage],
  servers: ['Servers', 'Create, configure and run dedicated servers', serversPage],
  cars: ['Cars', 'What the dedicated server can actually load', carsPage],
  content: ['Content', 'Install, export and manage cars and tracks', contentPage],
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
// the page the user last asked for, so a late render cannot overwrite it
let _wanted = '';

const _queued = {};
function keepPlace(name, fn) {
  const wrapped = async function (...args) {
    // ⚠ QUEUE, do not drop. go() has already swapped the title, moved the
    // nav highlight and blanked #page to "Loading..." by the time we get here,
    // so returning early left the header saying one page while the body showed
    // the previous one - or nothing at all until you clicked away and back.
    // Drive polls every 1200ms, so it almost always had a render in flight for
    // a click to collide with, which is why it was the one that "broke".
    if (_rendering[name]) { _queued[name] = args; return; }
    _rendering[name] = true;
    // a self-refresh should come back to where the user was, a fresh
    // navigation should start at the top
    const want = _page === name ? (_scrollPos[name] || 0) : 0;
    try {
      return await fn.apply(this, args);
    } finally {
      _rendering[name] = false;
      // ⚠ NO `return` anywhere in this finally block. A return here swallows
      // an exception thrown by the page function, and go() depends on
      // catching that to report the error instead of leaving a blank screen -
      // silently eaten errors are what made several of these bugs so hard to
      // find in the first place.
      const requeue = _queued[name];
      if (requeue) {
        // a click that landed mid-render runs now, rather than being lost
        delete _queued[name];
        setTimeout(() => wrapped(...requeue), 0);
      } else if (_wanted && _wanted !== name) {
        // ⚠ A page function clears #page, awaits, then appends. If the user
        // navigated away during that await, the OLD page finishes by painting
        // itself over the NEW one: you click Dashboard, a slow Cars render
        // lands a moment later, and the dashboard you asked for is gone. The
        // header and nav still say Dashboard, so it reads as a blank page
        // rather than as the wrong page. Repaint what was actually asked for.
        const spec = PAGES[_wanted];
        if (spec) { const again = spec[2]; setTimeout(() => again(), 0); }
      } else {
        _page = name;
        requestAnimationFrame(() => {
          if (want) scroller().scrollTop = want;
        });
      }
    }
  };
  return wrapped;
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
  if (name !== 'drive') stopDrivePoll();
  _wanted = PAGES[name] ? name : 'drive';
  const [title, sub, fn] = PAGES[name] || PAGES.drive;
  $('#ttl').textContent = title;
  $('#sub').textContent = sub;
  document.querySelectorAll('nav a').forEach(a =>
    a.classList.toggle('on', a.dataset.page === name));
  // ⚠ Only blank when the page is actually changing. Blanking on a refresh is
  // what makes the content flick away and come back.
  if (name !== _page) $('#page').innerHTML = '<div class="empty">Loading…</div>';
  fn().then(() => { const p = $('#page'); if (p) p.dataset.booted = '1'; })
    .catch(e => { $('#page').innerHTML = ''; toast(String(e), true); });
  location.hash = name;
  refreshAttention();
}
/* The section links are built from PAGES rather than written out in the HTML,
   so adding a page cannot leave the nav out of step with it. The everyday
   sections read as words next to the title; the occasional ones sit small on
   the right so the main row stays short enough to scan. */
const PRIMARY = ['drive', 'servers', 'cars', 'tracks', 'content',
                 'backend', 'browser', 'telemetry'];
function buildSections() {
  const main = $('#sections'), side = $('#sections2');
  main.innerHTML = ''; side.innerHTML = '';
  Object.keys(PAGES).forEach(name => {
    const a = el('a', null, PAGES[name][0]);
    a.dataset.page = name;
    a.onclick = () => go(name);
    (PRIMARY.includes(name) ? main : side).append(a);
  });
}
buildSections();
go((location.hash || '#drive').slice(1));
// problems are worth noticing wherever you are, but they do not change
// often - a slow tick is plenty and costs one small request.
setInterval(refreshAttention, 20000);
bindDropAnywhere();
api('state').then(s => {
  $('#navfoot').textContent = s.server_exe_ok ? 'server ready' : 'server not found';
});
