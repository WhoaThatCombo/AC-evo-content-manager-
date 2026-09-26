/* ACECM UI - nav. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* ----------------------------------------------------------------- nav --- */
const PAGES = {
  drive: ['Drive', 'Single player or a local server — same car picker', drivePage],
  servers: ['Servers', 'Create, configure and run dedicated servers', serversPage],
  cars: ['Cars', 'What the dedicated server can actually load', carsPage],
  tracks: ['Tracks', 'Layouts available to host', tracksPage],
  content: ['Content', 'Install, export and manage cars and tracks', contentPage],
  backend: ['Backend', 'Host through our own lobby', backendPage],
  gamesettings: ['Game settings', 'FFB, graphics, audio and bindings', gameSettingsPage],
  logs: ['Logs', 'What ACECM did, and every error in full', logsPage],
  settings: ['Settings', 'Paths and ports', settingsPage],
  // ⚠ Live map (telemetryPage, 13-telemetry.js) is deliberately NOT here:
  // telemetry is unfinished. The Servers page renders no telemetry controls
  // either, so nothing links to it - add it back here AND to Host below.
};
/* ---- sections: what you are doing, not how it works ----------------------
   The bar used to list pages - Drive, Servers, Cars, Tracks, Content, with
   Backend and Logs tucked into Settings. It is grouped by task now: Play,
   Host, Content, with Settings and Diagnostics small on the right. A section
   with more than one page gets a sub-tab row. Page names (and so #links and
   saved bookmarks) are unchanged.                                          */
const SECTIONS = [
  { id: 'play', label: 'Play', pages: [['drive', 'Play']] },
  { id: 'host', label: 'Host', pages: [['servers', 'Servers']] },
  { id: 'content', label: 'Content', pages: [['content', 'Install & share'],
                                             ['cars', 'Cars'],
                                             ['tracks', 'Tracks']] },
];
const SIDE_SECTIONS = [
  { id: 'settings', label: 'Settings', pages: [['settings', 'Settings'],
                                               ['gamesettings', 'Game settings']] },
  // the lobby proxy is plumbing: it lives here, and the activity strip
  // speaks up on every page when it is actually down
  { id: 'diagnostics', label: 'Diagnostics', pages: [['backend', 'Lobby proxy'],
                                                     ['logs', 'Logs']] },
];
// friendly aliases for links people type or share
const PAGE_ALIAS = { play: 'drive', host: 'servers', diagnostics: 'backend' };
function sectionOf(page) {
  return [...SECTIONS, ...SIDE_SECTIONS].find(sec =>
    sec.pages.some(([n]) => n === page)) || SECTIONS[0];
}
// the page you were last on inside each section, so clicking Content takes
// you back to Cars if that is where you were
const _lastInSection = {};
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
/* ⚠ #page is what scrolls (overflow:auto; body is overflow:hidden), NOT the
   document. This used to track document.scrollingElement, whose scrollTop
   is always 0 here - so "keep the scroll position" never did anything. */
const scroller = () => $('#page') || document.scrollingElement;
const _scrollPos = {};
let _page = '';
// scroll does not bubble, so listen in the capture phase for #page's own
addEventListener('scroll', e => {
  if (_page && e.target && e.target.id === 'page')
    _scrollPos[_page] = e.target.scrollTop;
}, { passive: true, capture: true });

// ⚠ One render at a time, per page. Every page function clears #page and then
// awaits its data, so two overlapping calls both clear an empty page and then
// both append - and the whole dashboard appears TWICE. It only became visible
// when a page grew an extra await, but the race was always there.
const _rendering = {};
// the page the user last asked for, so a late render cannot overwrite it
let _wanted = '';

/* ---- rebuild without losing your place ----------------------------------
   Pages rebuild themselves wholesale after most actions (~65 call sites):
   clear #page, await data, build it again. That flashed blank on every
   refresh, and threw away whatever you were in the middle of - the filter
   you had typed, the section you had opened, where a list was scrolled to,
   the box you were typing in.

   So a SELF-refresh is double-buffered, for every page at once: the rebuild
   goes into a fresh #page laid out off-screen, the old one stays visible,
   and when the new one is ready it inherits the old one's filters, open
   sections, focus and inner scroll positions before being swapped in.

   ⚠ Only FILTER boxes are carried over, never form fields. A rebuild after
   Save shows what the server now holds; putting back what was typed would
   hide a value the server rejected or normalised.                          */
