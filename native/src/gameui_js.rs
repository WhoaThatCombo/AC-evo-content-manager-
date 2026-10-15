//! The game-UI scripts, verbatim from acecm/gameui.py (generated - keep in step).

pub const BOOT: &str = r###"
(function(){
  var href = (location.href || '').toLowerCase();
  var path = '';
  try { path = ((ksUI.menuState.get() || {}).path || '').toLowerCase(); } catch (e) {}
  var car = !!(window.CurrentCar && CurrentCar.model && CurrentCar.model.name);
  var dest = '';
  try { dest = localStorage.getItem('loadingDestination') || ''; } catch (e) {}
  var cls = '';
  try { cls = String(document.body.className || ''); } catch (e) {}
  var paint = /(?:^|\s)paintshop(?:\s|$)/.test(cls) ? 'paint' : 'nopaint';
  var locked = cls.indexOf('ui-locked') >= 0 ? 'locked' : 'open';
  var page = 'other';
  if (href.indexOf('ingame.html') >= 0) page = 'ingame';
  else if (href.indexOf('singleplayer') >= 0) page = 'sp';
  else if (href.indexOf('multiplayer') >= 0) page = 'mp';
  else if (href.indexOf('menu.html') >= 0) page = car ? 'home' : 'menu-nocar';
  else if (href.indexOf('intro') >= 0) page = 'intro';
  return [page, path, car ? 'car' : 'nocar', dest, paint, locked].join('|');
})()
"###;

pub const GOTO: &str = r###"
(function(){
  if (!window.ksUI) return 'no-ksUI';
  if ((location.href||'').indexOf('singleplayer') >= 0) return 'already-sp';
  ksUI.goTo('singleplayer.html', 'singleplayer/main');
  return 'goto-sp';
})()
"###;

pub const MPH: &str = r###"
(function(){
  if (window.__acecmMph) { window.__acecmMph.on = true; return 'already'; }
  var S = window.__acecmMph = {on: true, timer: null};
  var K = 0.621371;
  function attach(){
    if (!S.on) return;
    var host = document.querySelector('.speed .value[data-bind-value]');
    if (!host) return;                                  // HUD not built yet
    var box = host.parentNode;
    if (box.querySelector('.acecm-mph')) return;         // still attached
    var mirror = host.cloneNode(false);
    mirror.className = host.className + ' acecm-mph';
    mirror.removeAttribute('data-bind-value');
    host.style.display = 'none';
    box.insertBefore(mirror, host.nextSibling);
    var unit = box.querySelector('.unit');
    if (unit) unit.textContent = 'mph';
    var paint = function(){
      var n = parseFloat((host.textContent || '').trim());
      mirror.textContent = isFinite(n) ? String(Math.round(n * K)) : '';
    };
    paint();
    new MutationObserver(paint).observe(
        host, {childList: true, characterData: true, subtree: true});
  }
  S.detach = function(){
    S.on = false;
    if (S.timer) { clearInterval(S.timer); S.timer = null; }
    var box = document.querySelector('.speed');
    if (!box) return;
    var m = box.querySelector('.acecm-mph');
    if (m) m.remove();
    var h = box.querySelector('.value[data-bind-value]');
    if (h) h.style.display = '';
    var u = box.querySelector('.unit');
    if (u) u.textContent = 'km/h';
  };
  attach();
  // Pitting, a respawn or a session restart rebuilds the HUD and takes our
  // element with it. Re-attach from INSIDE the page so the gap is half a
  // second rather than a round trip.
  //
  // A poll, deliberately, not a MutationObserver on the tree: this HUD
  // rewrites text every frame, so a subtree observer would fire constantly
  // for something that changes a handful of times per session. The early-out
  // above makes each tick two querySelectors.
  S.timer = setInterval(attach, 500);
  return 'mph-on';
})()
"###;

