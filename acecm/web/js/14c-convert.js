/* ACECM UI - Convert. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js).

   Pick an Assetto Corsa (AC1) car or track from a picture grid, press
   Convert, and it lands in the game's mods folder ready to try. The
   converter (aceconv, bundled) runs in the background at low priority; its
   own checks run on the result before anything is installed. The technical
   bits (paths, the EVO id, the converter's output) are folded away until
   wanted. */
/* ------------------------------------------------------------- convert --- */
let cvTimer = null, cvPick = '', cvFilter = '', cvShow = 'all', cvSetupOpen = null, cvMode = 'car';
const cvPicks = { car: '', track: '' };

function cvCss() {
  if (document.getElementById('cv-css')) return;
  const s = document.createElement('style');
  s.id = 'cv-css';
  s.textContent = `
  .cv-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(190px,1fr));gap:10px;max-height:46vh;overflow:auto;padding:2px}
  .cv-card{border:1px solid var(--line);border-radius:8px;overflow:hidden;cursor:pointer;background:rgba(255,255,255,.02);transition:border-color .12s}
  .cv-card:hover{border-color:var(--dim)}
  .cv-card.on{border-color:var(--accent);box-shadow:0 0 0 1px var(--accent)}
  .cv-card.locked{opacity:.55;cursor:not-allowed}
  .cv-grid.tracks .cv-card.locked{cursor:pointer}
  .cv-img{aspect-ratio:16/9;background:#0006 center/cover no-repeat;position:relative;overflow:hidden}
  .cv-img > img{position:absolute;inset:0;width:100%;height:100%;object-fit:cover}
  .cv-tag{z-index:1}
  .cv-tag{position:absolute;top:6px;left:6px;font-size:10.5px;padding:2px 6px;border-radius:4px;background:#000a}
  .cv-tag.ok{color:var(--accent)} .cv-tag.bad{color:var(--red)}
  .cv-meta{padding:7px 9px}
  .cv-meta b{display:block;font-size:12.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
  .cv-meta span{font-size:11px;color:var(--dim)}
  .cv-chip{border:1px solid var(--line);border-radius:14px;padding:3px 10px;font-size:11.5px;cursor:pointer;background:none;color:inherit}
  .cv-chip.on{border-color:var(--accent);color:var(--accent)}
  .cv-hero{display:flex;gap:16px;align-items:center}
  .cv-hero .cv-img{width:240px;flex:none;border-radius:8px}
  .cv-bar{height:10px;border-radius:5px;background:var(--line);overflow:hidden;margin:10px 0 6px}
  .cv-bar > div{height:100%;width:0;background:var(--accent);transition:width .5s}
  .cv-big{font-size:14px;padding:9px 22px}
  details.cv-adv summary{cursor:pointer;color:var(--dim);font-size:12px;margin-top:4px}
  .cv-banner{padding:10px 12px;border-radius:8px;border:1px solid var(--red);margin-bottom:10px}
  .cv-note{padding:8px 12px;border-radius:8px;border:1px solid var(--line);margin:8px 0;font-size:12px}
  .cv-note label{display:block;margin-top:4px}
  .cv-check{margin-top:8px;font-size:12px}
  .cv-check .bad{color:var(--red)} .cv-check .ok{color:var(--accent)}
  .cv-check ul{margin:4px 0 0 18px;padding:0;max-height:9em;overflow:auto}
  .cv-tdet{margin-top:14px;display:grid;grid-template-columns:minmax(0,1fr) 220px;gap:14px}
  .cv-tdet .cv-map{width:220px;aspect-ratio:1;border-radius:8px;background:#0006 center/contain no-repeat;border:1px solid var(--line)}
  .cv-facts{display:grid;grid-template-columns:110px 1fr;gap:3px 10px;font-size:12px}
  .cv-facts > :nth-child(odd){color:var(--dim)}
  .cv-desc{font-size:12px;color:var(--dim);margin-top:8px;max-height:6.5em;overflow:auto;white-space:pre-wrap}
  .cv-files{margin-top:10px;border:1px solid var(--line);border-radius:8px;max-height:180px;overflow:auto}
  .cv-files div{display:flex;gap:8px;align-items:center;padding:4px 8px;font-size:11.5px;border-bottom:1px solid var(--line)}
  .cv-files div:last-child{border-bottom:none}
  .cv-files .n{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
  .cv-files .lock{color:var(--red)} .cv-files .open{color:var(--accent)}
  .cv-src{display:flex;gap:6px;align-items:center;margin-top:10px}
  .cv-src input{flex:1}
  @media (max-width:760px){.cv-tdet{grid-template-columns:1fr}}
  `;
  document.head.append(s);
}

