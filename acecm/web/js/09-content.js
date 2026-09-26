/* ACECM UI - content. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
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
    + 'Share or stop each imported track below. The hosted server\'s track '
    + 'starts shared so joiners can download it; Stop sharing turns that off. '
    + 'Copy the link; they paste it into '
    + '<b>Server browser → Fetch from ACECM</b>. Stock tracks everyone already '
    + 'has are not listed.</div>';
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
      row.append(shareTrackBtn(folder, name, on));
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

/* ⚠ Our OWN confirmation, because window.confirm() is not reliable here.
   The embedded webview returns FALSE from confirm() without ever showing a
   dialog, so every `if (!await ask(...)) return;` in this file silently did
   nothing - delete a profile, deploy a track, restore a backup, launch the
   game: all dead, with no error to explain it. This renders a real element,
   so it cannot be suppressed by the host.
   Returns a promise; call it as `if (!await ask(msg)) return;`. */
function ask(msg, okLabel) {
  return new Promise(resolve => {
    const veil = el('div', 'pk-veil ask-veil');
    const box = el('div', 'pk-box ask-box');
    const body = el('div', 'ask-msg');
    // the messages carry newlines and bullets - keep them readable
    body.textContent = String(msg == null ? '' : msg);
    const row = el('div', 'row ask-row');
    const no = el('button', 'sm', 'Cancel');
    const yes = el('button', 'sm primary', okLabel || 'OK');
    let done = false;
    const close = (val) => {
      if (done) return;
      done = true;
      removeEventListener('keydown', onKey, true);
      veil.remove();
      resolve(val);
    };
    const onKey = (e) => {
      if (e.key === 'Escape') { e.preventDefault(); close(false); }
      else if (e.key === 'Enter') { e.preventDefault(); close(true); }
    };
    no.onclick = () => close(false);
    yes.onclick = () => close(true);
    veil.onclick = (e) => { if (e.target === veil) close(false); };
    addEventListener('keydown', onKey, true);
    row.append(no, yes);
    box.append(body, row);
    veil.append(box);
    document.body.append(veil);
    yes.focus();
  });
}

function overwritePrompt(r) {
  const name = (r && (r.label || r.name)) || 'this';
  if (r && r.same) {
    return ask('"' + name + '" is already installed and these files '
      + 'are the same.\n\nOverwrite it anyway?');
  }
  return ask('"' + name + '" is already installed, but these files '
    + 'are different.\n\nOverwrite the installed copy?');
}

