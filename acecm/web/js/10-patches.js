/* ACECM UI - patches. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* -------------------------------------------------------------- patches -- */
async function patchesPage() {
  const p = $('#page');
  p.innerHTML = '';
  const d = await api('patches');

  const intro = el('div', 'card');
  intro.innerHTML = '<h2>Binary patches</h2>'
    + '<div class="tiny dim">Patches are data, not scripts: each declares the '
    + 'build it was made for and the exact bytes it expects. Applying verifies '
    + '<b>every</b> site first and writes nothing unless all match, so a '
    + 'half-applied patch cannot happen. A patch made for another build refuses '
    + 'to apply rather than corrupting the file.</div>';
  p.append(intro);

  const STATE = {
    applied:      ['on',   'applied'],
    clean:        ['off',  'not applied'],
    mismatch:     ['bad',  'bytes unrecognised — will not write'],
    'wrong-build':['warn', 'built for a different game version'],
    missing:      ['bad',  'target file not found'],
  };

  (d.patches || []).forEach(pt => {
    const st = pt.status || {};
    const [cls, label] = STATE[st.state] || ['off', st.state || '?'];
    const c = el('div', 'card');
    c.innerHTML = `<div class="row"><div class="grow">
        <b>${esc(pt.title || pt.id)}</b>
        <div class="tiny dim">${esc(pt.description || '')}</div>
        <div class="tiny dim">${esc(pt.target || '')}<br>
        ${st.sites || 0} site(s) &middot; built for ${esc((pt.build_md5||'any').slice(0,12))}
        &middot; now ${esc((st.md5||'?').slice(0,12))}</div></div>
        <span class="pill ${cls}"><i class="dot"></i>${esc(label)}</span></div>`;
    const row = el('div', 'row wrap');
    row.style.marginTop = '10px';
    const ap = el('button', 'primary sm', 'Apply');
    ap.disabled = st.state === 'applied' || st.state === 'mismatch'
                  || st.state === 'missing';
    ap.onclick = async () => {
      const r = await api('patches/apply', { id: pt.id });
      toast(r.ok ? 'Applied — backup kept' : (r.error || 'Failed'), !r.ok);
      patchesPage();
    };
    const rs = el('button', 'sm danger', 'Restore');
    rs.disabled = st.state === 'clean' || st.state === 'missing';
    rs.onclick = async () => {
      if (!await ask('Restore the original bytes?')) return;
      const r = await api('patches/restore', { id: pt.id });
      toast(r.ok ? 'Restored' : (r.error || 'Failed'), !r.ok);
      patchesPage();
    };
    row.append(ap, rs);
    if (st.state === 'wrong-build') {
      const f = el('button', 'sm', 'Apply anyway');
      f.title = 'Offsets almost certainly do not match this build';
      f.onclick = async () => {
        if (!await ask('This patch was built for a different game version.\n'
            + 'Offsets will very likely be wrong. Continue?')) return;
        const r = await api('patches/apply', { id: pt.id, force: true });
        toast(r.ok ? 'Applied (forced)' : (r.error || 'Failed'), !r.ok);
        patchesPage();
      };
      row.append(f);
    }
    c.append(row);
    p.append(c);
  });
  if (!(d.patches || []).length)
    p.append(el('div', 'card').appendChild(
      el('div', 'empty', 'No patches registered yet')).parentElement);

  // --- what the binary offers a patch author ------------------------------
  const insp = await api('patches/inspect');
  if (insp.ok) {
    const ic = el('div', 'card');
    ic.innerHTML = `<h2>Patch space &middot; ${esc(insp.md5.slice(0,12))}</h2>`
      + '<div class="tiny dim" style="margin-bottom:9px">'
      + 'Code caves are int3 padding in executable sections. Slack is the free '
      + 'page tail after a writable section — <b>not</b> the BSS gap, which '
      + 'holds the program’s own globals and must never be written.</div>';
    (insp.code_caves || []).forEach(c => ic.append(el('div', 'chk',
      `<span class="name">cave ${esc(c.va)} <span class="dim">${esc(c.section)}</span></span>`
      + `<span class="pill off">${Math.round(c.size/1024)} KB</span>`)));
    (insp.bss_slack || []).forEach(s => ic.append(el('div', 'chk',
      `<span class="name">slack ${esc(s.va)} <span class="dim">${esc(s.section)}</span></span>`
      + `<span class="pill off">${s.size} B</span>`)));
    p.append(ic);
  }
}