pub const MPH_OFF: &str = r###"
(function(){
  if (window.__acecmMph) { window.__acecmMph.detach(); return 'mph-off'; }
  var box = document.querySelector('.speed');
  if (!box) return 'no-speedo';
  var m = box.querySelector('.acecm-mph');
  if (m) m.remove();
  var host = box.querySelector('.value[data-bind-value]');
  if (host) host.style.display = '';
  var unit = box.querySelector('.unit');
  if (unit) unit.textContent = 'km/h';
  return 'mph-off';
})()
"###;

pub const BACK_TO_PIT: &str = r###"
(function(){
  if (typeof ksUI === 'undefined') return 'no-ksUI';
  try {
    ksUI.requestNoResponse('GameMode', 'BackToPit');
    return 'sent';
  } catch (e) { return 'error:' + String((e && (e.message || e)) || e); }
})()
"###;

pub const START: &str = r###"
(async function(){
  if ((location.href||'').indexOf('singleplayer') < 0) return 'not-on-sp';
  if (document.body && document.body.classList
      && document.body.classList.contains('paintshop')) return 'paintshop';
  if (!window.GAMEMODESELECTION) return 'no-GAMEMODESELECTION';
  try {
    if (window.ksUI && typeof ksUI.setOriginAsReturnPage === 'function')
      ksUI.setOriginAsReturnPage();
  } catch (e) {}
  try {
    await GAMEMODESELECTION.start();
    return 'started';
  } catch (e) {
    return 'fail:' + String((e && (e.response || e.message)) || e);
  }
})()
"###;

pub const WHERE: &str = r###"
(function(){
  var href = location.href || '';
  var path = '';
  try { path = (ksUI.menuState.get() || {}).path || ''; } catch (e) {}
  return path + '|' + href;
})()
"###;

pub const GOTO_MP: &str = r###"
(function(target){
  if (!window.ksUI) return 'no-ksUI';
  if ((location.href||'').indexOf('multiplayer') >= 0) return 'already-mp';
  if (target) {
    ksUI.goTo('multiplayer.html', 'multiplayer/serverlist/' + target);
    return 'goto-mp-target';
  }
  ksUI.goTo('multiplayer.html', 'main/serverlist');
  return 'goto-mp';
})
"###;

pub const JOIN_PUBLIC: &str = r###"
(function(want){
  var key = String(want.ip) + ':' + String(want.tcp);
  if (window.__acecmJoin === key) return 'already-sent';
  var page = document.querySelector('ks-page-serverlist');
  if (!page) return 'no-page';
  if (page.classList && page.classList.contains('loading')) return 'loading';
  var list = page.ServerList || [];
  if (!list.length) return 'waiting-list';
  /* ⚠ TWO PASSES, address first. This used to test address-then-id on each
     row in turn, so an EARLIER row whose server_id happened to match won over
     the LATER row that was the exact address. Hosting several servers puts
     them all on one ip differing only by port, which is precisely when ids
     collide or go stale - and you joined the wrong one. The address is the
     only thing that actually identifies a server; the id is a fallback for
     when we have no address at all. */
  var hit = null;
  for (var i = 0; i < list.length; i++) {
    var s = list[i];
    if (String(s.server_ip) === String(want.ip)
        && String(s.server_tcp_port) === String(want.tcp)) { hit = s; break; }
  }
  if (!hit && want.id) {
    for (var j = 0; j < list.length; j++) {
      if (String(list[j].server_id) === String(want.id)) { hit = list[j]; break; }
    }
  }
  if (!hit) return 'not-in-list:' + list.length;
  if (!window.CurrentCar || !CurrentCar.model) return 'no-car';
  try { page.setSelectedServer(hit.server_id); } catch (e) {}
  if (page.txtPassword) page.txtPassword.value = want.password || '';
  page.selectedCar = CurrentCar.model;
  page.selectedCarPguid = CurrentCar.model.car_pguid;
  if (!page.selectedServerId) return 'select-fail:' + hit.server_id;
  /* ⚠ Verify the page selected what we ASKED for before pressing Join.
     setSelectedServer can leave the page on a different row (the UI shows a
     highlighted server and "No server selected" at the same time), and
     connecting then joins whatever it happens to be sitting on. Refusing is
     right: the caller retries, and joining the wrong server is worse than
     not joining. */
  if (String(page.selectedServerId) !== String(hit.server_id)) {
    return 'select-mismatch:' + page.selectedServerId + ':' + hit.server_id;
  }
  /* ⚠ The game disables #btnJoin when the CURRENT CAR is not allowed on the
     selected server - that is the real meaning of "join did nothing" when a
     server restricts its car list. We connect through connectToServer rather
     than the button, so the button is not the mechanism here, only the
     signal: read it and say so plainly instead of retrying for 50s. Checked
     after the selection is verified, because an unselected row disables it
     too. #btnSpectate is the spectate equivalent. */
  var jb = document.querySelector('#btnJoin');
  if (jb && jb.hasAttribute('disabled')) {
    var cn = (window.CurrentCar && CurrentCar.model &&
              (CurrentCar.model.name || CurrentCar.model.display_name)) || '?';
    window.__acecmJoin = '';
    return 'car-not-eligible:' + cn;
  }
  window.__acecmJoin = key;
  try {
    page.connectToServer(false);
    return 'connect:' + hit.server_id + ':' + (hit.server_tcp_port || '')
           + ':' + (hit.server_name || '');
  } catch (e) {
    window.__acecmJoin = '';
    return 'connect-fail:' + String(e && e.message || e);
  }
})
"###;

