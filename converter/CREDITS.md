# Credits

## Games and formats

- **Kunos Simulazioni**: Assetto Corsa and Assetto Corsa EVO, whose file formats this tool reads and writes. Assetto Corsa and Assetto Corsa EVO are trademarks of Kunos Simulazioni / 505 Games. No game content is included in this repository; everything is read from the user's own installation.
- **Firelight Technologies**: FMOD and FMOD Studio. FMOD is a trademark of Firelight Technologies. The FMOD bank and FSB5 notes come from FMOD's own tools and the game's `fmod.dll` / `fmodstudio.dll`; the Vorbis setup headers are read at run time from the user's `fmod.dll`, and none are shipped.

## Specifications

- Xiph.Org: the Vorbis I specification (identification header layout).
- Glenn Fowler, Landon Curt Noll, Kiem-Phong Vo: the FNV-1a hash (public domain).
- Bob Jenkins: lookup3 `hashlittle2` (public domain).
- ISO 3166-1 country codes and the UN M49 regions (`data/countries.tsv`).
- The IANA time zone database (`iso3166.tab`, `zone1970.tab`; public domain).

## Libraries (Rust crates)

| crate | licence | use |
|---|---|---|
| meshopt (meshoptimizer by Arseny Kapoulkine) | MIT / Apache-2.0 | mesh simplification |
| intel_tex_2 (Intel ISPC Texture Compressor) | MIT / Apache-2.0 | BC1 / BC4 / BC6H / BC7 encoding |
| texture2ddecoder | MIT / Apache-2.0 | DDS decoding |
| image | MIT / Apache-2.0 | PNG / JPEG / TGA decoding |
| lewton | MIT / Apache-2.0 | Vorbis decoding |
| vorbis_rs (libvorbis, aoTuV) | BSD-3-Clause | optional Vorbis encoding |
| liblzma (xz) | MIT / Apache-2.0 | share packages |
| zstd, zstd-safe, zstd-sys (Zstandard by Meta) | MIT, BSD-3-Clause | share packages (optional codec) |
| flate2 | MIT / Apache-2.0 | zlib (dynamic-track tiles) |
| zip | MIT | donor archive |
| md-5 | MIT / Apache-2.0 | content digests |
| memmap2, memchr, rayon, regex, serde, serde_json, indexmap, rustc-hash, anyhow, libloading | MIT / Apache-2.0 / Unlicense / ISC | general |
