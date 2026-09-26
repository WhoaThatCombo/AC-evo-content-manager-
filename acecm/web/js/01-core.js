/* Assetto Corsa EVO Content Manager - UI

   The UI was one 6,600-line app.js. It is now these ordered classic
   scripts, loaded by index.html in number order, sharing ONE global scope:

     01-core        helpers, api(), toast, the live feed, job steps, strip
     02-drive       Play
     03-shared      small shared painters + the attention banner
     04-servers     Host > Servers (and the server editor)
     05-cars        Content > Cars
     06-tracks      Content > Tracks
     07-backend     Diagnostics > Lobby proxy
     08-settings    Settings
     09-content     Content > Install & share, drag-drop, the progress bar
     10-patches     binary patches page (not in the nav)
     11-gamesettings Settings > Game settings
     12-remote      fetching content from other people's servers
     13-telemetry   live map page (unfinished; not in the nav)
     14-logs        Diagnostics > Logs
     15-nav         pages, sections, router, themes - and boot (keep LAST)

   ⚠ Not ES modules, on purpose: pages call each other by bare name and the
   router rebinds them (keepPlace), which module scope would break. A
   top-level `const`/`let` is visible to every later file, so names must stay
   unique across all of them - a duplicate is a load-time SyntaxError. */
const $ = (s, r) => (r || document).querySelector(s);
const el = (t, c, h) => { const e = document.createElement(t);
  if (c) e.className = c; if (h != null) e.innerHTML = h; return e; };