pub const REFRESH_LIST: &str = r###"
(function(){
  var el = document.querySelector('ks-page-serverlist');
  if (el && typeof el.getServerList === 'function') {
    el.getServerList();
    return 'refresh';
  }
  if (!window.ksUI) return 'no-ksUI';
  ksUI.goTo('multiplayer.html', 'main/serverlist');
  return 'goto-mp';
})()
"###;

pub const QUIT: &str = r###"
(function(){
  if (!window.ksUI) return 'no-ksUI';
  try {
    // MP / loading pages ignore QuitGame. Home first, then quit.
    try { ksUI.goTo('menu.html', 'main/main'); } catch (e) {}
    ksUI.requestNoResponse('GameMode', 'QuitGame');
    return 'quit';
  } catch (e) {
    return 'fail:' + String(e && e.message || e);
  }
})()
"###;

pub const SELECT: &str = r###"
(async function(want){
  if (!window.VEHICLES) return 'no-VEHICLES';
  /* A game "model" is a TRIPLE: the car, its mechanical configuration (trim)
     and its visual preset (livery) - each pair carrying its own car_guid, and
     SetCurrentCar wants that exact pair. Matching on a flattened string of
     names picks whichever trim sorts first, which is how you ask for a Cup
     car and get the base one. Narrow in that order and only then widen:
        name + mech + visual  ->  name + mech  ->  name
     checking the top-level list and each model's __uimetadata.presets at
     every step, exactly as the game's own menu resolves it. */
  var norm = function(x){ return String(x == null ? '' : x).trim().toLowerCase(); };
  var model  = norm(want.model);
  var mech   = norm(want.mech);
  var visual = norm(want.visual || want.preset);
  function names(m){
    if (!m) return null;
    return {
      n: norm(m.name) || norm(m.display_name),
      c: norm(m.configuration && m.configuration.name),
      p: norm(m.preset && m.preset.name)
    };
  }
  function byName(m){
    var x = names(m);
    if (!x) return false;
    if (!model) return false;
    return x.n === model || (x.n && x.n.indexOf(model) >= 0);
  }
  function byMech(m){ var x = names(m); return byName(m) && (!mech || x.c === mech); }
  function byPair(m){ var x = names(m); return byMech(m) && (!visual || x.p === visual); }
  function presets(m){
    return (m && m.__uimetadata && Array.isArray(m.__uimetadata.presets))
      ? m.__uimetadata.presets : [];
  }
  var list = [];
  try { list = (await VEHICLES.getAllModels()) || []; } catch (e) { list = []; }
  function search(pred){
    var hit = list.find(pred);
    if (hit) return hit;
    for (var i = 0; i < list.length; i++) {
      var p = presets(list[i]).find(pred);
      if (p) return p;
    }
    return null;
  }
  var found = search(byPair) || search(byMech) || search(byName);
  /* last resort: the old loose match, so a guid or display-name still works */
  if (!found && (model || visual)) {
    var loose = function(m){
      var x = [m && m.name, m && m.display_name, m && m.car_guid].join(' ').toLowerCase();
      return (model && x.indexOf(model) >= 0) || (visual && x.indexOf(visual) >= 0);
    };
    found = search(loose);
  }
  if (!found && list.length)
    return 'not-found:' + list.length + ':' + norm(list[0] && list[0].name).slice(0, 60);
  if (!found && model) found = { name: want.model, display_name: want.model };
  if (!found) return 'not-found';
  try { await VEHICLES.setCurrent(found); }
  catch (e) { return 'set-fail:' + String(e && e.message || e); }
  var x = names(found) || {};
  var now = (window.CurrentCar && CurrentCar.model &&
             (CurrentCar.model.name || CurrentCar.model.display_name)) || '';
  return 'set:' + [x.n, x.c || '-', x.p || '-'].join('/') + ' now:' + now;
})
"###;