// filter/search boxes, plus anything a page opts in with data-keep="1"
// (an address or path you are typing that is not a saved setting)
const FILTER_INPUT = 'input[type=search], input[placeholder*="ilter" i], '
  + 'input[placeholder*="earch" i], input[data-keep], textarea[data-keep]';
function uiKey(n) {
  // stable enough across a rebuild: which card it is in, what it is
  const card = n.closest('.card, .pane, .drive-col');
  const head = card && card.querySelector('h2, h3');
  const sum = n.tagName === 'DETAILS' && n.querySelector('summary');
  const what = n.id || n.getAttribute('name') || n.getAttribute('placeholder')
    || n.getAttribute('aria-label') || (sum && sum.textContent)
    || n.className || '';
  return [(head && head.textContent || '').trim().slice(0, 40), n.tagName,
          n.type || '', String(what).trim().slice(0, 60)].join('|');
}
function keyed(nodes) {
  const seen = {};
  return nodes.map(n => {
    const k = uiKey(n);
    seen[k] = (seen[k] || 0) + 1;
    return [k + '#' + seen[k], n];
  });
}
function captureUi(root) {
  const st = { inputs: {}, details: {}, scroll: {}, focus: null };
  for (const [k, n] of keyed([...root.querySelectorAll(FILTER_INPUT)]))
    if (n.value) st.inputs[k] = n.value;
  for (const [k, n] of keyed([...root.querySelectorAll('details')]))
    st.details[k] = n.open;
  const scrolled = [...root.querySelectorAll('*')].filter(n => n.scrollTop > 0);
  for (const [k, n] of keyed(scrolled)) st.scroll[k] = n.scrollTop;
  const a = document.activeElement;
  if (a && root.contains(a) && a.matches('input, textarea, select')) {
    const hit = keyed([...root.querySelectorAll(a.tagName)]).find(([, n]) => n === a);
    let s = null, e = null;
    try { s = a.selectionStart; e = a.selectionEnd; } catch (x) {}
    if (hit) st.focus = { k: hit[0], tag: a.tagName, s, e };
  }
  return st;
}
function restoreUi(root, st) {
  for (const [k, n] of keyed([...root.querySelectorAll(FILTER_INPUT)])) {
    const v = st.inputs[k];
    // only into a box the rebuild left empty - never over a real value
    if (v && !n.value) {
      n.value = v;
      n.dispatchEvent(new Event('input', { bubbles: true }));
    }
  }
  for (const [k, n] of keyed([...root.querySelectorAll('details')]))
    if (k in st.details && n.open !== st.details[k]) n.open = st.details[k];
  // inner scroll after layout, or there is nothing to scroll yet
  requestAnimationFrame(() => {
    const cands = [...root.querySelectorAll('*')]
      .filter(n => n.scrollHeight > n.clientHeight + 1);
    for (const [k, n] of keyed(cands)) if (st.scroll[k]) n.scrollTop = st.scroll[k];
  });
  if (st.focus) {
    const hit = keyed([...root.querySelectorAll(st.focus.tag)])
      .find(([k]) => k === st.focus.k);
    if (hit) {
      const n = hit[1];
      try {
        n.focus({ preventScroll: true });
        if (st.focus.s != null && n.setSelectionRange) n.setSelectionRange(st.focus.s, st.focus.e);
      } catch (e) {}
    }
  }
}
let _buffer = null;
function bufferBegin() {
  const old = $('#page');
  if (!old || !old.childNodes.length) return null;
  const r = old.getBoundingClientRect();
  const neu = old.cloneNode(false);
  // laid out for real (so pages that measure themselves get real sizes),
  // just not visible or clickable until it is ready
  neu.style.cssText = `position:fixed;left:${r.left}px;top:${r.top}px;`
    + `width:${r.width}px;visibility:hidden;pointer-events:none;z-index:-1`;
  old.id = 'page-old';
  old.after(neu);
  // ⚠ old.scrollTop, not scroller(): #page is already the NEW node here
  _buffer = { old, neu, state: captureUi(old), y: old.scrollTop };
  return _buffer;
}
function bufferEnd(buf, ok) {
  if (!buf || buf.done) return;
  buf.done = true;
  if (_buffer === buf) _buffer = null;
  // ⚠ a page that returned early without drawing anything must not blank
  // the screen - keep what was there
  if (!ok || !buf.neu.childNodes.length) {
    buf.neu.remove();
    buf.old.id = 'page';
    return;
  }
  // ⚠ visible FIRST: focus() is refused inside visibility:hidden. All of
  // this runs in one task, so nothing paints between the swap and restore.
  buf.neu.style.cssText = '';
  buf.old.remove();
  restoreUi(buf.neu, buf.state);
  // ⚠ Parts of a page land AFTER its function returns (Drive's server list,
  // thumbnails), so at swap time it can be too short and the browser clamps
  // the scroll. Keep re-applying for a moment as it grows - and stop the
  // instant the user scrolls, since that is no longer our position to hold.
  const neu = buf.neu, y = buf.y;
  neu.scrollTop = y;
  if (neu.scrollTop < y - 1 && typeof ResizeObserver === 'function') {
    // re-apply whenever the page grows, for up to 5 s, unless the user acts
    const ro = new ResizeObserver(() => {
      if (!neu.isConnected) return stop();
      neu.scrollTop = y;
      if (neu.scrollTop >= y - 1) stop();
    });
    const mine = () => stop();
    const stop = () => {
      ro.disconnect();
      clearTimeout(t);
      ['wheel', 'keydown', 'pointerdown', 'touchstart'].forEach(
        ev => neu.removeEventListener(ev, mine, true));
    };
    ['wheel', 'keydown', 'pointerdown', 'touchstart'].forEach(
      ev => neu.addEventListener(ev, mine, true));
    const t = setTimeout(stop, 5000);
    for (const c of neu.children) ro.observe(c);
  }
}
// navigating away mid-refresh: drop the half-built page, keep the real one
function bufferAbandon() {
  if (_buffer) bufferEnd(_buffer, false);
}

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
    // a page refreshing ITSELF builds off-screen and keeps your place
    const buf = _page === name ? bufferBegin() : null;
    let ok = false;
    try {
      const r = await fn.apply(this, args);
      ok = true;
      return r;
    } finally {
      bufferEnd(buf, ok && _wanted === name);
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
  name = PAGE_ALIAS[name] || name;
  bufferAbandon();
  if (typeof telTimer !== 'undefined' && telTimer) { clearInterval(telTimer); telTimer = null; }
  if (typeof telRaf !== 'undefined' && telRaf) { cancelAnimationFrame(telRaf); telRaf = null; }
  // page-level live subscriptions belong to the page being left
  livePageClear();
  if (name !== 'servers') stopAllLogFollows();
  _wanted = PAGES[name] ? name : 'drive';
  const [title, sub, fn] = PAGES[name] || PAGES.drive;
  $('#ttl').textContent = title;
  $('#sub').textContent = sub;
  paintNav(name);
  // ⚠ Only blank when the page is actually changing. Blanking on a refresh is
  // what makes the content flick away and come back.
  if (name !== _page) {
    // ⚠ A FRESH container, not the same one blanked. A page still rendering
    // when you leave it holds a reference to its #page and appends to it
    // after its await - that used to land on the page you had just opened
    // (switch quickly and each page briefly showed the previous one). Now it
    // lands in a detached node nobody can see.
    const old = $('#page');
    const fresh = old.cloneNode(false);
    fresh.innerHTML = '<div class="empty">Loading…</div>';
    old.replaceWith(fresh);
  }
  fn().then(() => { const p = $('#page'); if (p) p.dataset.booted = '1'; })
    .catch(e => { $('#page').innerHTML = ''; toast(String(e), true); });
  location.hash = name;
  refreshAttention();
}
/* The section links are built from SECTIONS rather than written out in the
   HTML, so adding a page cannot leave the nav out of step with it. */
