/* ACECM UI - gamesettings. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
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
      if (!await ask(`Apply ${names.length} settings file(s) from ${f.name}?

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
        if (!await ask('Restore ' + b.name + '?')) return;
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
    if (!await ask('Restore this file from the ACECM backup?')) return;
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