function cvAgo(t) {
  if (!t) return '';
  const s = Date.now() / 1000 - t;
  if (s < 90) return 'just now';
  if (s < 5400) return Math.round(s / 60) + ' min ago';
  if (s < 129600) return Math.round(s / 3600) + ' h ago';
  return Math.round(s / 86400) + ' days ago';
}

/* The converter's check (check-car / check-track), in words. */
function cvCheckBox(c) {
  const box = el('div', 'cv-check');
  if (!c) return box;
  const lines = c.lines || [];
  const errs = lines.filter(l => /^\s+ERROR /.test(l)).map(l => l.replace(/^\s+ERROR /, ''));
  const warns = lines.filter(l => /^\s+warn /.test(l)).map(l => l.replace(/^\s+warn\s+/, ''));
  box.innerHTML = 'Check: <b class="' + (c.errors ? 'bad' : 'ok') + '">' + c.errors + ' error' + (c.errors === 1 ? '' : 's') + '</b>, '
    + c.warnings + ' warning' + (c.warnings === 1 ? '' : 's');
  const items = errs.map(e => '<li class="bad">' + esc(e) + '</li>').concat(warns.map(w => '<li>' + esc(w) + '</li>'));
  if (items.length) box.append(el('ul', '', items.join('')));
  return box;
}