function buildSections() {
  const main = $('#sections'), side = $('#sections2');
  main.innerHTML = ''; side.innerHTML = '';
  const add = (box, sec) => {
    const a = el('a', null, esc(sec.label));
    a.dataset.section = sec.id;
    a.onclick = () => go(_lastInSection[sec.id] || sec.pages[0][0]);
    box.append(a);
  };
  SECTIONS.forEach(sec => add(main, sec));
  SIDE_SECTIONS.forEach(sec => add(side, sec));
}
function paintNav(page) {
  const sec = sectionOf(page);
  _lastInSection[sec.id] = page;
  document.querySelectorAll('#sections a, #sections2 a').forEach(a =>
    a.classList.toggle('on', a.dataset.section === sec.id));
  const sub = $('#subnav');
  if (!sub) return;
  sub.innerHTML = '';
  sub.hidden = sec.pages.length < 2;
  for (const [n, label] of sec.pages) {
    const a = el('a', n === page ? 'on' : null, esc(label));
    a.onclick = () => go(n);
    sub.append(a);
  }
}
buildSections();
/* Back / Forward. go() sets the hash; a hash we did not set (the buttons, a
   pasted link) navigates. Without this the app ignored both. */
addEventListener('hashchange', () => {
  const want = PAGE_ALIAS[location.hash.slice(1)] || location.hash.slice(1);
  if (want && PAGES[want] && want !== _wanted) go(want);
});

