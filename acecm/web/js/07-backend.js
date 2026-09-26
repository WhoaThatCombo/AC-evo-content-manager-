/* ACECM UI - backend. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* -------------------------------------------------------------- backend -- */
async function backendPage() {
  const b = await api('backend');
  const p = $('#page');
  p.innerHTML = '';

  const c = el('div', 'card');
  c.innerHTML = '<h2>Own lobby backend</h2>'
    + '<div class="tiny dim" style="margin-bottom:12px">'
    + 'EVO has no direct connect: the client asks a lobby to resolve a server '
    + 'and authorise the join. So we run the lobby. <b>Proxy</b> relays to Kunos '
    + 'and appends our servers, keeping your account and the public list. '
    + '<b>Standalone</b> replaces it entirely, fully offline.</div>';
  const row = el('div', 'row wrap');
  const proxy = el('button', 'primary', 'Start proxy');
  proxy.onclick = async () => { const r = await api('backend/start', { mode: 'proxy' }); if (r.ok) toast('Proxy up on :' + b.port); backendPage(); };
  const alone = el('button', null, 'Start standalone');
  alone.onclick = async () => { const r = await api('backend/start', { mode: 'standalone' }); if (r.ok) toast('Standalone backend up'); backendPage(); };
  const stop = el('button', 'danger', 'Stop');
  stop.onclick = async () => { await api('backend/stop', {}); toast('Backend stopped'); backendPage(); };
  row.append(proxy, alone, stop);
  c.append(row);
  c.append(el('div', 'row wrap', `<span class="pill ${b.listening ? 'on' : 'off'}">`
    + `<i class="dot"></i>port ${b.port} ${b.listening ? 'listening' : 'closed'}</span>`
    + `<span class="pill ${b.have_cert ? 'on' : 'warn'}"><i class="dot"></i>`
    + `${b.have_cert ? 'TLS keypair present' : 'no TLS keypair — run backend/gencert.sh'}</span>`
    + `<span class="pill ${b.client_patched ? 'on' : 'off'}"><i class="dot"></i>`
    + `${b.client_patched ? 'rdata → local backend' : 'rdata still Kunos'}</span>`
    + `<span class="pill ${b.inspector_patched ? 'on' : 'off'}"><i class="dot"></i>`
    + `${b.inspector_patched ? 'menu inspector :' + (b.inspector_port || 9444)
                             : 'menu inspector off — Drive cannot Start'}</span>`));

  const seen = (b.game_backend || {}).url;
  if (seen) {
    const local = /localhost|127\.0\.0\.1/.test(seen);
    c.append(el('div', local ? 'tiny dim' : 'warn',
      local
        ? `Game log: talking to <code>${esc(seen)}</code>`
        : `Game log: still talking to <code>${esc(seen)}</code> — `
          + 'Steam dropped <code>-backend=</code>. Close the game and Launch '
          + 'from ACECM so the lobby URL is rewritten.'));
  }

  const cu = b.client_url || {};
  const red = el('div');
  red.style.marginTop = '12px';
  red.innerHTML = '<div class="tiny dim" style="margin-bottom:8px">'
    + '<b>Required on a fresh install:</b> rewrite the lobby URL in the exe '
    + 'and turn on the Gameface inspector. Steam relaunches the game with no '
    + 'arguments (<code>Arguments: 1</code>), so <code>-backend=</code> never '
    + 'arrives, and stock EVO leaves the inspector off — Drive then launches '
    + 'and sits on the home menu. Launch from ACECM does both rewrites if the '
    + 'game is closed, and starts Steam first if it is not already up. '
    + `Intended: <code>${esc(b.launch_backend || cu.intended || '')}</code>`
    + (cu.slot ? ` &middot; slot ${esc(cu.slot)}` : '')
    + '</div>';
  const rrow = el('div', 'row wrap');
  const ap = el('button', b.client_patched ? '' : 'primary', 'Point client at us');
  ap.onclick = async () => {
    const r = await api('backend/redirect', { action: 'apply' });
    toast(r.ok ? (r.already ? 'Already pointed at us' : 'Client URL patched')
               : (r.error || 'Patch failed'), !r.ok);
    backendPage();
  };
  const rs = el('button', null, 'Restore Kunos URL');
  rs.onclick = async () => {
    if (!await ask('Put the official lobby URL back in the client?')) return;
    const r = await api('backend/redirect', { action: 'restore' });
    toast(r.ok ? (r.already ? 'Already on Kunos' : 'Restored official URL')
               : (r.error || 'Restore failed'), !r.ok);
    backendPage();
  };
  rrow.append(ap, rs);
  red.append(rrow);
  c.append(red);
  p.append(c);

  const l = el('div', 'card');
  l.innerHTML = '<h2>Backend log</h2>';
  const pre = el('pre', 'log', '(nothing yet)');
  l.append(pre);
  p.append(l);
  const r = await api('backend/log?mode=proxy');
  pre.textContent = (r.lines || []).join('\n') || '(nothing yet)';
  pre.scrollTop = pre.scrollHeight;
}