pub const SET_TRACK: &str = r###"
(async function(want){
  if (!window.GAMEMODESELECTION) return 'no-GAMEMODESELECTION';
  function req(cmd, payload){
    return new Promise(function(resolve){
      try {
        GAMEMODESELECTION.Client.request(cmd, payload || {}, function(r){
          resolve(r || {});
        });
      } catch (e) { resolve({error: String(e && e.message || e)}); }
    });
  }
  var mode = 'ClientCommandsMode_SINGLEPLAYER';
  function norm(x){ return String(x == null ? '' : x).trim().toLowerCase(); }
  var item = null, seen = 0;
  try {
    var tl = await req('TrackList', {mode: mode});
    var list = (tl && (tl.track_item || tl.track_items || tl.tracks)) || [];
    if (list && !Array.isArray(list)) list = [list];
    seen = list.length;
    for (var i = 0; i < list.length; i++) {
      var t = list[i] || {};
      if (norm(t.name) === norm(want.name) &&
          norm(t.layout) === norm(want.layout)) { item = t; break; }
    }
    if (!item) {
      for (var j = 0; j < list.length; j++) {
        var u = list[j] || {};
        if (norm(u.event_name) && norm(u.event_name) === norm(want.event_name)) {
          item = u; break;
        }
      }
    }
  } catch (e) {}
  if (!item) {
    item = {name: want.name, layout: want.layout,
            event_name: want.event_name || '',
            track_length: want.track_length | 0,
            is_enabled: true, nation: '', continent: ''};
  }
  var r = await req('SetTrack', {mode: mode, track_item: item});
  var resp = (r && (r.response || r.result)) || 'sent';
  var now = '';
  try {
    var c = await req('CurrentTrack', {mode: mode});
    var ci = c && c.track_item;
    if (ci) now = String(ci.name || '') + '/' + String(ci.layout || '');
  } catch (e) {}
  return 'set:' + resp + ' now:' + now + ' from:' + seen +
         (item.image_path ? ' (game item)' : ' (built)');
})
"###;

pub const CURRENT_TRACK: &str = r###"
(async function(){
  if (!window.GAMEMODESELECTION) return '';
  return await new Promise(function(resolve){
    try {
      GAMEMODESELECTION.Client.request('CurrentTrack',
        {mode: 'ClientCommandsMode_SINGLEPLAYER'}, function(r){
          var t = r && r.track_item;
          resolve(t ? String(t.name || '') + '/' + String(t.layout || '') : '');
        });
    } catch (e) { resolve(''); }
  });
})
"###;

