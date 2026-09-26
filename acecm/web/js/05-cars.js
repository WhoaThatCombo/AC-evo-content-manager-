/* ACECM UI - cars. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
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
    liveWatch(s => s.thumbs, j => j.state === 'running', j => {
      build.textContent = 'Render missing';
      toast(`${j.made} car render(s) made`);
      if (_page === 'cars') carsPage();
    }, j => {
      build.textContent = `Rendering ${j.done}/${j.total} — ${j.current}`;
    });
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
        if (!await ask(`Remove "${n}" from both sides?`)) return;
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
  liveWatch(s => s.thumbs, j => j.state === 'running', j => {
    // repaint so the new pictures actually appear, but only if the user is
    // still looking at this page
    if (j.made) {
      toast(`${j.made} car picture${j.made === 1 ? '' : 's'} rendered`);
      if (_page === 'cars') carsPage();
    }
  });
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
