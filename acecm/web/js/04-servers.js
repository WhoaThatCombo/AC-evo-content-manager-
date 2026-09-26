/* ACECM UI - servers. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* -------------------------------------------------------------- servers -- */
let editing = null;
// stop-functions for every log currently following, so a page rebuild or a
// navigation does not leave them polling a <pre> that is no longer on screen
let openLogTimers = [];
function stopAllLogFollows() {
  openLogTimers.forEach(fn => { try { fn(); } catch (e) {} });
  openLogTimers = [];
}

async function serversPage() {
  stopAllLogFollows();
  const [pr, trkWrap] = await Promise.all([
    api('profiles'), api('tracks'),
  ]);
  const { profiles, template, options, telemetry: telState } = pr || {};
  const trk = (trkWrap && trkWrap.tracks) || [];
  const p = $('#page');
  p.innerHTML = '';

  /* ⚠ The lobby backend is a hard dependency, not a nicety. Once ACECM has
     pointed the client at 127.0.0.1:448 it talks to nothing else, so with the
     proxy stopped the in-game Multiplayer list is EMPTY - not "missing your
     server", empty, including every public one. That reads as a broken game
     rather than a stopped helper, and it is the single most confusing state
     this app can leave the player in. Say so here, where servers are managed,
     and offer the one-click fix. Only when the client is actually patched:
     an unpatched install has no such dependency and needs no warning. */
  try {
    const be = await api('backend');
    if (be && be.client_patched && !be.listening) {
      const warn = el('div', 'card');
      warn.style.borderColor = '#b4632a';
      warn.innerHTML = '<h2>Lobby backend is stopped</h2>'
        + '<div class="tiny">The game is pointed at this app for its server '
        + 'list, so until the backend is running <b>the in-game Multiplayer '
        + 'list will be empty</b> - your own servers and every public one. '
        + 'That is not a problem with the game.</div>';
      const go = el('button', 'primary', 'Start backend');
      go.onclick = async () => {
        go.disabled = true;
        const r = await api('backend/start', { mode: 'proxy' });
        toast(r && r.ok ? 'Backend started' : ((r && r.error) || 'Failed'),
              !(r && r.ok));
        serversPage();
      };
      warn.append(go);
      p.append(warn);
    } else if (be && be.listening) {
      /* quiet confirmation - the failure above is invisible otherwise, so
         its absence should be visible too */
      const okline = el('div', 'tiny dim',
        'Lobby backend running on port ' + (be.port || 448)
        + ' - the in-game server list is being served.');
      okline.style.margin = '0 0 8px 2px';
      p.append(okline);
    }
  } catch (e) { /* never block the page on a status probe */ }

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
  // ⚠ server only. The client/single-player switch moved to Drive > Assists,
  // where it reads as the personal setting it is.
  for (const side of ['server']) {
    const info = pens && pens[side];
    if (!info) continue;
    const label = 'Dedicated server (affects everyone who joins)';
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
      // the live feed hands the button back the moment the server is up
      // (paintServerPills); this is only the backstop if it never comes up
      start._tick = tick;
      start._was = was;
      setTimeout(() => {
        if (!start._tick) return;
        clearInterval(start._tick); start._tick = null;
        start.disabled = false; start.textContent = was;
      }, 90000);
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
    /* ---- live log ---------------------------------------------------------
       This used to fetch once and paste the tail, so watching a server start
       meant clicking Logs over and over. It follows now: only the bytes
       written since the last poll come back, which is why it can run every
       couple of seconds without re-reading the file each time.

       ⚠ The timer is owned by the <pre>. This whole row is rebuilt by
       serversRefresh on a timer, so a poller left running against a detached
       node would keep polling for the rest of the session - one more leaked
       every few seconds, for every server whose log had ever been opened. */
    const logs = el('button', 'sm', 'Logs');
    const pre = el('pre', 'log');
    pre.style.display = 'none';
    let logAt = 0, logTimerId = null;

    const atBottom = () =>
      pre.scrollHeight - pre.scrollTop - pre.clientHeight < 40;

    const pullLog = async () => {
      if (!pre.isConnected) { stopLog(); return; }
      const r = await api(`server/log?id=${prof.id}&since=${logAt}`);
      if (!r) return;
      // ⚠ stick to the bottom only if the user was ALREADY there. Yanking
      // the view back down while they are reading further up is exactly what
      // makes a live log unusable.
      const follow = atBottom();
      if (r.reset) {
        pre.textContent = (r.lines || []).join('\n') || r.error || '(empty)';
      } else if ((r.lines || []).length) {
        pre.textContent += (pre.textContent ? '\n' : '') + r.lines.join('\n');
      }
      logAt = r.size || logAt;
      if (follow) pre.scrollTop = pre.scrollHeight;
    };

    const stopLog = () => {
      if (logTimerId) { clearInterval(logTimerId); logTimerId = null; }
    };

    logs.onclick = async () => {
      const opening = pre.style.display === 'none';
      pre.style.display = opening ? '' : 'none';
      logs.classList.toggle('primary', opening);
      if (!opening) { stopLog(); return; }
      logAt = 0;
      await pullLog();
      stopLog();
      logTimerId = setInterval(pullLog, 2000);
      openLogTimers.push(stopLog);
    };

    /* Capture the log to hand someone, without them hunting for the file or
       reading it off screen. Clipboard first - that is what actually gets
       pasted into a bug report or a Discord message - a download only when
       the clipboard is refused. Always the FULL log (since=0), independent
       of whatever the live view above has polled so far: opening this after
       a crash and getting only the tail from when you happened to click
       Logs would cut off exactly the part that explains it. */
    const capture = el('button', 'sm', 'Copy log');
    capture.title = 'Copy the full server log, to paste into a bug report';
    capture.onclick = async () => {
      const was = capture.textContent;
      capture.disabled = true;
      capture.textContent = 'Copying…';
      try {
        const r = await api(`server/log?id=${prof.id}&since=0`);
        const text = (r && r.lines || []).join('\n')
          || (r && r.error) || '(log is empty)';
        try {
          await navigator.clipboard.writeText(text);
          capture.textContent = 'Copied!';
        } catch (e) {
          const blob = new Blob([text], { type: 'text/plain' });
          const a = document.createElement('a');
          a.href = URL.createObjectURL(blob);
          a.download = `${(prof.name || 'server').replace(/[^\w-]+/g, '_')}`
                     + `-${new Date().toISOString().slice(0, 19).replace(/:/g, '-')}.log`;
          a.click();
          setTimeout(() => URL.revokeObjectURL(a.href), 4000);
          capture.textContent = 'Downloaded';
        }
      } catch (e) {
        toast('Could not read the log', true);
        capture.textContent = was;
      } finally {
        setTimeout(() => { capture.disabled = false; capture.textContent = was; }, 1800);
      }
    };
    /* Send this server's content to an ACECM running ON the server box.
       ACECM already knows what the profile needs - the modded cars and the
       custom track - so this is one button rather than a file-by-file copy. */
    const send = el('button', 'sm', 'Send content to server');
    send.title = 'Upload the mods and track this server needs to a remote '
               + 'ACECM (started with --headless)';
    send.onclick = async () => {
      const p = await api('push/plan', { id: prof.id });
      if (!p || !p.ok) { toast((p && p.error) || 'could not work out what to send', true); return; }
      const items = [...(p.mods || []), ...(p.tracks || [])];
      if (!items.length) {
        toast('Nothing modded about this server - there is nothing to send');
        return;
      }
      if ((p.missing || []).length) {
        toast('not installed here: ' + p.missing.join(', '), true);
      }
      const last = localStorage.getItem('acecm_push_base') || '';
      const base = prompt('Address of the ACECM on the server box:', last
                          || 'http://100.x.y.z:8092');
      if (!base) return;
      localStorage.setItem('acecm_push_base', base.trim());
      const tok = prompt("That server's admin token (blank if it is this PC):",
                         localStorage.getItem('acecm_push_token') || '');
      if (tok === null) return;
      localStorage.setItem('acecm_push_token', tok.trim());
      const list = items.map(x => '• ' + x).join('\n');
      if (!await ask('Send to ' + base + ':\n\n' + list)) return;
      const r = await api('push/send', { id: prof.id, base: base.trim(), token: tok.trim() });
      if (!r || !r.ok) { toast((r && r.error) || 'could not start', true); return; }
      toast('Sending ' + items.length + ' item(s) - watch the bar');
      progKick();
    };
    /* ---- shareable package -------------------------------------------
       For the person on bad internet or the far side of the world: one zip
       with this server's track and mod cars, to hand over however they like.
       It holds exactly what ACECM would have sent them over the network - no
       stock content, nothing from your other servers. */
    const packBtn = el('button', 'sm', 'Export package');
    packBtn.title = 'Save this server’s track and car mods as one zip to '
      + 'share - the other person drags it onto their ACECM';
    packBtn.onclick = async () => {
      const pv = await api('pack/preview?id=' + encodeURIComponent(prof.id));
      if (!pv || !pv.ok) { toast((pv && pv.error) || 'could not read', true); return; }
      if (!pv.files) {
        toast('Nothing to package - this server is all stock content', true);
        return;
      }
      const mb = (pv.bytes / 1e6).toFixed(0);
      const what = [
        pv.tracks.length ? pv.tracks.length + ' track' : '',
        pv.mods.length ? pv.mods.length + ' car mod' + (pv.mods.length > 1 ? 's' : '') : '',
      ].filter(Boolean).join(' + ');
      const NL = String.fromCharCode(10);
      const msg = [
        'Package "' + prof.name + '"?', '',
        what,
        pv.files + ' files, about ' + mb + ' MB', '',
        'Saved to your Downloads folder. This can take a few minutes and',
        'the progress bar will show it.',
      ].join(NL);
      if (!await ask(msg, 'Build it')) return;
      packBtn.disabled = true;
      progKick();
      const r = await apiLong('pack/build', { id: prof.id });
      packBtn.disabled = false;
      if (!r || !r.ok) { toast((r && r.error) || 'could not build', true); return; }
      toast('Saved ' + r.path.split(/[\/]/).pop()
            + ' (' + (r.bytes / 1e6).toFixed(0) + ' MB)');
    };
    const del = el('button', 'sm danger', 'Delete');
    del.onclick = async () => {
      if (!await ask('Delete "' + prof.name + '"?')) return;
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
    const more = el('details');
    more.style.marginTop = '8px';
    more.append(el('summary', 'tiny dim',
                   'More — logs, share package, delete'));
    const extra = el('div', 'row wrap');
    extra.style.marginTop = '8px';
    // ⚠ telemetry controls (tOn/tOff/tView/tLink/tpill) are deliberately not
    // rendered - only useful when hosting a multiplayer race. They are still
    // built above, so restoring them is adding them back to this line.
    extra.append(logs, capture, packBtn, del);
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
  // running / stopped / clients arrive from the live feed and are painted in
  // place - no rebuild, no poll of its own
  livePage('servers', s => paintServerPills(s.servers || []));
}

/* ⚠ Look things up in the CURRENT page, not the whole document. While a
   page refreshes itself the old copy is still on screen (see bufferBegin),
   and document.querySelector finds that one first - so an update meant for
   the new page landed on the one about to be thrown away. */
function pageQ(sel) { const p = $('#page'); return p ? p.querySelector(sel) : null; }

function paintServerPills(list) {
  for (const sv of list) {
    const pill = pageQ('[data-st="' + sv.id + '"]');
    if (pill) {
      const cls = 'pill ' + (sv.running ? 'on' : 'off');
      const html = '<i class="dot"></i>' + (sv.running
        ? 'running' + (sv.clients != null ? ' · ' + sv.clients + ' clients' : '')
        : 'stopped');
      if (pill.className !== cls) pill.className = cls;
      if (pill.innerHTML !== html) pill.innerHTML = html;
    }
    // A start that has finished must give the button back, otherwise it stays
    // stuck on "starting… 0s" forever now that nothing rebuilds it.
    const start = pageQ('[data-start="' + sv.id + '"]');
    if (start && sv.running && start._tick) {
      clearInterval(start._tick); start._tick = null;
      start.disabled = false;
      start.textContent = start._was || 'Start';
    }
  }
}

/* Update the live parts of the servers page WITHOUT rebuilding it.
   A rebuild collapsed every open "More" panel, closed any log pane you were
   reading and scrolled the editor away - all to change a pill from "starting"
   to "running". Actions now patch the few elements that changed. */
async function serversRefresh() {
  const anchor = pageQ('[data-st]');
  if (!anchor) return;                    // not on this page any more
  // server pills come from the live feed now (paintServerPills); this only
  // fills what the feed does not carry - each server's telemetry tracker
  const pr = await api('profiles');
  const tel = (pr && pr.telemetry) || {};
  Object.entries(tel).forEach(([id, ts]) => {
    const tp = pageQ('[data-tp="' + id + '"]');
    if (tp) {
      tp.className = 'pill ' + (ts.running ? 'on' : 'off');
      tp.innerHTML = '<i class="dot"></i>telemetry '
        + (ts.running ? 'on :' + ts.port : 'off');
    }
    const on = pageQ('[data-ton="' + id + '"]');
    if (on) on.textContent = ts.running ? 'Restart telemetry' : 'Start telemetry';
    const off = pageQ('[data-toff="' + id + '"]');
    if (off) off.disabled = !ts.running;
    const view = pageQ('[data-tview="' + id + '"]');
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
    // ⚠ NOT trk.forEach - trk is content.tracks(), which also carries a
    // synthetic entry per imported track (index 10000+n, so Drive's picker
    // can offer them too) alongside the real ones. Those same tracks are
    // ALSO in "Deployed tracks" below, by name, which is the only address
    // the server understands for one. Listing them here too let this exact
    // synthetic index be picked and saved as track_index with no
    // custom_track set - a number events_practice.json has maybe 40 entries
    // for, not 10000+ - so the launcher's events[EVENT_IDX] died before it
    // ever reached the CUSTOM_EVENT it needed.
    trk.filter(t => !t.mod).forEach(t => {
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
    'Stock and modded are separate switches — one click each.');
  const cars = ((extra && extra.cat && extra.cat.cars) || []);
  const chosen = new Set(prof.cars || []);
  const box = el('div');
  box.style.cssText = 'grid-column:1/-1';
  const policy = el('div', 'row wrap');
  policy.style.marginBottom = '8px';
  const chips = el('div', 'row wrap');
  // Two INDEPENDENT axes. The old three buttons could not express "all
  // Kunos, no mods": picking "all Kunos + the mods I pick" with nothing
  // picked fell straight back to every installed mod, so the only way to run
  // a stock-only server was to whitelist ~100 Kunos cars by hand.
  if (prof.mods == null || prof.mods === '') {
    const pickedMods = (prof.cars || []).filter(id => {
      const m = cars.find(x => x.id === id);
      return m ? m.mod : true;
    });
    prof.mods = pickedMods.length ? 'pick'
              : (prof.allow_kunos === false ? 'none' : 'all');
  }
  const setStock = (on) => {
    prof.allow_kunos = !!on;
    if (!on && prof.mods === 'none') prof.mods = 'pick';
    prof.cars = [...chosen];
    redraw();
  };
  const setMods = (mode) => {
    prof.mods = mode;
    if (mode !== 'pick') {
      // drop mod picks so the tiles do not contradict the switch
      [...chosen].forEach(id => {
        const m = cars.find(x => x.id === id);
        if (!m || m.mod) chosen.delete(id);
      });
    }
    if (mode === 'none' && prof.allow_kunos === false && !chosen.size)
      prof.allow_kunos = true;
    prof.cars = [...chosen];
    redraw();
  };
  const redraw = () => {
    policy.innerHTML = '';
    const stockRow = el('div', 'row wrap');
    stockRow.append(el('span', 'tiny dim', 'Stock cars'));
    [[true, 'All Kunos'], [false, 'Only the ones I pick']].forEach(([v, lab]) => {
      const b = el('button',
        'sm' + ((!!prof.allow_kunos === v) ? ' primary' : ''), lab);
      b.onclick = () => setStock(v);
      stockRow.append(b);
    });
    const modRow = el('div', 'row wrap');
    modRow.style.marginTop = '6px';
    modRow.append(el('span', 'tiny dim', 'Modded cars'));
    [['all', 'All mods'], ['none', 'No mods'],
     ['pick', 'Only the ones I pick']].forEach(([k, lab]) => {
      const b = el('button', 'sm' + (prof.mods === k ? ' primary' : ''), lab);
      b.onclick = () => setMods(k);
      modRow.append(b);
    });
    policy.append(stockRow, modRow);
    chips.innerHTML = '';
    const note =
      prof.mods === 'all'
        ? (prof.allow_kunos ? 'Every stock car and every installed mod is allowed.'
                            : 'Every installed mod is allowed. No stock cars.')
      : prof.mods === 'none'
        ? (prof.allow_kunos ? 'Stock cars only. Mods cannot join.'
                            : 'Only the stock cars you highlight below can join.')
        : 'Click mods below to allow them'
          + (prof.allow_kunos ? '. Stock cars stay allowed.' : ' (stock cars off).');
    chips.append(el('span', 'tiny dim', note));
    if (prof.mods === 'pick' || !prof.allow_kunos) {
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
      img.src = 'api/thumb/car?id=' + encodeURIComponent(x.model || x.id)
              + '&preset=' + encodeURIComponent(x.id);
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
        // Clicking a tile is a PICK, so flip that axis into pick mode rather
        // than silently wiping the other one.
        if (x.mod && prof.mods !== 'pick') { prof.mods = 'pick'; chosen.clear(); }
        if (!x.mod && prof.allow_kunos) { prof.allow_kunos = false; chosen.clear(); }
        if (on()) chosen.delete(x.id); else chosen.add(x.id);
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

  // ⚠ `policy` was BUILT and repainted but never put in the DOM, so the
  // whole car-policy control has been invisible: the only way to change
  // what a server allows was clicking the car tiles one at a time.
  box.append(policy, chips, panels);
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