pub const CONDITIONS: &str = r###"
(async function(want){
  if (!window.GAMEMODESELECTION) return 'no-GAMEMODESELECTION';
  function req(cmd, payload){
    return new Promise(function(resolve){
      GAMEMODESELECTION.Client.request(cmd, payload || {}, function(r){
        resolve(r || {});
      });
    });
  }
  var wtype = 'GameModeSelectionWeatherType_' + String(want.weather || 'CLEAR').toUpperCase();
  var hour = want.hour | 0;
  var minute = want.minute | 0;
  var mode = String(want.mode || '');
  var data;
  try {
    data = await GAMEMODESELECTION.getGameModeParameters();
  } catch (e) {
    return 'params-fail:' + String(e && (e.message || e.response) || e);
  }
  var typeRes = 'same';
  if (mode && data && data.type !== mode) {
    var tr = await req('SetGameType', {
      mode: 'ClientCommandsMode_SINGLEPLAYER',
      type: mode
    });
    typeRes = (tr && tr.response) || 'set';
    try { data = await GAMEMODESELECTION.getGameModeParameters(); }
    catch (e) { return 'params2-fail:' + String(e && e.message || e); }
  }
  var list = (data && data.weather_data) || [];
  var wd = null;
  for (var i = 0; i < list.length; i++) {
    if (list[i].type === wtype) { wd = list[i]; break; }
  }
  var wres = wd ? 'no-client' : 'no-preset:' + list.length;
  if (wd && GAMEMODESELECTION.Client) {
    var wr = await req('SetWeather', { weather_data: wd });
    wres = (wr && wr.response) || 'ok';
  }
  var sessions = (GAMEMODESELECTION.Current && GAMEMODESELECTION.Current.sessions)
              || (data && data.sessions) || [];
  var s0 = sessions[0];
  for (var j = 0; j < sessions.length; j++) {
    if (!sessions[j].time_of_day) sessions[j].time_of_day = {};
    sessions[j].time_of_day.hour = hour;
    sessions[j].time_of_day.minute = minute;
  }
  if (s0 && want.opponents != null) {
    s0.num_opponents = want.opponents | 0;
    s0.min_strength = want.min_strength | 0;
    s0.max_strength = want.max_strength | 0;
    s0.aggressivness = want.aggressiveness || 'Safe';
    s0.grid_type = 'GameModeSelectionGridType_AUTO';
    s0.single_make = !!want.single_make;
    if (want.starting_position)
      s0.starting_position = want.starting_position | 0;
  }
  if (sessions.length >= 4 && want.practice_min != null) {
    sessions[0].duration = (want.practice_min | 0) * 60;
    sessions[0].duration_type = 'GameModeSelectionDuration_TIME';
    sessions[1].duration = (want.quali_min | 0) * 60;
    sessions[1].duration_type = 'GameModeSelectionDuration_TIME';
    sessions[2].duration = (want.warmup_min | 0) * 60;
    sessions[2].duration_type = 'GameModeSelectionDuration_TIME';
    sessions[3].duration = want.race_laps | 0;
    sessions[3].duration_type = 'GameModeSelectionDuration_LAPS';
  } else if (s0 && want.duration_min != null && !want.opponents) {
    s0.duration = (want.duration_min | 0) * 60;
    s0.duration_type = 'GameModeSelectionDuration_TIME';
  } else if (s0 && want.race_laps != null && want.opponents != null
             && sessions.length < 4) {
    s0.duration = want.race_laps | 0;
    s0.duration_type = 'GameModeSelectionDuration_LAPS';
  }
  try {
    await GAMEMODESELECTION.save(data && data.mode, sessions);
  } catch (e) {
    return 'save-fail:' + String(e && (e.message || e.response) || e)
         + ' weather:' + wres;
  }
  var nowW = (window.CurrentGameMode && CurrentGameMode.weather_type) || '';
  var nowT = '';
  var nowAi = '';
  try {
    var cur = GAMEMODESELECTION.Current.sessions[0];
    nowT = String(cur.time_of_day.hour) + ':' + String(cur.time_of_day.minute);
    nowAi = String(cur.num_opponents) + '@' + cur.min_strength + '-' + cur.max_strength;
  } catch (e) {}
  return 'mode:' + ((data && data.type) || mode) + ' setType:' + typeRes
       + ' weather:' + wtype + ' via:' + wres + ' now:' + nowW
       + ' tod:' + nowT + ' ai:' + nowAi;
})
"###;
