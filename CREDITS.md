# Credits

ACECM is free software under the GNU General Public License, version 3 or
later (LICENSE.txt). It is not affiliated with Kunos Simulazioni or 505 Games.
Assetto Corsa and Assetto Corsa EVO are their trademarks. No game content is
included: everything is read from the user's own installation.

## Bundled programs

- **aceconv** (`converter/`): the Assetto Corsa -> Assetto Corsa EVO car and
  track converter, GPL-3.0-or-later. Its own credits are in
  `converter/CREDITS.md`.
- **evoview** (`tools/evoview.exe`): the 3D car and track viewer, by the same
  author, built from its own repository.

## Libraries (Rust crates)

The app and the converter are built from crates.io crates whose licences are
all compatible with the GPL-3.0: by count, mostly MIT and/or Apache-2.0, then
BSD-2/3-Clause, ISC, Zlib, Unlicense, Unicode-3.0, BSL-1.0, CC0-1.0,
CDLA-Permissive-2.0 (the webpki-roots certificate data) and MPL-2.0
(cssparser, selectors: file-level copyleft, compatible with GPL-3.0 through
its secondary-licence clause). `cargo metadata` in `native/` and
`converter/` lists every crate with its licence.

Notable ones:

| crate | licence | use |
|---|---|---|
| tauri, wry, tao | MIT / Apache-2.0 | the window (WebView2) |
| axum, hyper, tokio, reqwest | MIT | the local server and downloads |
| rustls, ring, webpki-roots | Apache-2.0 / ISC / MIT, CDLA-Permissive-2.0 | TLS |
| prost, prost-reflect | Apache-2.0 | the game's protobuf messages |
| ed25519-dalek, curve25519-dalek | BSD-3-Clause | update signatures |
| image, png, texture2ddecoder | MIT / Apache-2.0 | pictures |
| zip, tar, flate2 | MIT / Apache-2.0 | archives |
| iced-x86 | MIT | game binary inspection |
| intel_tex_2 (ISPC Texture Compressor), meshopt | MIT / Apache-2.0 | converter: textures, meshes |
| liblzma (xz), zstd (libzstd) | MIT / Apache-2.0, BSD-3-Clause | converter: share packages |
| vorbis_rs (libvorbis, aoTuV), lewton | BSD-3-Clause, MIT / Apache-2.0 | converter: car sounds |