async function finishIngest(payload) {
  let r = await apiLong('drop', payload);
  if (r && r.need_confirm) {
    if (!await overwritePrompt(r)) {
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

/* ---- the bar the whole app shows ---------------------------------------
   Installing content is the one thing here that runs for minutes, and it was
   only visible on the page that started it: navigate away mid-download and
   there was nothing anywhere saying the app was busy, which is why a working
   install read as a hang.

   ⚠ OWNERSHIP. A window-drop drives this bar directly from the upload's own
   byte counter, which is better than anything the server can tell us about
   that phase. So the poller stands aside while a local job holds it, rather
   than the two writing over each other every second.                       */
let _progOwner = null;      // 'local' while a drop is uploading
let _progTimer = null;
let _progRate = { done: 0, at: 0, v: 0 };

function progClaim(who) { _progOwner = who; }
function progRelease(who) { if (_progOwner === who) _progOwner = null; }

// fed from the live snapshot (liveOn in startProgressWatch), not its own poll
function pollProgress(p) {
  if (_progOwner) return;                       // a drop is driving it
  const prog = dropProgress();
  if (!p || !p.active) {
    // ⚠ hide unconditionally, not only after a run this poller saw start. A
    // drop drives the bar itself and hands it back; if that job finished
    // between two ticks the bar would otherwise stay on screen for ever.
    pollProgress._was = false;
    prog.hide();
    return;
  }
  const done = p.done || 0, total = p.total || 0;
  const now = Date.now();
  if (!pollProgress._was) {
    pollProgress._was = true;
    // ⚠ Seed the baseline with what is ALREADY done, not zero. We usually
    // arrive part-way through - the first poll of a download that has moved
    // 100 MB would otherwise divide all of it by one interval and claim
    // 100 GB/s. No rate until there are two samples to compare.
    _progRate = { done, at: now, v: 0 };
    prog.show(p.phase === 'download' ? 'Downloading content…' : 'Installing…');
  }
  if (done > _progRate.done) {
    const inst = (done - _progRate.done) / Math.max(0.001, (now - _progRate.at) / 1000);
    _progRate.v = _progRate.v ? _progRate.v * 0.7 + inst * 0.3 : inst;
    _progRate.done = done; _progRate.at = now;
  }
  const left = (_progRate.v > 0 && total > done) ? (total - done) / _progRate.v : 0;
  const detail = [
    p.what,
    total ? `${mb(done)} of ${mb(total)}` : null,
    _progRate.v > 0 ? `${mb(_progRate.v)}/s` : null,
    left > 2 ? `${Math.ceil(left)}s left` : null,
  ].filter(Boolean).join(' · ');
  if (total) prog.set(done / total, detail);
  // no byte count for this phase - move the bar rather than invent a number
  else prog.busy(p.phase === 'download' ? 'Downloading content…' : 'Installing…',
                 p.what || '');
}

function startProgressWatch() {
  if (startProgressWatch._on) return;
  startProgressWatch._on = true;
  // ⚠ The bar used to run its own poller (0.7 s busy, 2.5 s idle). It now
  // reads the one live feed like everything else, so it cannot drift out of
  // step with the rest of the app or keep polling on its own.
  liveOn(s => pollProgress(s.progress));
}

/* Refresh NOW rather than on the next push. Without this the bar lagged
   behind the button that started the job, which reads as the click not
   having worked. */
function progKick() {
  liveKick();
}

function putFile(url, file, onProgress) {
  return new Promise(resolve => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', url);
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    // ⚠ Carry the admin token like api() does. Without this, dragging a car
    // or track onto a server managed over Tailscale is refused with 401 - the
    // one upload path that bypassed the shared helper and so bypassed auth.
    const _tok = adminToken();
    if (_tok) xhr.setRequestHeader('X-ACECM-Token', _tok);
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
  // the upload's own byte counter beats anything the server can report for
  // this phase, so hold the bar until the files are actually up
  progClaim('local');
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
    // which reads as a hang. Hand the bar back here: the upload is done, and
    // from now on only the server knows what is happening, which is exactly
    // what the global watcher reports. It also means the bar survives
    // navigating away mid-install.
    prog.busy('Installing…', 'unpacking and registering the content');
    progRelease('local');
    progKick();                     // hand straight over, no gap
    try {
      return await finishIngest({ id });
    } finally {
      // let the watcher paint the finished state rather than yanking the bar
      // out from under it
      progKick();
    }
  } finally {
    progRelease('local');
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

function shareTrackBtn(folder, label, on) {
  const b = el('button', on ? 'sm danger' : 'sm primary',
               on ? 'Stop sharing' : 'Share');
  b.onclick = async (ev) => {
    if (ev) ev.stopPropagation();
    const r = await api('share/track', {
      folder, share: !on, label: label || folder,
    });
    if (!(r && r.error)) {
      toast(on ? 'No longer shared' : (label || folder) + ' is now downloadable');
    }
    contentPage();
  };
  return b;
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
  const shareB = item.kind === 'track'
    ? shareTrackBtn(item.folder || item.name, item.label || item.name,
                    !!item.shared)
    : null;
  let link = null;
  if (item.shared && shareUrl) {
    link = el('button', 'sm', 'Link');
    link.title = 'Copy the Get content share URL';
    link.onclick = (ev) => {
      ev.stopPropagation();
      copyText(shareUrl, shareUrl);
    };
  }
  const rm = el('button', 'sm danger', 'Delete');
  rm.onclick = async (ev) => {
    ev.stopPropagation();
    const what = item.kind === 'track'
      ? `Delete track "${item.label}"?\n\nRemoves the imported files. `
        + 'Stock tracks are not touched.'
      : `Delete car "${item.name}" from client and server?`;
    if (!await ask(what)) return;
    const r = await api('library/remove',
                        { kind: item.kind, name: item.name });
    toast(r && r.ok ? 'Removed ' + item.name : (r.error || 'Delete failed'),
          !(r && r.ok));
    contentPage();
  };
  act.append(exp, copy, pathB);
  if (shareB) act.append(shareB);
  if (link) act.append(link);
  act.append(rm);
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

  // Fetching from a host goes FIRST. It was third, which is fine in the DOM
  // and useless on screen: the installed-mods card above it is 42 rows tall,
  // so anything after it is off the bottom of the page.
  p.append(fetchHostCard());
  // Then the library: install, export, delete, copy. Host/share stay below.
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
        if (!await ask(`Deploy "${t.display_name}" to the server?\n\n`
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
    if (!await ask('Restore the server archive from the backup taken before deploy?'))
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
    if (!await ask(`${n} track(s) missing from tracks.table:\n\n${names}\n\n`
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
    if (!await ask(`${n} track(s) missing from the client's tracks.table:\n\n`
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
  inp.dataset.keep = '1';   // survives the page rebuilding under you
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