async function convertPage() {
  cvCss();
  const p = $('#page');
  p.innerHTML = '';
  if (cvTimer) { clearInterval(cvTimer); cvTimer = null; }
  const root = el('div');
  root.id = 'cv-root';
  p.append(root);

  const T = cvMode === 'track';
  const KIND = T ? 'tracks' : 'cars';
  cvPick = cvPicks[cvMode];
  const [info, raw] = await Promise.all([api('convert'), api(T ? 'convert/tracks' : 'convert/cars')]);
  const cars = { ok: raw.ok, error: raw.error, cars: T ? (raw.tracks || []) : (raw.cars || []) };
  const noun = T ? 'track' : 'car';
  const st = info.settings || {}, have = info.present || {};
  const missing = Object.entries({ ac1_root: 'Assetto Corsa (the first game)', aceconv: 'the converter',
    game_content: 'Assetto Corsa EVO' }).filter(([k]) => !have[k]).map(([, v]) => v);
  if (cvSetupOpen === null) cvSetupOpen = missing.length > 0;

  // ---- problems first, in words
  if (missing.length) {
    root.append(el('div', 'cv-banner', '<b>Not ready yet.</b> Can\'t find ' + esc(missing.join(', '))
      + '. Fix the paths under <b>Setup</b> below' + (have.game_content ? '' : ' (the EVO game folder is in Settings)') + '.'));
  }
  if (info.game_running) {
    root.append(el('div', 'cv-banner', T
      ? 'The game is running. Close it before installing a track - installing changes content.kspkg, which the game keeps open.'
      : 'The game is running. You can convert, but the car only shows up after a game restart, '
        + 'a car the game has open can\'t be replaced, and its saved cameras are only reset with the game closed.'));
  }

  // ---- 1. pick a car
  const modes = el('div', 'row');
  modes.style.marginBottom = '10px';
  for (const [k, label] of [['car', 'Cars'], ['track', 'Tracks']]) {
    const b = el('button', 'cv-chip' + (cvMode === k ? ' on' : ''), label);
    b.style.fontSize = '13px';
    b.style.padding = '5px 16px';
    b.onclick = () => { if (cvMode !== k) { cvPicks[cvMode] = cvPick; cvMode = k; cvFilter = ''; cvShow = 'all'; convertPage(); } };
    modes.append(b);
  }
  root.append(modes);
  const pc = el('div', 'card');
  pc.innerHTML = '<h2>1 · Pick a ' + noun + '</h2>';
  const bar = el('div', 'row wrap');
  const filt = el('input');
  filt.placeholder = 'Search by name, brand or folder…';
  filt.value = cvFilter;
  filt.style.width = '300px';
  const chips = el('div', 'row');
  const chip = (k, label) => { const b = el('button', 'cv-chip' + (cvShow === k ? ' on' : ''), label);
    b.onclick = () => { cvShow = k; chips.querySelectorAll('.cv-chip').forEach(x => x.classList.remove('on')); b.classList.add('on'); paint(); }; return b; };
  chips.append(chip('all', 'All'), chip('new', 'Not converted'), chip('done', 'Converted'));
  const count = el('span', 'tiny dim grow');
  count.style.textAlign = 'right';
  bar.append(filt, chips, count);
  const grid = el('div', 'cv-grid' + (T ? ' tracks' : ''));
  grid.style.marginTop = '10px';
  pc.append(bar, grid);
  root.append(pc);

  // ---- 2. convert
  const cc = el('div', 'card');
  cc.innerHTML = '<h2>2 · Convert</h2>';
  const hero = el('div', 'cv-hero');
  const himg = el('div', 'cv-img');
  const hside = el('div', 'grow');
  const htitle = el('div', '', '<span class="dim">Pick a ' + noun + ' above.</span>');
  htitle.style.fontSize = '15px';
  const hsub = el('div', 'tiny dim');
  const opts = el('div', 'row wrap');
  opts.style.margin = '10px 0';
  const cbC = el('label', 'tiny', '<input type="checkbox" checked> Put it in the game');
  const cbS = el('label', 'tiny', '<input type="checkbox"> Also in my dedicated server');
  if (T) cbS.hidden = true;
  opts.append(cbC, cbS);
  // earlier conversions of the same source (the converter's legacy-id)
  const prevBox = el('div');
  const actions = el('div', 'row wrap');
  const go_ = el('button', 'primary cv-big', 'Convert');
  const stop = el('button', 'sm', 'Stop');
  stop.hidden = true;
  const uninst = el('button', 'sm', 'Remove from the game');
  uninst.hidden = true;
  // a track converted without installing: install that build, no reconvert
  const instNow = el('button', 'sm', 'Put this build in the game');
  instNow.hidden = true;
  actions.append(go_, stop, uninst, instNow);
  const adv = el('details', 'cv-adv');
  adv.innerHTML = '<summary>Advanced</summary>';
  const idRow = el('div', 'row wrap');
  idRow.style.marginTop = '6px';
  const idIn = el('input');
  idIn.spellcheck = false;
  idIn.style.width = '260px';
  idRow.append(el('span', 'tiny dim', 'EVO ' + noun + ' id'), idIn,
    el('span', 'tiny dim', T ? '3-16 lower-case letters, digits and _. Converting again with the same id replaces the old one.'
      : 'ks_ + lower-case letters, digits and _, 13-37 characters. Converting again with the same id replaces the old one.'));
  adv.append(idRow);
  hside.append(htitle, hsub, opts, prevBox, actions, adv);
  hero.append(himg, hside);
  // progress
  const prog = el('div');
  prog.hidden = true;
  const pbar = el('div', 'cv-bar', '<div></div>');
  const pline = el('div', 'row');
  const pword = el('span', 'grow');
  const ptime = el('span', 'tiny dim');
  pline.append(pword, ptime);
  const pcheck = el('div');
  const det = el('details', 'cv-adv');
  det.innerHTML = '<summary>Details</summary>';
  const pre = el('pre', 'log');
  pre.style.maxHeight = '30vh';
  det.append(pre);
  prog.append(pbar, pline, pcheck, det);
  // track details: map, facts, layouts, the model files and where they are
  const tdet = el('div', 'cv-tdet');
  tdet.hidden = true;
  let tLayout = '', tSource = '', tBlocked = false, tReq = 0, pReq = 0;
  let lastId = '';
  let replaceIds = [];
  cc.append(hero, tdet, prog);
  root.append(cc);

  // ---- setup (folded unless something is missing)
  const sc = el('details', 'card');
  sc.open = cvSetupOpen;
  sc.ontoggle = () => { cvSetupOpen = sc.open; };
  sc.innerHTML = '<summary style="cursor:pointer"><b style="font-size:12px;letter-spacing:.09em;text-transform:uppercase;color:var(--dim)">Setup</b>'
    + ' <span class="tiny dim">' + (missing.length ? 'needs attention' : 'everything found') + '</span></summary>';
  const fields = [
    ['ac1_root', 'Assetto Corsa folder', 'Where Assetto Corsa (the first game) is installed', have.ac1_root],
    ['out_dir', 'Work folder', 'Where converted cars and tracks are built (several hundred MB each). The converter also keeps what it builds once from your EVO install here (_aceconv).', have.out_dir],
    ['threads', 'Threads', '0 = automatic: half of your cores, fewer when memory is short', true],
    ['memory_gb', 'Memory limit (GB)', '0 = automatic: what is free, less 2 GB for everything else', true],
  ];
  const inputs = {};
  const sg = el('div');
  sg.style.cssText = 'display:grid;grid-template-columns:170px 1fr 22px;gap:6px 10px;align-items:center;margin:10px 0';
  for (const [k, label, hint, ok] of fields) {
    const i = el('input');
    i.value = st[k] === undefined ? '' : st[k];
    i.spellcheck = false;
    i.title = hint;
    inputs[k] = i;
    const mark = el('div', '', ok ? '✓' : '✗');
    mark.style.color = ok ? 'var(--accent)' : 'var(--red)';
    sg.append(el('div', 'tiny', esc(label)), i, mark);
  }
  const row3 = (label, text, ok) => {
    const mark = el('div', '', ok ? '✓' : '✗');
    mark.style.color = ok ? 'var(--accent)' : 'var(--red)';
    sg.append(el('div', 'tiny', esc(label)), el('div', 'tiny dim', text), mark);
  };
  row3('EVO game', esc(info.game_dir || 'not found'), have.game_content);
  row3('Converter', esc(have.aceconv ? ('aceconv' + (info.converter_rev ? ' ' + info.converter_rev : '') + ' · ' + (info.threads_why || '')) : 'missing from this build'), have.aceconv);
  row3('From your game', have.donor ? 'the donor car and templates are built'
    : 'built on the first conversion from your EVO install (a minute or two, once)', true);
  const srow = el('div', 'row wrap');
  const save = el('button', 'sm', 'Save');
  save.onclick = async () => {
    const body = {};
    for (const k in inputs) body[k] = inputs[k].value;
    const r = await api('convert/settings', body);
    if (r.ok) { toast('Saved'); convertPage(); }
  };
  srow.append(save, el('span', 'tiny dim', 'Cars go to ' + esc(info.client_mods || '?')
    + ' · server: ' + esc(info.server_mods || '?')));
  sc.append(sg, srow);
  root.append(sc);

  // ---- the grid
  const all = cars.cars || [];
  const srcOf = (c, small) => '/api/convert/' + (T ? 'track_preview' : 'preview') + '?car=' + encodeURIComponent(c.folder) + (small ? '&thumb=1' : '');
  const imgOf = c => 'url("' + srcOf(c, false) + '")';
  function tagOf(c) {
    if (c.protected) return '<span class="cv-tag bad">Protected</span>';
    const l = c.last;
    if (!l) return '';
    if (!l.ok) return '<span class="cv-tag bad">Failed ' + esc(cvAgo(l.at)) + '</span>';
    return '<span class="cv-tag ok">' + ((l.installed || []).length ? 'In the game' : 'Converted') + '</span>';
  }
  function line2(c) {
    if (T) {
      const lay = (c.layouts || []).length > 1 ? (c.layouts.length + ' layouts') : '';
      return [c.country, c.length, c.pitboxes ? c.pitboxes + ' boxes' : '', lay].filter(Boolean).map(esc).join(' · ') || esc(c.folder);
    }
    return [c.year, c.power, c.class].filter(Boolean).map(esc).join(' · ') || esc(c.folder);
  }
  function paint() {
    const q = filt.value.trim().toLowerCase();
    cvFilter = filt.value;
    grid.innerHTML = '';
    const shown = all.filter(c => {
      if (cvShow === 'new' && c.last && c.last.ok) return false;
      if (cvShow === 'done' && !(c.last && c.last.ok)) return false;
      return !q || [c.folder, c.name, c.brand, c.year, c.class].join(' ').toLowerCase().includes(q);
    });
    count.textContent = cars.ok ? (shown.length === all.length ? all.length + ' ' + noun + 's' : shown.length + ' of ' + all.length + ' ' + noun + 's') : (cars.error || '');
    for (const c of shown.slice(0, 300)) {
      const card = el('div', 'cv-card' + (c.folder === cvPick ? ' on' : '') + (c.protected ? ' locked' : ''));
      card.title = c.protected ? (T ? 'Protected by its author - click to see its files' : 'Protected by its author - it can\'t be converted') : c.folder;
      card.dataset.folder = c.folder;
      // small cached thumbnails, loaded only when scrolled into view
      const im = el('div', 'cv-img', tagOf(c));
      const pic = document.createElement('img');
      pic.loading = 'lazy';
      pic.decoding = 'async';
      pic.alt = '';
      pic.src = srcOf(c, true);
      pic.onerror = () => pic.remove();
      im.prepend(pic);
      card.append(im, el('div', 'cv-meta', '<b>' + esc(c.name || c.folder) + '</b><span>'
        + (T ? '' : esc(c.brand || '') + (c.brand ? ' · ' : '')) + line2(c) + '</span>'));
      // a protected track can still be looked at (and pointed at another copy)
      card.onclick = () => { if (T || !c.protected) pick(c); };
      card.ondblclick = () => { if (!c.protected) { pick(c); go_.click(); } };
      grid.append(card);
    }
    if (shown.length > 300) grid.append(el('div', 'tiny dim', 'Showing 300 - search to narrow it down.'));
    if (!shown.length) grid.append(el('div', 'tiny dim', 'No ' + noun + 's match.'));
  }
  /* Earlier conversions of this source under another id: offer to replace
     them (a name-only match is offered unticked - it may be someone else's
     mod of the same car). */
  function showEarlier(list, newId) {
    prevBox.innerHTML = '';
    replaceIds = [];
    const other = (list || []).filter(e => e.id !== newId);
    if (!other.length) return;
    const box = el('div', 'cv-note', '<b>Converted before</b> under another id. Replacing takes the old one out of the game'
      + (T ? ' (its rows and folder)' : ' (kept in the work folder\'s _replaced) and resets its saved cameras') + '.');
    for (const e of other) {
      const lab = el('label', 'tiny');
      const cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.checked = e.strength >= 2;
      const sync = () => { replaceIds = [...prevBox.querySelectorAll('input:checked')].map(x => x.dataset.id); };
      cb.dataset.id = e.id;
      cb.onchange = sync;
      lab.append(cb, document.createTextNode(' Replace ' + e.id + ' - ' + (e.strength >= 2 ? 'same source folder' : 'same name only') + ' (' + e.why + ')'));
      box.append(lab);
    }
    prevBox.append(box);
    replaceIds = other.filter(e => e.strength >= 2).map(e => e.id);
  }
  async function pick(c) {
    cvPick = c.folder;
    cvPicks[cvMode] = c.folder;
    idIn.value = c.id || '';
    himg.style.backgroundImage = imgOf(c);
    htitle.innerHTML = '<b>' + esc(c.name || c.folder) + '</b>';
    const l = c.last;
    hsub.innerHTML = esc((T ? [c.country, c.length] : [c.brand, c.year, c.power]).filter(Boolean).join(' · '))
      + (l && l.ok ? '<br>Last converted ' + esc(cvAgo(l.at)) + ' in ' + l.secs + ' s'
        + (l.check ? ' · check: ' + l.check.errors + ' errors, ' + l.check.warnings + ' warnings' : '') : '');
    go_.textContent = l && l.ok ? 'Convert again' : 'Convert';
    grid.querySelectorAll('.cv-card').forEach(x => x.classList.toggle('on', x.dataset.folder === c.folder));
    lastId = c.id || '';
    uninst.hidden = !(l && (l.installed || []).length);
    instNow.hidden = !(T && l && l.ok && !(l.installed || []).length);
    prevBox.innerHTML = '';
    replaceIds = [];
    if (T) {
      if (c.folder !== tdet.dataset.folder) { tLayout = ''; tSource = ''; }
      tdet.dataset.folder = c.folder;
      trackDetails(c);
    } else {
      const req = ++pReq;
      const e = await api('convert/earlier?kind=cars&folder=' + encodeURIComponent(c.folder));
      if (req === pReq && cvPick === c.folder && e && e.ok) showEarlier(e.earlier, c.id);
    }
  }
  const mb = n => n >= 1048576 ? (n / 1048576).toFixed(0) + ' MB' : (n / 1024).toFixed(0) + ' KB';
  async function trackDetails(c) {
    const req = ++tReq;
    tBlocked = !!c.protected;
    go_.disabled = tBlocked;
    const q = 'convert/track_info?track=' + encodeURIComponent(c.folder) + (tSource ? '&source=' + encodeURIComponent(tSource) : '');
    const d = await api(q);
    if (req !== tReq || cvPick !== c.folder) return;
    tdet.hidden = false;
    tdet.innerHTML = '';
    const left = el('div'), right = el('div');
    if (!d.ok) {
      left.append(el('div', 'cv-banner', esc(d.error || 'Could not read the track')));
      if (tSource) {
        const back = el('button', 'sm', 'Use the installed track instead');
        back.onclick = () => { tSource = ''; trackDetails(c); };
        left.append(back);
      }
      tdet.append(left, right);
      tBlocked = true;
      go_.disabled = true;
      return;
    }
    showEarlier(d.earlier, c.id);
    const lays = d.layouts || [];
    if (!lays.some(x => x.layout === tLayout)) tLayout = (lays[0] || {}).layout || '';
    const cur = lays.find(x => x.layout === tLayout) || lays[0] || { ui: {} };
    const u = cur.ui || {};
    // layout picker (the map and facts follow it; every layout is converted)
    if (lays.length > 1) {
      const row = el('div', 'row wrap');
      row.style.marginBottom = '8px';
      row.append(el('span', 'tiny dim', 'Layouts (all are converted)'));
      for (const x of lays) {
        const b = el('button', 'cv-chip' + (x.layout === tLayout ? ' on' : ''), esc((x.ui || {}).name || x.layout));
        b.title = x.layout;
        b.onclick = () => { tLayout = x.layout; trackDetails(c); };
        row.append(b);
      }
      left.append(row);
    }
    const imgQ = '?car=' + encodeURIComponent(c.folder) + (tLayout ? '&layout=' + encodeURIComponent(tLayout) : '');
    himg.style.backgroundImage = 'url("/api/convert/track_preview' + imgQ + '")';
    if (u.name) htitle.innerHTML = '<b>' + esc(u.name) + '</b>';
    const facts = el('div', 'cv-facts');
    const fact = (k, v) => { if (v) facts.append(el('div', '', esc(k)), el('div', '', esc(v))); };
    fact('Author', u.author);
    fact('Where', [u.city, u.country].filter(Boolean).join(', '));
    fact('Length', u.length);
    fact('Width', u.width);
    fact('Pit boxes', u.pitboxes);
    fact('Direction', u.run);
    fact('Version', u.version);
    fact('Tags', u.tags);
    fact('Layouts', lays.length > 1 ? lays.length + '' : '');
    fact('Model files', (d.files || []).length + ' · ' + mb(d.kn5_bytes || 0));
    fact('CSP config', d.csp_config ? 'yes (lights, materials)' : '');
    fact('Skin', d.skin_used ? d.skin_used + ' (fills in placeholder textures)' : '');
    fact('Source', d.dir);
    left.append(facts);
    if (u.description) left.append(el('div', 'cv-desc', esc(String(u.description).replace(/<br\s*\/?>/gi, '\n').replace(/<[^>]+>/g, ''))));
    // protection, in words
    const locked = d.protected_files || 0;
    tBlocked = locked > 0;
    if (locked) {
      left.append(el('div', 'cv-banner', '<b>Protected by its author.</b> ' + locked + ' of ' + d.files.length
        + ' model files are encrypted (CSP), so this copy can\'t be converted. If the author gives you an unencrypted copy, '
        + 'paste its folder under <b>Convert from</b> - or swap the files in the track folder and pick the track again.'));
    }
    // the model files, each one click from Explorer
    const fl = el('div', 'cv-files');
    for (const f of d.files || []) {
      const r = el('div');
      r.append(el('span', 'n', esc(f.name)), el('span', 'tiny dim', mb(f.size)),
        el('span', f.protected ? 'lock' : 'tiny dim', f.protected ? 'encrypted' : 'ok'));
      const o = el('button', 'sm open', 'Show');
      o.title = 'Show this file in Explorer';
      o.onclick = () => api('convert/track_open', { track: c.folder, file: f.name, source: tSource });
      r.append(o);
      fl.append(r);
    }
    left.append(fl);
    // where to convert from
    const src = el('div', 'cv-src');
    const sIn = el('input');
    sIn.placeholder = 'Convert from another copy: paste its folder (empty = the installed track)';
    sIn.value = tSource;
    sIn.spellcheck = false;
    const sUse = el('button', 'sm', 'Use');
    sUse.onclick = () => { tSource = sIn.value.trim().replace(/^"|"$/g, ''); trackDetails(c); };
    sIn.onkeydown = e => { if (e.key === 'Enter') sUse.click(); };
    const sOpen = el('button', 'sm', 'Open folder');
    sOpen.title = 'Open the track folder in Explorer';
    sOpen.onclick = () => api('convert/track_open', { track: c.folder, source: tSource });
    const sRe = el('button', 'sm', 'Re-check');
    sRe.title = 'Read the files again (after you changed them)';
    sRe.onclick = () => trackDetails(c);
    src.append(el('span', 'tiny dim', 'Convert from'), sIn, sUse, sOpen, sRe);
    left.append(src);
    // the track map
    const map = el('div', 'cv-map');
    if (cur.outline) map.style.backgroundImage = 'url("/api/convert/track_preview' + imgQ + '&outline=1")';
    else map.innerHTML = '<div class="tiny dim" style="padding:10px">No track map</div>';
    right.append(map);
    tdet.append(left, right);
    go_.disabled = tBlocked;
    go_.title = tBlocked ? 'This copy is encrypted by its author' : '';
  }
  let filtT = null;
  filt.oninput = () => { clearTimeout(filtT); filtT = setTimeout(paint, 150); };
  paint();
  const again = all.find(c => c.folder === cvPick);
  if (again) pick(again);

  // ---- actions
  go_.onclick = async () => {
    if (!cvPick) { toast('Pick a ' + noun + ' first', true); return; }
    const r = T
      ? await api('convert/track_start', { track: cvPick, id: idIn.value.trim(), install: cbC.querySelector('input').checked,
          source: tSource, replace: replaceIds })
      : await api('convert/start', { car: cvPick, id: idIn.value.trim(), replace: replaceIds,
          install_client: cbC.querySelector('input').checked, install_server: cbS.querySelector('input').checked });
    if (r.ok) poll(); else toast(r.error || 'could not start', true);
  };
  stop.onclick = () => api('convert/cancel', {});
  instNow.onclick = async () => {
    if (!lastId || !await ask('Put ' + lastId + ' in the game?\n\nThis adds it to the game\'s track list (content.kspkg, backed up first). Assetto Corsa EVO must be closed.', 'Install')) return;
    instNow.disabled = true;
    const r = await api('convert/track_install', { id: lastId, replace: replaceIds });
    instNow.disabled = false;
    if (r.ok) { toast('Installed ' + lastId); convertPage(); } else toast(r.error || 'could not install', true);
  };
  uninst.onclick = async () => {
    if (!lastId || !await ask('Take ' + lastId + ' out of the game' + (T ? '' : ' (and your server)') + '?\n\nThe converted files in the work folder are kept.', 'Remove')) return;
    const r = await api(T ? 'convert/track_uninstall' : 'convert/uninstall', { id: lastId });
    if (r.ok) { toast('Removed'); convertPage(); } else toast(r.error || 'could not remove', true);
  };

  // no stage output from the converter: progress is by time, against the
  // last run of this source (or a typical one)
  const est = () => { const c = all.find(x => x.folder === cvPick); return (c && c.last && c.last.ok && c.last.secs) || (T ? 600 : 90); };
  let wasRunning = false;
  async function poll() {
    if (!document.getElementById('cv-root')) { clearInterval(cvTimer); cvTimer = null; return; }
    const s = await api('convert/status');
    if (!s.ok) return;
    const running = s.state === 'running';
    const lines = s.lines || [];
    go_.disabled = running || (T && tBlocked);
    stop.hidden = !running;
    const atBottom = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 40;
    pre.textContent = lines.join(String.fromCharCode(10));
    if (atBottom) pre.scrollTop = pre.scrollHeight;
    const res = s.result || {};
    // the last job belongs to the other tab (a track while showing cars):
    // its result is not this page's
    const mine = String(s.car || '').startsWith('track:') === T;
    prog.hidden = s.state === 'idle' || (!running && !mine);
    if (!running && !mine) { wasRunning = running; return; }
    const fill = pbar.firstChild;
    pcheck.innerHTML = '';
    if (running) {
      const prep = lines.some(l => l.includes('building the donor car')) && !lines.some(l => l.startsWith('note:'));
      const snd = lines.some(l => l.includes('building the FMOD template')) && !lines.some(l => l.startsWith('note:'));
      const checking = lines.some(l => l.startsWith('ACECM: checking'));
      fill.style.width = (checking ? 98 : Math.min(95, s.secs / est() * 100)).toFixed(0) + '%';
      fill.style.background = 'var(--accent)';
      pword.innerHTML = esc(checking ? 'Checking the result' : snd ? 'Preparing the sound template from your game (first time only)'
        : prep ? 'Preparing the donor car from your game (first time only)' : 'Converting the ' + noun) + '…';
      ptime.textContent = s.secs.toFixed(0) + ' s';
      uninst.hidden = true;
    } else if (s.state === 'done') {
      fill.style.width = '100%';
      const inst = [...new Set((res.installed || []).map(x => x.to || 'game'))];
      const nm = (all.find(x => x.folder === String(s.car).replace(/^track:/, '')) || {}).name || s.car;
      const warn = (res.warnings || []).slice();
      const sizeTxt = T ? (((res.installed || [])[0] || {}).files || 0) + ' files' : ((res.size || 0) / 1048576).toFixed(0) + ' MB';
      const rep = (res.replaced || []).length ? ' Replaced ' + res.replaced.length + ' earlier file' + (res.replaced.length === 1 ? '' : 's') + '.' : '';
      const cams = res.settings_reset && (res.settings_reset.removed || []).length ? ' Saved cameras / mirrors of the old version reset.' : '';
      pword.innerHTML = '<b style="color:var(--accent)">Ready.</b> ' + esc(nm) + ' took ' + res.secs + ' s (' + sizeTxt + '). '
        + (inst.length ? 'It\'s in the ' + inst.join(' and ') + ' - start (or restart) the game and look for it in the ' + noun + ' list.'
                       : 'Not put in the game - tick "Put it in the game" next time.')
        + esc(rep + cams)
        + (warn.length ? '<br><span style="color:var(--red)">' + esc(warn.join(' · ')) + '</span>' : '');
      pcheck.append(cvCheckBox(res.check));
      ptime.textContent = '';
      lastId = s.id || lastId;
      uninst.hidden = !inst.length;
    } else if (s.state === 'error' || s.state === 'cancelled') {
      fill.style.width = '100%';
      fill.style.background = 'var(--red)';
      const why = lines.filter(l => l.startsWith('! ')).slice(-2).map(l => l.slice(2)).join(' ');
      pword.innerHTML = s.state === 'cancelled' ? 'Stopped.' : '<b style="color:var(--red)">That didn\'t work.</b> '
        + esc(res.error || why || '') + ' <span class="tiny dim">Open Details for the full output.</span>';
      if (res.check) pcheck.append(cvCheckBox(res.check));
      ptime.textContent = '';
    }
    if (wasRunning && !running) {
      const c2 = await api(T ? 'convert/tracks' : 'convert/cars');
      if (c2.ok) { all.length = 0; all.push(...(T ? c2.tracks : c2.cars)); paint(); const c = all.find(x => x.folder === cvPick); if (c) pick(c); }
    }
    wasRunning = running;
    if (running && !cvTimer) cvTimer = setInterval(poll, 800);
    if (!running && cvTimer) { clearInterval(cvTimer); cvTimer = null; }
  }
  poll();
}
