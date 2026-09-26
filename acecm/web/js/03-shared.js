/* ACECM UI - shared. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* ------------------------------------------------------------- shared -- */
/* _pill and _setText are shared: they write only when a value actually
   differs, which is what keeps a refresh from touching the DOM needlessly.
   They outlived the dashboard that introduced them. */
function _pill(node, on, text, offClass) {
  const cls = 'pill ' + (on ? 'on' : (offClass || 'off'));
  if (node.className !== cls) node.className = cls;
  const want = '<i class="dot"></i>' + esc(text);
  if (node.innerHTML !== want) node.innerHTML = want;
}

function _setText(node, text) {
  const s = String(text);
  if (node.textContent !== s) node.textContent = s;
}

/* ---- what needs attention ----------------------------------------------
   The one thing the dashboard did that nothing else did: say what is wrong.
   It lived on a page nobody landed on, so a broken server dir or an occupied
   port stayed invisible until you went looking. It is now a strip under the
   header on EVERY page - silent when there is nothing to say. */
let _attnKey = '';
async function refreshAttention() {
  const host = $('#attention');
  if (!host) return;
  const ov = await api('overview');
  // ⚠ PROBLEMS only. `attention` also carries level:"info" tips, and this
  // strip is on every page permanently - a standing tip rendered as a box
  // would be a warning that never goes away, which is how people learn to
  // ignore the strip entirely. Info items are explanatory, not actionable.
  const att = ((ov && ov.attention) || []).filter(a => a.level !== 'info');
  const key = JSON.stringify(att.map(a => [a.level, a.what, a.do]));
  if (key === _attnKey) return;          // nothing changed - leave the DOM be
  _attnKey = key;
  host.innerHTML = '';
  att.forEach(a => {
    const box = el('div', a.level === 'bad' ? 'err' : 'warn');
    box.innerHTML = '<b>' + esc(a.what) + '</b> ' + esc(a.do);
    host.append(box);
  });
}