const esc = s => String(s ?? '').replace(/[&<>"]/g,
  c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

/* Admin token, for an ACECM being managed from another machine.
   Taken from ?token=... the first time (so a pasted link just works), then
   kept so later navigations do not ask again. Locally there is no token and
   none is needed - the server lets loopback straight through. */
const TOKEN_KEY = 'acecm_token';
function adminToken() {
  try {
    const q = new URLSearchParams(location.search).get('token');
    if (q) {
      localStorage.setItem(TOKEN_KEY, q);
      // keep it out of the address bar and out of the back/forward history
      history.replaceState(null, '', location.pathname);
      return q;
    }
    return localStorage.getItem(TOKEN_KEY) || '';
  } catch (e) { return ''; }
}
function askForToken() {
  const t = prompt('This ACECM is managed remotely. Paste its admin token:');
  if (!t) return false;
  try { localStorage.setItem(TOKEN_KEY, t.trim()); } catch (e) {}
  return true;
}
async function api(path, body) {
  const opt = body ? { method: 'POST', body: JSON.stringify(body) } : {};
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), 12000);
  const tok = adminToken();
  const headers = tok ? { 'X-ACECM-Token': tok } : {};
  try {
    const r = await fetch('/api/' + path,
                          { ...opt, headers, signal: ctrl.signal });
    const j = await r.json().catch(() => ({ error: 'bad response' }));
    /* 401 means the token is missing or stale. Ask once and retry, rather
       than showing the same error on every panel of the page. */
    if (r.status === 401 && j && j.need_token) {
      if (askForToken()) return api(path, body);
      return j;
    }
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

/* ----------------------------------------------------------- live feed --
   ONE source of live state for the whole app: the Join/Start job and its
   steps, downloads/installs, compression, the lobby proxy and your servers.

   Pages used to poll a dozen endpoints on timers they each armed and
   disarmed themselves. A slow endpoint, or a timer that never re-armed, read
   as random flakiness - the dead Drive poll and the 12 s aborts were both
   that. Now the server pushes one snapshot over /api/events and every page
   subscribes to it.

   liveOn(fn)   - for app-wide listeners (activity strip, progress bar)
   livePage(fn) - for the current page; dropped automatically by go()
   liveKick()   - fetch a snapshot NOW (after starting something)

   If the stream dies, or never delivers (a proxy buffering it), the watchdog
   falls back to fetching snapshots, so nothing depends on SSE working. */
const live = { snap: null, subs: new Set(), es: null, lastAt: 0, skew: 0,
               started: false };
// keyed, because pages rebuild themselves: a second drivePage() must REPLACE
// its subscription, not add another one that also repaints the old nodes
const _pageSubs = new Map();
function liveOn(fn) {
  live.subs.add(fn);
  if (live.snap) { try { fn(live.snap); } catch (e) { console.warn('live', e); } }
  return () => live.subs.delete(fn);
}
function livePage(key, fn) {
  const old = _pageSubs.get(key);
  if (old) old();
  const off = liveOn(fn);
  _pageSubs.set(key, off);
  return off;
}
function livePageClear() {
  for (const off of _pageSubs.values()) { try { off(); } catch (e) {} }
  _pageSubs.clear();
}
// repaint from the last snapshot (elapsed times) without claiming the
// stream is alive - liveEmit's lastAt is what the watchdog trusts
function liveTick() {
  if (!live.snap) return;
  for (const f of [...live.subs]) {
    try { f(live.snap); } catch (e) { console.warn('live subscriber', e); }
  }
}
// the server's clock, so elapsed times do not depend on the PC agreeing
function liveNow() { return Date.now() / 1000 + live.skew; }
function liveEmit(s) {
  if (!s || !s.ok) return;
  live.snap = s;
  live.lastAt = Date.now();
  if (s.drive && s.drive.now) live.skew = s.drive.now - Date.now() / 1000;
  for (const f of [...live.subs]) {
    // ⚠ one broken subscriber must not starve the rest - and must be loud
    try { f(s); } catch (e) { console.warn('live subscriber', e); }
  }
}
async function liveKick() {
  try {
    const tok = adminToken();
    const r = await fetch('/api/events/snapshot?fresh=1',
                          { headers: tok ? { 'X-ACECM-Token': tok } : {} });
    liveEmit(await r.json());
  } catch (e) {}
}
/* Watch one background job to its end, from the live feed.
   pick(snap) -> that job's state object; running(state) -> still going?
   onDone(state) runs ONCE, only after the job was seen running, so a
   snapshot left over from the previous run cannot end this one early. */
function liveWatch(pick, running, onDone, onTick) {
  let seen = false, ended = false, off = null, first = null;
  off = liveOn(s => {
    if (ended) return;
    const st = pick(s) || {};
    if (running(st)) { seen = true; if (onTick) onTick(st); return; }
    // ⚠ a job quicker than one feed tick is never SEEN running - but its
    // finished state differs from the one we started with, so that counts
    const now = JSON.stringify(st);
    if (first === null) { first = now; if (!seen) return; }
    if (!seen && now === first) return;
    ended = true;
    setTimeout(() => off && off(), 0);
    onDone(st);
  });
  liveKick();
  return () => { ended = true; if (off) off(); };
}
function liveStart() {
  if (live.started) return;
  live.started = true;
  // EventSource cannot send headers. The server already accepts the token as
  // a cookie, which keeps it out of the stream's URL.
  const tok = adminToken();
  if (tok && /^[\w.~-]+$/.test(tok)) {
    try { document.cookie = 'acecm_token=' + tok + '; path=/; SameSite=Strict'; }
    catch (e) {}
  }
  if (typeof EventSource === 'function') {
    try {
      live.es = new EventSource('/api/events');
      live.es.onmessage = ev => {
        try { liveEmit(JSON.parse(ev.data)); } catch (e) {}
      };
    } catch (e) { live.es = null; }
  }
  // the server sends at least every 5 s; silence for 6 s means fall back
  setInterval(() => { if (Date.now() - live.lastAt > 6000) liveKick(); }, 3000);
  setInterval(liveTick, 1000);
  liveOn(paintActivity);
  liveKick();
}

/* ---- the Join / Start job, described once for every page ------------- */
const JOB_TERMINAL = ['idle', 'launched', 'failed', 'cancelled'];
const JOB_STEP = {
  writing: 'Write the session',
  entering: 'Check the running game',
  launching_game: 'Launch the game',
  starting_backend: 'Start the lobby proxy',
  starting_server: 'Start your server',
  waiting_for_menu: 'Wait for the main menu',
  waiting_for_session: 'Wait for the session',
  selecting_car: 'Set your car',
  selecting_track: 'Set the track',
  setting_conditions: 'Set weather, time and mode',
  opening_sp: 'Open Single Player',
  starting_session: 'Start the session',
  joining: 'Join the server',
  capturing_list: 'Capture the public server list',
  quitting_game: 'Close the game',
};
// what usually comes next, so the list can show the road ahead dimmed
const JOB_PLAN = {
  sp: ['writing', 'launching_game', 'waiting_for_menu', 'starting_session'],
  local: ['starting_backend', 'starting_server', 'launching_game',
          'waiting_for_menu', 'selecting_car', 'joining'],
  server: ['launching_game', 'waiting_for_menu', 'selecting_car', 'joining'],
  capture: ['capturing_list', 'quitting_game'],
};
const JOB_KIND = { sp: 'Single player', local: 'Joining your server',
                   server: 'Joining', capture: 'Refreshing the server list' };
function jobBusy(d) { return !!d && !JOB_TERMINAL.includes(d.phase || 'idle'); }
function fmtSecs(s) {
  s = Math.max(0, Math.round(s));
  return s < 60 ? s + 's' : Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
}
// 45 s on one step is where "slow" turns into "probably stuck"
const JOB_SLOW = 45;

/* ---- activity strip: what is happening, on every page ----------------- */
let _actFailSeen = 0;
function paintActivity(s) {
  const box = $('#activity');
  if (!box) return;
  const chips = [];
  const d = s.drive || {};
  const steps = d.steps || [];
  const cur = steps[steps.length - 1];
  if (jobBusy(d) && _page !== 'drive') {
    const t = cur ? liveNow() - cur.at : 0;
    chips.push({ cls: t > JOB_SLOW ? 'warn' : 'busy', go: 'drive',
      text: `${JOB_KIND[d.kind] || 'Working'} · `
        + `${JOB_STEP[d.phase] || d.hint || d.phase} · ${fmtSecs(t)}` });
  } else if (d.phase === 'failed' && d.started > _actFailSeen
             && _page !== 'drive') {
    chips.push({ cls: 'bad', go: 'drive', dismiss: () => { _actFailSeen = d.started; },
      text: `${JOB_KIND[d.kind] || 'Drive'} failed: ${d.fault || 'see Drive'}` });
  }
  const c = s.compress || {};
  if (c.active) chips.push({ cls: 'busy', go: 'settings',
    text: (c.undo ? 'Uncompressing' : 'Compressing') + ' the mods folder…' });
  for (const [k, what, page] of [['thumbs', 'Rendering car pictures', 'cars'],
                                 ['covers', 'Decoding track covers', 'tracks']]) {
    const j = s[k] || {};
    if (j.state === 'running') chips.push({ cls: 'busy', go: page,
      text: `${what} ${j.done || 0}/${j.total || '?'}` });
  }
  const px = s.proxy || {};
  if (d.game_running && px.up === false) {
    chips.push({ cls: 'bad', text: 'Lobby proxy is down — the in-game server '
      + 'list will be empty', action: ['Start it', 'backend/ensure'] });
  } else if (d.game_running && px.up && px.ours === false) {
    chips.push({ cls: 'warn', text: 'The lobby proxy is serving another '
      + 'ACECM\'s servers', action: ['Use mine', 'backend/ensure'] });
  }
  const up = (s.servers || []).filter(v => v.running);
  if (up.length) chips.push({ cls: 'ok', go: 'servers',
    text: up.length === 1 ? `${up[0].name || 'Server'} is running`
                          : `${up.length} servers running` });
  // ⚠ rebuilt only when the text changes - this runs every second
  const key = JSON.stringify(chips.map(c => [c.cls, c.text]));
  if (box.dataset.key === key) return;
  box.dataset.key = key;
  box.innerHTML = '';
  box.hidden = !chips.length;
  for (const c of chips) {
    const b = el('div', 'act ' + c.cls);
    b.append(el('span', 'actdot'), el('span', 'acttext', esc(c.text)));
    if (c.go) { b.classList.add('link'); b.onclick = () => go(c.go); }
    if (c.action) {
      const a = el('button', 'mini', esc(c.action[0]));
      a.onclick = async ev => {
        ev.stopPropagation();
        a.disabled = true;
        const r = await api(c.action[1], {});
        toast(r.ok ? 'Lobby proxy started' : (r.error || 'failed'), !r.ok);
        liveKick();
      };
      b.append(a);
    }
    if (c.dismiss) {
      const x = el('button', 'mini ghost', '×');
      x.title = 'Dismiss';
      x.onclick = ev => { ev.stopPropagation(); c.dismiss(); paintActivity(live.snap); };
      b.append(x);
    }
    box.append(b);
  }
}

/* ---- the Join / Start checklist ----------------------------------------
   The job used to show one line of text. When it stalled you saw "waiting for
   the menu" and nothing else - not how long, not what came before, not what
   was next. This lists every step it has been through with its time, the one
   it is on, and the ones still ahead, and turns red on the step that failed.
   opts: { onRetry, onStop } */
function paintJobSteps(box, d, opts) {
  opts = opts || {};
  const steps = (d && d.steps) || [];
  // a finished job is kept on screen for 10 minutes, then the list goes away
  const last = steps[steps.length - 1];
  const stale = !jobBusy(d) && last && liveNow() - last.at > 600;
  if (!steps.length || stale || d.phase === 'idle') {
    if (box.dataset.key !== '') { box.dataset.key = ''; box.innerHTML = ''; }
    box.hidden = true;
    return;
  }
  box.hidden = false;
  const busy = jobBusy(d);
  const work = steps.filter(x => !JOB_TERMINAL.includes(x.phase));
  const endAt = busy ? liveNow()
    : (last && JOB_TERMINAL.includes(last.phase) ? last.at : liveNow());
  const rows = work.map((x, i) => {
    const next = work[i + 1];
    const until = next ? next.at : endAt;
    const isLast = i === work.length - 1;
    let state = 'done';
    if (isLast && busy) state = 'now';
    else if (isLast && d.phase === 'failed') state = 'bad';
    else if (isLast && d.phase === 'cancelled') state = 'stop';
    return { state, label: JOB_STEP[x.phase] || x.phase, hint: x.hint || '',
             secs: until - x.at };
  });
  // the road ahead, dimmed - only while it is still moving
  const plan = JOB_PLAN[d.kind] || [];
  if (busy) {
    const seen = new Set(work.map(x => x.phase));
    const at = plan.indexOf(d.phase);
    const lastSeen = Math.max(-1, ...work.map(x => plan.indexOf(x.phase)));
    plan.slice(Math.max(at, lastSeen) + 1)
      .filter(p => !seen.has(p))
      .forEach(p => rows.push({ state: 'todo', label: JOB_STEP[p] || p,
                                hint: '', secs: null }));
  }
  const key = JSON.stringify([rows.map(r => [r.state, r.label, r.hint,
                               r.secs == null ? null : Math.round(r.secs)]),
                              d.phase, d.fault, d.hint, d.stopping]);
  if (box.dataset.key === key) return;
  box.dataset.key = key;
  box.innerHTML = '';
  const head = el('div', 'jobhead');
  head.append(el('b', null, esc(JOB_KIND[d.kind] || 'Drive')));
  const tot = work.length ? endAt - work[0].at : 0;
  head.append(el('span', 'tiny dim', fmtSecs(tot)));
  box.append(head);
  const ICON = { done: '✓', now: '', bad: '✕', stop: '■', todo: '' };
  for (const r of rows) {
    const row = el('div', 'jobstep ' + r.state);
    const slow = r.state === 'now' && r.secs > JOB_SLOW;
    if (slow) row.classList.add('slow');
    row.append(el('span', 'jobicon', ICON[r.state]),
               el('span', 'joblabel', esc(r.label)
                 + (r.hint && r.state !== 'todo'
                    ? `<span class="tiny dim"> · ${esc(r.hint)}</span>` : '')),
               el('span', 'jobtime tiny', r.secs == null ? '' : fmtSecs(r.secs)));
    box.append(row);
  }
  const cur = rows.find(r => r.state === 'now');
  if (cur && cur.secs > JOB_SLOW) {
    box.append(el('div', 'jobnote warn',
      'This step is taking longer than usual. If the game is sitting on a '
      + 'menu, Stop and try again.'));
  }
  if (d.phase === 'failed') {
    box.append(el('div', 'jobnote bad', esc(d.fault || 'It stopped without saying why.')));
  } else if (d.phase === 'launched') {
    box.append(el('div', 'jobnote ok', esc(d.hint || 'Done')));
  } else if (d.phase === 'cancelled') {
    box.append(el('div', 'jobnote', 'Stopped.'));
  }
  const btns = el('div', 'row jobbtns');
  if (busy && opts.onStop) {
    const b = el('button', 'mini', d.stopping ? 'Stopping…' : 'Stop');
    b.disabled = !!d.stopping;
    b.onclick = () => { b.disabled = true; opts.onStop(); };
    btns.append(b);
  }
  if ((d.phase === 'failed' || d.phase === 'cancelled') && opts.onRetry) {
    const b = el('button', 'mini primary', 'Try again');
    b.onclick = () => opts.onRetry();
    btns.append(b);
  }
  if (d.phase === 'failed') {
    const b = el('button', 'mini ghost', 'Open logs');
    b.onclick = () => go('logs');
    btns.append(b);
  }
  if (btns.childNodes.length) box.append(btns);
}