/* ------------------------------------------------------------- themes ---
   Accent-only, and stored per browser profile. index.html applies the saved
   theme before first paint so there is no flash of the default colour. */
const THEMES = [
  ['teal', '#2ee6c8'], ['blue', '#4c8dff'], ['purple', '#a78bfa'],
  ['red', '#ff5c7a'], ['orange', '#ff9d47'], ['green', '#3fb950'],
  ['pink', '#f472b6'], ['amber', '#ffd166'],
];
function currentTheme() {
  try { return localStorage.getItem('acecm.theme') || 'teal'; } catch (e) { return 'teal'; }
}
function setTheme(name) {
  // teal is the stylesheet default, so it is the ABSENCE of data-theme
  if (name === 'teal') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = name;
  try { localStorage.setItem('acecm.theme', name); } catch (e) {}
  // ⚠ and in config, so it survives the profile wipe an update performs
  api('config', { ui_theme: name }).catch(() => {});
  paintThemeMenu();
}
function closeThemeMenu() {
  const m = $('#thememenu');
  if (!m) return;
  m.hidden = true;
  m.classList.remove('open');
}
function paintThemeMenu() {
  const m = $('#thememenu');
  if (!m) return;
  m.innerHTML = '';
  const cur = currentTheme();
  THEMES.forEach(([name, swatch]) => {
    const b = el('button', name === cur ? 'on' : null);
    b.style.background = swatch;
    b.title = name;
    b.onclick = (e) => {
      e.stopPropagation();
      setTheme(name);
      closeThemeMenu();
    };
    m.append(b);
  });
}
function bindThemes() {
  const btn = $('#themebtn'), m = $('#thememenu');
  if (!btn || !m) return;
  btn.onclick = (e) => {
    e.stopPropagation();
    if (m.classList.contains('open')) { closeThemeMenu(); return; }
    paintThemeMenu();
    const r = btn.getBoundingClientRect();
    m.style.top = (r.bottom + 8) + 'px';
    m.style.right = Math.max(8, innerWidth - r.right) + 'px';
    m.hidden = false;
    m.classList.add('open');
  };
  /* ⚠ CAPTURE phase, and more than one event name. Bubble-phase
     pointerdown works in a plain browser tab but not reliably in the desktop
     WebView shell - anything downstream that stops propagation (or a shell
     that delivers mouse without pointer events) swallowed the dismiss, and
     the menu then stayed open until the app was restarted. Capture sees the
     event before any handler can eat it. */
  const away = (e) => {
    if (!m.classList.contains('open')) return;
    if (m.contains(e.target) || btn.contains(e.target)) return;
    closeThemeMenu();
  };
  ['pointerdown', 'mousedown', 'click', 'touchstart'].forEach(
    ev => addEventListener(ev, away, true));
  // the menu is fixed-positioned against the button, so anything that moves
  // it out from under the cursor should close it rather than leave it adrift
  ['resize', 'blur'].forEach(ev => addEventListener(ev, closeThemeMenu));
  addEventListener('scroll', closeThemeMenu, true);
  addEventListener('keydown', (e) => {
    if (e.key === 'Escape') closeThemeMenu();
  }, true);
}
bindThemes();
{ const b = $('#brand'); if (b) b.onclick = () => go('drive'); }

// the live feed first, so the first page subscribes to a running stream
liveStart();
go((location.hash || '#drive').slice(1));
// problems are worth noticing wherever you are, but they do not change
// often - a slow tick is plenty and costs one small request.
setInterval(refreshAttention, 20000);
// installing content is the one job that runs for minutes - show it wherever
// the user happens to be, not only on the page that started it
startProgressWatch();
bindDropAnywhere();
// ⚠ nothing writes #navfoot any more - the element is gone. A missing server
// exe still surfaces through refreshAttention(), which is the place for it.
