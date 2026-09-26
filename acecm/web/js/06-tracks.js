/* ACECM UI - tracks. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* --------------------------------------------------------------- tracks -- */
async function tracksPage() {
  const [d, loc, st] = await Promise.all([
    api('tracks'), api('browser/local?refresh=1'), api('thumbs/status'),
  ]);
  const p = $('#page');
  p.innerHTML = '';

  /* After a game update the client's tracks.table is replaced and every
     imported track vanishes from it, even though the files are still on
     disk. Detect that and offer to put them back, rather than leaving the
     user to wonder where their tracks went. */
  try {
    const miss = await api('tracks/missing');
    if (miss && miss.ok && miss.count > 0) {
      const warn = el('div', 'card');
      warn.style.borderLeft = '3px solid var(--accent)';
      warn.innerHTML = '<h2>' + miss.count + ' imported track'
        + (miss.count === 1 ? '' : 's') + ' missing</h2>'
        + '<div class="tiny dim" style="margin-bottom:9px">These are on disk '
        + 'but the game forgot them &mdash; usually because Assetto Corsa EVO '
        + 'updated and replaced its track list. Restoring re-registers them '
        + '(about 15s each). Close the game first.</div>';
      const b = el('button', 'sm primary', 'Restore ' + miss.count + ' track'
                   + (miss.count === 1 ? '' : 's'));
      b.onclick = async () => {
        const r = await api('tracks/restore', {});
        if (!r || !r.ok) { toast((r && r.error) || 'could not start', true); return; }
        toast('Restoring ' + (r.count || '') + ' track(s) - watch the bar');
        progKick();
      };
      warn.append(b);
      p.append(warn);
    }
  } catch (e) { /* detection is best-effort */ }

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
    liveWatch(s => s.covers, j => j.state === 'running', j => {
      dec.textContent = 'Decode all covers';
      toast(`${j.made} cover(s) ready in ${j.seconds}s`);
      if (_page === 'tracks') tracksPage();
    }, j => {
      dec.textContent = `Decoding ${j.done}/${j.total} — ${j.current}`;
    });
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
