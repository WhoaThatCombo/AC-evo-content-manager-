/* ACECM UI - logs. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* ---------------------------------------------------------------- logs --- */
let logTimer = null, logFollow = true;
async function logsPage() {
  const p = $('#page');
  p.innerHTML = '';
  const card = el('div', 'card');
  card.innerHTML = '<h2>Logs</h2>'
    + '<div class="tiny dim">Everything ACECM does, plus full tracebacks for '
    + 'any error. Newest at the bottom.</div>';
  const row = el('div', 'row wrap');
  row.style.margin = '10px 0';
  const pick = el('select');
  const follow = el('button', 'sm primary', 'Following');
  const refresh = el('button', 'sm', 'Refresh');
  const openDir = el('button', 'sm', 'Show folder');
  const info = el('span', 'tiny dim grow');
  row.append(pick, refresh, follow, openDir, info);
  const pre = el('pre', 'log');
  pre.style.maxHeight = '62vh';
  card.append(row, pre);
  p.append(card);

  const files = await api('logs/files');
  (files.files || []).forEach(f => {
    const o = document.createElement('option');
    o.value = f.name;
    o.textContent = `${f.name}  (${(f.size / 1024).toFixed(0)} KB)`;
    pick.append(o);
  });
  info.textContent = files.dir || '';
  openDir.onclick = () => { navigator.clipboard.writeText(files.dir || '');
    toast('Log folder path copied'); };
  follow.onclick = () => { logFollow = !logFollow;
    follow.textContent = logFollow ? 'Following' : 'Paused';
    follow.className = logFollow ? 'sm primary' : 'sm'; };

  async function pull() {
    const r = await api('logs?lines=500&file=' + encodeURIComponent(pick.value || 'acecm.log'));
    if (!r.ok) { pre.textContent = r.error || 'no log yet'; return; }
    const atBottom = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 40;
    pre.textContent = (r.lines || []).join(String.fromCharCode(10));
    // Colourless but scannable: keep the view pinned unless the user scrolled up.
    if (logFollow && atBottom) pre.scrollTop = pre.scrollHeight;
  }
  refresh.onclick = pull;
  pick.onchange = pull;
  await pull();
  logTimer = setInterval(() => { if (logFollow) pull(); }, 3000);
}
