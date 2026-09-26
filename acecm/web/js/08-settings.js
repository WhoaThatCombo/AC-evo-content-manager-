/* ACECM UI - settings. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* ------------------------------------------------------------- settings -- */
async function settingsPage() {
  const cfg = await api('config');
  const p = $('#page');
  p.innerHTML = '';

  /* ---- disk space ------------------------------------------------------
     EVO content is enormous - about 600 MB a car, several GB a track - so a
     modest collection runs to tens of gigabytes. Windows can store it
     compressed and decompress on read, which the game never notices.
     Measured on a real folder: 46.3 GB held in 11.6 GB, 4.0 to 1.

     ⚠ The readout has to be LOGICAL vs ON-DISK. A file's size does not
     change when it is compressed, so printing file sizes would show the same
     number before and after and look broken - the whole point is the gap
     between what the content weighs and what it occupies. */
  const diskCard = el('div', 'card');
  diskCard.innerHTML = '<h2>Disk space</h2>';
  const diskLine = el('div', 'tiny dim', 'Measuring the mods folder…');
  diskCard.append(diskLine);
  const diskRow = el('div', 'row');
  diskCard.append(diskRow);
  p.append(diskCard);

  const fmtDiskGB = b => (Number(b || 0) / 1e9).toFixed(2) + ' GB';
  const paintDisk = (m) => {
    diskRow.innerHTML = '';
    if (!m || !m.ok) {
      /* ⚠ "measuring" is not a failure: the walk takes longer than the
         browser waits, so it runs in the background and we poll. Saying
         "could not measure" here made a working feature look broken. */
      if (m && m.measuring) {
        diskLine.textContent = 'Measuring the mods folder…';
        setTimeout(() => api('compress').then(paintDisk).catch(() => {}), 3000);
        return;
      }
      diskLine.textContent = (m && m.supported === false)
        ? 'Disk compression is a Windows feature — not available here.'
        : ((m && m.error) || 'could not measure the mods folder');
      return;
    }
    const pct = m.logical ? Math.round((1 - m.on_disk / m.logical) * 100) : 0;
    diskLine.innerHTML =
      '<b>' + fmtDiskGB(m.logical) + '</b> of mods stored in <b>'
      + fmtDiskGB(m.on_disk) + '</b> on disk'
      + (m.ratio >= 1.05
          ? ' — ' + m.ratio.toFixed(1) + ' to 1, saving '
            + fmtDiskGB(m.saved) + ' (' + pct + '%)'
          : ' — not compressed yet')
      + '<br><span class="dim">' + esc(m.path) + ' · '
      + Number(m.files).toLocaleString() + ' files</span>';
    const go = el('button', 'primary',
                  m.ratio >= 1.05 ? 'Compress again' : 'Compress mods folder');
    go.title = 'Windows stores the files compressed and decompresses them on '
             + 'read. The game sees ordinary files.';
    go.onclick = async () => {
      if (!await ask('Compress the mods folder?\n\n'
          + fmtDiskGB(m.logical) + ' across '
          + Number(m.files).toLocaleString()
          + ' files. This takes a while, and the game should be closed.\n\n'
          + 'Nothing about the content changes - only how much disk it uses, '
          + 'and it can be undone.')) return;
      const r = await api('compress/start', {});
      if (!r.ok) { toast(r.error || 'could not start', true); return; }
      watchCompress();
    };
    diskRow.append(go);
    if (m.ratio >= 1.05) {
      const un = el('button', 'sm', 'Undo');
      un.title = 'Store the files uncompressed again';
      un.onclick = async () => {
        if (!await ask('Store the mods folder uncompressed again?\n\n'
            + 'That needs about ' + fmtDiskGB(m.logical)
            + ' of free space.')) return;
        const r = await api('compress/start', { undo: true });
        if (!r.ok) { toast(r.error || 'could not start', true); return; }
        watchCompress();
      };
      diskRow.append(un);
    }
  };
  const watchCompress = () => {
    diskRow.innerHTML = '';
    diskLine.textContent = 'Working… this can take a while on a large folder. '
                         + 'You can leave this page.';
    // from the live feed; page-scoped, and a revisit picks it up again below
    let sawActive = false, ended = false;
    const off = livePage('compress', s => {
      if (ended) return;
      const st = s.compress || {};
      if (st.active) { sawActive = true; return; }
      // ⚠ a snapshot from just BEFORE Start shows the last run's result
      if (!sawActive) return;
      ended = true;
      setTimeout(() => off(), 0);
      if (st.phase === 'error') {
        toast(st.error || 'compression failed', true);
        api('compress?refresh=1').then(paintDisk);
        return;
      }
      if (st.phase === 'done') {
        const b = st.before || {}, a = st.after || {};
        toast('Mods folder: ' + fmtDiskGB(b.on_disk) + ' → '
              + fmtDiskGB(a.on_disk) + ' ('
              + fmtDiskGB(Math.max(0, (b.on_disk || 0) - (a.on_disk || 0)))
              + ' freed)');
        paintDisk(a);
        return;
      }
    });
    liveKick();
  };
  /* measured lazily: walking ~90k files costs seconds and must not hold up
     the rest of Settings */
  api('compress').then(paintDisk).catch(() => {});
  api('compress/status').then(st => {
    if (st && st.ok && st.active) watchCompress();
  }).catch(() => {});

  /* ---- launch the game, on its own -------------------------------------
     ⚠ Deliberately NOT on Drive. Drive writes a session and then launches;
     this just starts the game with nothing set up, which is only useful for
     poking at menus and reading what the game writes back. Putting it on the
     page you use every day is how it gets pressed by accident, so it lives
     at the top of Settings - somewhere you have to mean to go - and asks
     before it does anything. */
  const lc = el('div', 'card');
  lc.innerHTML = '<h2>Launch the game</h2>';
  lc.append(el('div', 'tiny dim',
    'Starts Assetto Corsa EVO on its own, with no session written - for '
    + 'checking what the game does to its own files. To actually drive, use '
    + 'Drive: it writes the session first.'));
  const lrow = el('div', 'row');
  const lbtn = el('button', 'sm', 'Launch game');
  const lnote = el('span', 'tiny dim');
  lbtn.onclick = async () => {
    if (!await ask('Launch Assetto Corsa EVO now? No session is written - '
                 + 'the game starts with whatever it last had.')) return;
    lbtn.disabled = true;
    lnote.textContent = 'launching…';
    try {
      const r = await api('game/launch', {});
      lnote.textContent = r && r.ok
        ? 'launched' + (r.via ? ' (' + r.via + ')' : '')
        : ((r && r.error) || 'could not launch');
      if (!r || !r.ok) toast((r && r.error) || 'Could not launch', true);
    } finally {
      lbtn.disabled = false;
    }
  };
  lrow.append(lbtn, lnote);
  lc.append(lrow);
  p.append(lc);

  // (The "More" card that linked Game settings, Backend and Logs is gone:
  // Game settings is a tab beside this page, the other two are Diagnostics.)


  // ---- faster loading ----------------------------------------------------
  const boot = el('div', 'card');
  boot.innerHTML = '<h2>Faster loading</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">Three switches the '
    + 'game already has and ships turned off, named in its own flag list: it '
    + 'stops preloading every car at boot and loads them when needed, takes '
    + 'the fast path when swapping mode on the same track, and skips the idle '
    + 'camera sequence. The intro is always skipped and is separate from '
    + 'this.<br><br><b>This moves work rather than deleting it</b> — boot is '
    + 'shorter and the first use of a car pays for it, so it is worth timing '
    + 'both ways for how you actually play. Only applies when ACECM launches '
    + 'the game; starting from Steam is unaffected.</div>';
  const brow = el('div', 'row wrap');
  const blab = el('label', 'ctl');
  const bbox = el('input');
  bbox.type = 'checkbox';
  bbox.checked = !!cfg.fast_boot;
  bbox.onchange = async () => {
    const r = await api('config', { fast_boot: bbox.checked });
    if (r && r.error) { toast(r.error, true); bbox.checked = !bbox.checked; return; }
    toast(bbox.checked
      ? 'Faster loading on — applies next time ACECM launches the game'
      : 'Back to the game default');
  };
  blab.append(bbox, document.createTextNode(' Skip car preloading and the idle camera'));
  brow.append(blab);
  boot.append(brow);

  // ---- in-game HUD -------------------------------------------------------
  const hud = el('div', 'card');
  hud.innerHTML = '<h2>In-game HUD</h2>'
    + '<div class="tiny dim" style="margin-bottom:10px">EVO has no units '
    + 'setting — the speedometer is hard-wired to km/h. This redraws it in '
    + 'mph while the game is running, and looks the same otherwise. Nothing '
    + 'on disk changes, so launching straight from Steam is unaffected.</div>';
  const hrow = el('div', 'row wrap');
  const hlab = el('label', 'ctl');
  const hbox = el('input');
  hbox.type = 'checkbox';
  hbox.checked = !!cfg.hud_mph;
  hbox.onchange = async () => {
    const r = await api('config', { hud_mph: hbox.checked });
    if (r && r.error) { toast(r.error, true); hbox.checked = !hbox.checked; return; }
    // apply straight away if a session is up, rather than at the next poll
    const now = await api('hud/mph', { on: hbox.checked });
    toast(now && now.ok
      ? (hbox.checked ? 'Speedometer set to mph' : 'Speedometer back to km/h')
      : (hbox.checked ? 'Saved — applies when the HUD is up' : 'Saved'));
  };
  hlab.append(hbox, document.createTextNode(' Speedometer in mph'));
  hrow.append(hlab);
  hud.append(hrow);

  // ---- back to pits on a button -----------------------------------------
  hud.append(el('h2', null, 'Back to pits'));
  hud.append(el('div', 'tiny dim',
    'Sends the car to the pitlane immediately, even at speed. The game can '
    + 'already do this — its own button just lives in the pause menu and is '
    + 'hidden while you are on track. Bind a wheel, button box or controller '
    + 'button to reach it without pausing.'));
  const prow = el('div', 'row wrap');
  prow.style.marginTop = '8px';
  const bound = el('span', 'pill ' + (cfg.pit_btn_mask ? 'on' : 'off'));
  bound.innerHTML = '<i class="dot"></i>'
    + esc(cfg.pit_btn_label || 'not bound');
  const bind = el('button', 'sm primary', 'Bind a button');
  bind.onclick = async () => {
    const was = bind.textContent;
    bind.disabled = true;
    bind.textContent = 'Press it now…';
    const r = await apiLong('pit/capture', { seconds: 6 }, 20000);
    bind.disabled = false;
    bind.textContent = was;
    if (!r || !r.ok) { toast((r && r.error) || 'nothing pressed', true); return; }
    const s = await api('pit/bind',
      { kind: r.kind, id: r.id, mask: r.mask, label: r.label });
    if (s && s.error) { toast(s.error, true); return; }
    toast('Bound to ' + r.label);
    settingsPage();
  };
  const clear = el('button', 'sm', 'Clear');
  clear.onclick = async () => {
    await api('pit/bind', { kind: '', id: '', mask: 0, label: '' });
    toast('Binding cleared');
    settingsPage();
  };
  const test = el('button', 'sm', 'Test now');
  test.title = 'Send the car to the pits right now';
  test.onclick = async () => {
    const r = await api('pit/go', {});
    toast(r && r.ok ? 'Sent to pits'
                    : ((r && r.error) || 'not in a session'), !(r && r.ok));
  };
  prow.append(bound, bind, clear, test);
  hud.append(prow);

  const krow = el('div', 'row wrap');
  krow.style.marginTop = '8px';
  const kin = el('input');
  kin.value = cfg.pit_key || '';
  kin.placeholder = 'ctrl+y';
  kin.style.maxWidth = '10em';
  const ksave = el('button', 'sm', 'Save key');
  ksave.onclick = async () => {
    const r = await api('config', { pit_key: kin.value.trim() });
    toast(r && r.error ? r.error
          : (kin.value.trim() ? 'Keyboard shortcut set' : 'Keyboard shortcut off'),
          !!(r && r.error));
  };
  krow.append(el('span', 'tiny dim', 'Keyboard '), kin, ksave);
  hud.append(krow);
  hud.append(el('div', 'tiny dim',
    'The keyboard shortcut only fires while the game is the window in front, '
    + 'so it cannot go off while you are typing elsewhere. A bound '
    + 'wheel/controller button works whatever has focus. Leave the key box '
    + 'empty to turn the shortcut off.'));
  hud.append(el('div', 'tiny dim',
    'Verified in single-player. In multiplayer the server decides whether to '
    + 'honour it, so it may simply do nothing.'));
  p.append(boot);
  p.append(hud);

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
        if (!await ask('Remove the Start Menu and Desktop shortcuts?\n\n'
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

  c.append(save);
  p.append(c);
}
