"""Turn a verify_cars sweep into one self-contained HTML gallery.

    python tools/build_gallery.py verify_out gallery.html

Images are embedded as data URIs so the page works anywhere (and satisfies the
artifact CSP, which blocks every external request).
"""
import base64
import io as _io
import json
import os
import sys

from PIL import Image


def thumb(png, width=460, quality=72):
    im = Image.open(png).convert("RGB")
    if im.width > width:
        im = im.resize((width, round(im.height * width / im.width)), Image.LANCZOS)
    buf = _io.BytesIO()
    im.save(buf, "JPEG", quality=quality, optimize=True)
    return base64.b64encode(buf.getvalue()).decode()


def main(src, out):
    rows = json.load(open(os.path.join(src, "results.json"), encoding="utf-8"))
    rows.sort(key=lambda r: (r["mod"], r["label"]))
    ok = sum(1 for r in rows if r.get("ok"))
    mods = sum(1 for r in rows if r["mod"])
    tris = sum(r.get("tris", 0) for r in rows)

    cards = []
    for r in rows:
        png = os.path.join(src, r["id"] + ".png")
        if not os.path.isfile(png):
            continue
        b64 = thumb(png)
        issues = "; ".join(r.get("flags") or []) or "clean"
        cls = "bad" if r.get("flags") else "good"
        cards.append(f'''<figure>
  <img loading="lazy" src="data:image/jpeg;base64,{b64}" alt="{r['label']}">
  <figcaption>
    <b>{r['label']}</b>{' <span class="pill">mod</span>' if r['mod'] else ''}
    <span class="id">{r['id']}</span>
    <span class="stat">{r.get('tris', 0):,} tris · {r.get('rims', 0)} rims ·
      {r.get('tyres', 0)} tyres · {r.get('anims', 0)} anims ·
      {r.get('bones', 0)} bones</span>
    <span class="{cls}">{issues}</span>
  </figcaption>
</figure>''')

    html = f"""<title>AC EVO car viewer - every car</title>
<style>
 :root {{ color-scheme: light dark; --bg:#0e1013; --fg:#e8eaed; --dim:#9aa0a6;
          --card:#16191d; --line:#262b31; --good:#5cc98b; --bad:#e5a13c; }}
 @media (prefers-color-scheme: light) {{
   :root {{ --bg:#f6f7f9; --fg:#16191d; --dim:#5f6368; --card:#fff;
            --line:#e2e5e9; --good:#1a7f4b; --bad:#a35c00; }} }}
 :root[data-theme="dark"] {{ --bg:#0e1013; --fg:#e8eaed; --dim:#9aa0a6;
   --card:#16191d; --line:#262b31; --good:#5cc98b; --bad:#e5a13c; }}
 :root[data-theme="light"] {{ --bg:#f6f7f9; --fg:#16191d; --dim:#5f6368;
   --card:#fff; --line:#e2e5e9; --good:#1a7f4b; --bad:#a35c00; }}
 body {{ margin:0; padding:24px; background:var(--bg); color:var(--fg);
   font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif; }}
 h1 {{ font-size:22px; margin:0 0 4px; }}
 .sub {{ color:var(--dim); margin-bottom:20px; }}
 .grid {{ display:grid; gap:16px;
   grid-template-columns:repeat(auto-fill,minmax(300px,1fr)); }}
 figure {{ margin:0; background:var(--card); border:1px solid var(--line);
   border-radius:10px; overflow:hidden; }}
 img {{ width:100%; display:block; background:#000; }}
 figcaption {{ padding:10px 12px; display:flex; flex-direction:column; gap:2px; }}
 .id {{ color:var(--dim); font-size:12px; font-family:ui-monospace,monospace; }}
 .stat {{ color:var(--dim); font-size:12px; }}
 .good {{ color:var(--good); font-size:12px; }}
 .bad {{ color:var(--bad); font-size:12px; }}
 .pill {{ background:var(--bad); color:#000; border-radius:4px; padding:1px 6px;
   font-size:11px; margin-left:4px; }}
</style>
<h1>Every car in the viewer</h1>
<div class="sub">{ok}/{len(rows)} rendered · {mods} mods · {tris:,} triangles total ·
 read straight from the .kspkg, nothing extracted</div>
<div class="grid">
{''.join(cards)}
</div>
"""
    open(out, "w", encoding="utf-8").write(html)
    print(f"{out}  {os.path.getsize(out)/1e6:.1f} MB, {len(cards)} cars")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else "gallery.html")
