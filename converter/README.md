# aceconv

Converts Assetto Corsa (AC1) cars and tracks into Assetto Corsa EVO mods. It is the converter behind ACECM, the Assetto Corsa EVO content manager, and runs on its own as a command-line tool.

Status: foundation plus the track base. The package format, texture encoder, FMOD bank writer, donor builder, validator and the stock statistics tool are in place. `aceconv track` writes a loadable track (geometry, collision surfaces, spawns, timing, declaration rows) with minimal materials; the full track look, AI lines and pits, lighting bakes and vegetation, and the car stages are written next.

A converted track is installed by ACECM, which also applies its declaration rows (`ui\track_decl.json`) to the game's track tables: the converter itself never writes the game's files.

## What it needs

- Assetto Corsa EVO installed (the converter reads `content.kspkg` and `fmod.dll`; it never writes the install).
- The AC1 car or track to convert.

Nothing from either game is shipped: the donor car, the material templates, the sound template and the Vorbis tables are all built on the user's machine from the user's own game files.

## Build

```
cargo build --release
```

Windows MSVC; the binary links the C runtime statically (`.cargo/config.toml`).

## Commands

```
aceconv stockstats [content.kspkg] [out dir]        measure stock content -> stockstats.json
aceconv stockstats compare <car.kspkg | track dir> <stockstats.json>
aceconv track <ac1 track dir> <out dir> [--id <id>]  AC1 track -> <out>\<id>\ (+ ui\track_decl.json)
aceconv track-scan <ac1 track dir>...                 layouts, meshes, spawns, AI lines of AC1 tracks
aceconv check-car <car.kspkg>  /  check-track <track dir>
aceconv make-donor  /  make-fmod-template  /  fmodbank <project> <out>
aceconv unpack / pack / slim / pack-share / unpack-share
aceconv car <ac1 car dir> <out dir> [--id <id>] [--donor <zip>] [--refmats <dir>] [--content <content.kspkg>]
aceconv car-frame <ac1 car dir>                      the car's EVO frame (datum, wheel centres, CoG) as json
aceconv sound <ac1 car dir> <out dir> [id]           AC1 car sounds -> the car's own EVO banks (as inside `car`)
aceconv acd <ac1 car dir> [file...]
aceconv car-id <folder>  /  track-id <folder>  /  legacy-id <mods dir> <cars|tracks> <ac1 folder>
```

`aceconv` with no arguments prints the full list.

## How results are checked

Not by comparing with any other tool's output. A conversion is checked by the validator (`check-car`, `check-track`), against the statistics of the game's own content (`stockstats compare`), with the evoview viewer as a black-box checker, and in the game.

## Documentation

- [docs/FORMATS.md](docs/FORMATS.md): the formats, with where each fact comes from.
- [docs/FMOD_BANK.md](docs/FMOD_BANK.md): FMOD Studio 2.03 bank layout, AC1's FMOD Studio 1.08 banks, the car sound stage.
- `tools/fmodrender.py`, `tools/soundcmp.py`: offline renders through the games' own FMOD runtimes (no audio device).
- [CREDITS.md](CREDITS.md).

## Development

Enable the repository hooks once: `git config core.hooksPath .githooks`.

## Licence

GPL-3.0-or-later (see LICENSE). Not affiliated with Kunos Simulazioni, 505 Games or Firelight Technologies.
