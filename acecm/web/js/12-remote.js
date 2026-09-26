/* ACECM UI - remote. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* ------------------------------------------ content from other servers -- */
/* What this machine already has, so a row can say "you are missing this"
   without a round trip per server. */
let brLocal = { tracks: [], cars: [] };
let brTags = {};

/* Ask the host beside a game server for whatever we lack.

   ⚠ Only an ACECM can answer. The dedicated server's archive holds a track's
   LOGIC - seven scene files - while a playable track is the ~1 GB of art in
   the player's own folder. Those bytes exist only on the host's game install,
   so a stock Kunos server has nothing to give and we say so. */
/* Download a track from an EvoForge server's share.

   Those hosts do not run ACECM, so browser/discover can only ever answer
   "not sharing" for them; they publish over their own protocol instead and
   the directory already gave us the URL. The share's index carries absolute
   byte spans into the pack, so we plan first and state the real size before
   fetching anything - these are multi-GB tracks and starting one by accident
   is expensive. The host publishes a slot count (max, observed 3); we take
   one at a time and back off politely when it is busy. */
async function evoshareFrom(s) {
  const base = s.share_url;
  toast('Asking ' + (s.name || s.server_ip) + ' what it shares…');
  const m = await api('evoshare/manifest?base=' + encodeURIComponent(base));
  if (!m.ok) { toast(m.error || 'that share did not answer', true); return; }
  const items = m.content || [];
  if (!items.length) { toast('that share publishes nothing', true); return; }
  if (m.max && m.active >= m.max) {
    toast('That host is busy (' + m.active + '/' + m.max
          + ' downloads) — try again shortly', true);
    return;
  }
  /* ⚠ Plan EVERY item, not just the first. A host lists one entry per thing,
     and a drift server publishes its track plus five car mods - taking
     content[0] meant the cars were invisible and "Get content" looked like it
     had nothing to offer. contentFrom's ACECM path already learned this; this
     path repeated the mistake. */
  toast('Checking what you already have…');
  const plans = [];
  for (const it of items) {
    const kind = it.kind === 'car' ? 'car' : 'track';
    const p = await api('evoshare/plan?base=' + encodeURIComponent(base)
                        + '&file=' + encodeURIComponent(it.file)
                        + '&kind=' + kind);
    if (p && p.ok && (p.need || []).length) plans.push({ it, kind, p });
  }
  if (!plans.length) {
    toast('You already have everything ' + (s.name || s.server_ip)
          + ' publishes');
    return;
  }
  const bytes = plans.reduce((a, x) => a + (x.p.bytes || 0), 0);
  const lines = plans.map(x => '  • ' + (x.it.name || x.it.id)
    + ' (' + x.kind + ', ' + ((x.it.bytes || 0) / 1e9).toFixed(2) + ' GB)');
  if (!await ask('Download ' + plans.length + ' item'
      + (plans.length === 1 ? '' : 's') + ' from '
      + (s.name || s.server_ip) + '?\n\n' + lines.join('\n')
      + '\n\nTotal ' + (bytes / 1e9).toFixed(2) + ' GB, from that server\'s '
      + 'own machine rather than from ACECM.')) return;
  evoshareQueue(base, plans.slice());
}

/* One at a time: the share advertises a slot limit (max, observed 3) and the
   backend runs a single download, so the queue lives here. */
async function evoshareQueue(base, queue) {
  const next = async () => {
    const job = queue.shift();
    if (!job) {
      toast('All downloads finished');
      /* ⚠ Rebuild the page that is showing. The car picker filters what the
         server allows against what you HAVE, and both lists were read before
         the download - so a car that just arrived was still absent and the
         server read as "no cars available". `contentPage && null` was a
         no-op left here by mistake, so nothing refreshed at all.
         Safe to rebuild Drive now: the live selection survives it. */
      if (_page === 'drive') drivePage();
      else if (_page === 'content') contentPage();
      return;
    }
    const r = await api('evoshare/start',
                        { base, file: job.it.file, kind: job.kind,
                          folder: job.it.id || '' });
    if (!r.ok) { toast(r.error || 'could not start', true); return; }
    toast('Downloading ' + (job.it.name || job.it.id) + ' — '
          + ((job.it.bytes || 0) / 1e9).toFixed(2) + ' GB'
          + (queue.length ? ' (' + queue.length + ' more after this)' : ''));
    /* ⚠ Show the shared bar NOW rather than on the next poll tick. Without
       this there is a gap at the start of every item where nothing on screen
       says a multi-GB download has begun - which is the whole reason this
       path stopped relying on toasts. */
    progKick();
    evoshareWatch(job.it, next, r.job);
  };
  next();
}

function evoshareWatch(item, onDone, jobId) {
  /* From the live feed, not a poll of its own. App-wide (liveOn, not
     livePage): a download keeps going when you change page, and so must
     the toast and the refresh at its end.
     ⚠ Only THIS job's end counts. The snapshot can still be showing the
     previous download's "done" for a moment after this one starts, which
     would fire onDone at once and start the whole queue over. */
  // ⚠ a flag, not just `off`: liveOn calls back once immediately, before
  // `off` exists, and a job that already ended would then fire twice
  let ended = false, off = null;
  off = liveOn(s => {
    if (ended) return;
    const st = s.evoshare || {};
    if (jobId && st.job !== jobId) return;
    if (st.active || !st.phase) return;
    ended = true;
    setTimeout(() => off && off(), 0);
    if (st.phase === 'done') {
      /* ⚠ Downloaded is not installed. A track needs its table rows, which
         cannot be written while the game holds content.kspkg; a car needs its
         .json descriptor or it simply never appears in the car list. Say
         which of those actually happened. */
      if (st.needs_close) {
        toast('Downloaded ' + (item.name || item.id)
              + ' — close the game, then press Get content again to add it '
              + 'to the track list', true);
      } else if (st.registered === false) {
        toast('Downloaded ' + (item.name || item.id) + ' — but '
              + (st.note || 'it is not in the track list yet'), true);
      } else if (st.kind === 'car' && st.note) {
        toast('Downloaded ' + (item.name || item.id) + ' — ' + st.note, true);
      } else {
        toast('Installed ' + (item.name || item.id));
      }
      if (onDone) onDone();
    } else if (st.phase === 'error') {
      toast(st.error || 'download failed', true);
    } else if (st.phase === 'cancelled') {
      toast('Download cancelled');
    }
  });
}

async function contentFrom(s) {
  /* ⚠ An EvoForge host does NOT run ACECM, so asking browser/discover about
     it can only ever answer "not sharing". Take its own share when the
     directory gave us one, and keep the ACECM protocol for everything else. */
  if (s.share_url) return evoshareFrom(s);
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
  /* ⚠ "need" is only the missing BYTES. A track whose files are all here but
     which never made it into the game's track list needs no download and is
     still not usable, so counting only need meant answering "you already have
     everything" to someone who could not load the track. */
  const wanted = plans.filter(x => x.p.ok && ((x.p.need || []).length
                                   || (x.p.needs_register || []).length));
  if (!wanted.length) {
    toast('You already have everything this host offers'); return;
  }
  const files = wanted.reduce((a, x) => a + (x.p.need || []).length, 0);
  const fixing = wanted.reduce((a, x) => a + (x.p.needs_register || []).length, 0);
  const mb = (wanted.reduce((a, x) => a + (x.p.bytes || 0), 0) / 1e6).toFixed(0);
  if (!files && fixing) {
    if (!await ask(fixing + ' track(s) are already downloaded but missing from '
        + "the game's track list.\n\nAdd them now? "
        + '(Assetto Corsa EVO must be closed.)')) return;
  } else if (!await ask(wanted.map(x => '• ' + x.e.name).join('\n')
      + `\n\nMissing ${files} file(s), ${mb} MB.`
      + (fixing ? `\n${fixing} track(s) also need adding to the game.` : '')
      + `\n\nDownload from ${d.base} and install?`)) return;
  const r = await api('browser/install',
                      { base: d.base, ids: wanted.map(x => x.e.id) });
  if (!r.ok) { toast(r.error || 'install failed', true); return; }
  // ⚠ This used to be one line of text in the page, which is why a download
  // that was working looked like nothing was happening: no bar, no rate, no
  // idea whether a 4 GB track was moving or stuck. Reuse the same progress
  // component a window-drop already uses, so both ways of getting content
  // look and behave the same.
  // ⚠ The BAR is not this function's job any more - startProgressWatch draws
  // it from /api/progress, so it keeps going (and stays visible) if the user
  // wanders off to another page mid-download. All that is left here is
  // noticing the end, so the browser page can refresh itself.
  const bar = $('#brprog');
  const started = Date.now();
  progKick();                       // show the bar now, not on the next tick
  const poll = setInterval(async () => {
    const st = await api('browser/status');
    if (bar) {
      bar.textContent = st.total
        ? `${st.detail} — ${mb(st.done || 0)} of ${mb(st.total)}`
        : (st.detail || '');
    }
    if (st.state === 'done' || st.state === 'error') {
      clearInterval(poll);
      const secs = Math.round((Date.now() - started) / 1000);
      toast(st.state === 'done'
        ? `Content installed in ${secs}s — you can join now`
        : st.detail, st.state === 'error');
      brLocal = await api('browser/local');
      // ⚠ Refresh whatever page the user is actually ON. This only ever
      // refreshed the server-browser page, so after fetching from Content (or
      // from Drive) the new cars stayed invisible until you navigated away and
      // back - the download had worked, the screen just never redrew. With the
      // browser page gone this refreshed nothing at all.
      const spec = PAGES[_page];
      if (spec && typeof spec[2] === 'function') spec[2]();
      // ⚠ Freshly fetched cars have no render yet. The list GET deliberately
      // never renders (it would spawn an evoview console per row on every
      // Drive keystroke), so without this the new cars sit there as blank
      // tiles until someone happens to run a render pass from the Cars page.
      // Kick one in the background; it skips cars already rendered.
      if (st.state === 'done') api('thumbs/build', {});
    }
  }, 1000);
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
  inp.dataset.keep = '1';   // survives the page rebuilding under you
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
