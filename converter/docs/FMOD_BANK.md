# FMOD Studio 2.03 bank format (FEV, FMT version 0x92)

Writer notes for `src/sound/bankwrite` and `src/sound/bankread`, worked out by building projects with FMOD's own command-line tool (fmodstudiocl 2.03.13) and comparing its output field by field, and by reading how the runtime (fmodstudio.dll) loads them. Section numbers are referred to from the code.


## 0. Determinism
fmodstudiocl output is byte-identical across rebuilds and project paths. No timestamps, no randomness.

## 1. Container
* RIFF LE: `RIFF <size> 'FEV '`, chunks `<id4><u32 size><body>`, odd sizes padded with one zero byte (not counted).
  `LIST <size> <type4>` holds children.
* `FMT ` = u32 0x92, u32 0x92.
* `LIST PROJ` = `BNKI`, then the object lists in fixed order: IBSS GBSS RBSS PRTS MBSS BEFX PEFX SEFX SCFX SSFX VCAS EVTS
  TLNS PMLS PRMS CTRS CRVS MPGS MUIS SPIS PRIS EVIS WAIS EFIS CMDS SLNS LWVS WAVS SNAS MODS, then SNDH STDT STBL HASH DEL MUTE
  REFI PLAT. Empty list = `LIST <4> <type>` (no LCNT). Non-empty: `LCNT {u32 n}` + n items.
* Items: most are `LIST <ITEM>` wrappers (GBUS, BEFF, EVNT, TMLN, PMLO, PARM, WAIT, MUIT, VCA , SNAP, MODU ...);
  CTRS/CRVS/MPGS/WAVS items are bare leaf chunks (CTRL, CURV, MAP , WAV ).
* Every object list is sorted by GUID compared as the raw 16 bytes (bytes_le memcmp).
* After PROJ: `SND ` = zero padding so the FSB5 starts 32-byte aligned in the file, then the FSB5. `SNDH` =
  array{1 elem, size 8}: u32 FSB5 file offset, u32 FSB5 size. File ends at the FSB end.
* BNKI: guid bank, u32 0, u32 0, u32 events+snapshots, u32 flags 7.
* Strings bank: same container, BNKI guid = bank ^ {981d20d4-717b-4c42-8649-8afef1f30698}, 0 events, all lists empty,
  SNDH empty, STDT = trie, HASH = empty array (`01 00`), PLAT empty.

## 2. Primitive encodings
* GUID: 16 bytes Windows layout. vcount: u16, if bit15 set a second u16 carries bits 15..30.
* Array: vcount `n<<1 | same`; same=1: one u16 element size then n elements; same=0: u16 size before each element.
  The writer uses same=1 for POD element types (also when empty: `01 00`), same=0 for strings / string pairs / code
  lists / markers (empty: `00 00`).
* Sub-block: u16 size + fields. String: vcount length + bytes.
* code (trigger conditions): u32 byte length + u32-tagged entries. `0x11 <GUID>` = parameter; `tag 0x20 f32 lo f32 hi`
  range, tag 0x5220 = upper bound exclusive, 0x5020 = inclusive; `0x1002` AND joining two conditions; `0x30` ends a
  multi-condition list. Sheet regions: [start, start+length) with 0x5220 unless start+length >= parameter maximum
  (end computed in double, rounded to f32). ParameterCondition: continuous [min,max] 0x5020; discrete/labeled
  parameters [min, max+1) 0x5220.
* Effect params (PMEF): u32 n, per param u32 type (0 f32, 1 i32, 2 bool u8, 3 data: vcount + bytes), then u8 0.

## 3. Derived ids and hashes
* XOR-derived GUIDs (Data3 top nibble 0): event parameter instance & event sheet layout = Event ^ GameParameter;
  global parameter layout = GP ^ {d04fb8b4-a6aa-4f2a-a9fa-dbbeeb5abb4f}; snapshot SNAB = Snapshot ^
  {9b8f058f-e215-491b-808b-ba061f27afd8}; snapshot master bus = Snapshot ^ {44dc1587-6b49-42fe-905d-6983c57b6b8f};
  parameter cursor mapping = GP ^ {81a9378f-8927-40bc-a1bc-51e8312b25f0}; loop-back transition = LoopRegion ^
  {071550f9-589b-4f5f-9936-a8a83cbc041e}; strings bank = Bank ^ {981d20d4-...}.
* Parameter ID (EVTB h90, FMOD_STUDIO_PARAMETER_ID) = `calculatePublicID(guid)`: lookup3 hashlittle2 chained over
  Data1 (4 bytes), Data2 (2), Data3 (2), Data4 (8), pc=pb=0 initially -> (data1=c, data2=b).
* Snapshot property key = xor of the target GUID's four LE u32 words ^ property index.
* Effect/property mapping GUIDs are fixed constants per (class, property) (table in effects.rs).
* FSB5 header hash = MD5(sample headers || 60-byte header with the hash field zeroed).
* HASH chunk: per top-level object MD5 over a QDataStream (v20, Qt_5_14) serialisation of the Studio model object
  tree, folded to u32 (xor of the 4 big-endian words). Not reproduced; the runtime ignores HASH (read only with live
  update) - writer emits an empty array.

## 4. Leaf layouts
* IBSB/GBSB/RBSB/MBSB: guid, {guid output (0 for masters), u32 output speaker mode, i32 -1}.
* BUS: u32 flags (1 mixer-level bus, 0 event bus/snapshot master), u32 input speaker mode, [guid] pre-fader effects,
  [guid] post-fader effects (+ emitted panner last), {f32 volume, f32 0}, [guid] VCAs (masters rel order), u32
  maxInstances (0x7fffffff), u32 stealing (MixerGroup/EventMixerGroup/EventMixerMaster 2, MixerMaster instanceStealing,
  MixerReturn/MixerInput/snapshot master 0), [u32] speaker mode at each pre effect input, [u32] post, i32 -1, u32 0;
  then LIST PRPS.
* PRPS: LCNT + PROP {u32 property index, u16 a, u16 b, guid mapping, [guid] curves (sorted), [guid] modulators (sorted)}.
  Mapping (MAP: guid, [f32 x, f32 y]) is emitted once in MPGS.
* BEFF: BEFB {guid, u32 FMOD_DSP_TYPE}, PMEF, EFFB {u32 flags (1 LoudnessMeter), f32 100, 0, -80, 0}, PRPS.
  PEFF: PEFB {guid, str plugin identifier, str ""}, PMEF (plugin parameters in order: float value / empty data), EFFB, PRPS.
  SEFF: SEFB {guid, guid return, f32 level}, EFFB, PRPS.
* VCAB: guid, [guid] controlled buses (sorted), {f32 volume, u32 0}, [guid]; PRPS.
* EVTB: guid id, guid (0 / snapshot SNAB id), guid timeline, guid mixer input (0 snapshot), guid master bus, u32
  maxVoices (0x7fffffff), u32 priority = (4 - priority) * 64, u8 (1 when voiceStealing = 3), u32 e4 scheduling mode
  (Studio calculateSchedulingModeForEvent: 3 noLookahead, 2 streaming, 1 when the event has an EventSound, a
  SoundScatterer, a MultiSound with `looping`, or more than one instrument - transition sounds and SnapshotModule
  excluded - else 0), [guid] parameter instances (sorted), [str u32], [str str] user properties, f32 doppler scale (0 when
  doppler off, default 100), u32 stealing {0:2,1:3,2:1,3:0,4:4}, u32 0, u32 flags (0x1441 | 0x20 3D | 2 persistent | 0x100
  doppler [| 0x800 oneshot]; snapshots 0x1463). 3D = the event file holds a Spatialiser/ObjectSpatialiser effect, or
  something in it references a built-in parameter (parameterType >= 3: distance, cone angle, ...); unused sheet
  proxies do not count (Studio: is3DEvent visitor in fmod_studio_impl.cpp), [guid] event buses (sorted), [u32 u32] parameter ids (same order as
  instances), [guid], f32 min distance, f32 max distance; PRPS (EventAutomatableProperties min/max distance
  automation: props 30/31).
* TLNB: guid, [guid u32 u32] (unused), [guid u32 start u32 length] timeline instruments (sorted; seconds*48000
  rounded), [u32 code], [guid u32 position str name u32 length] markers: NamedMarker / LoopRegion objects of the
  timeline grouped by marker track in Event.markerTracks order, GUID-descending (FMOD comparator) inside a track,
  [guid u32 u32 u32 f32]; then LIST TRNS (TRAN items, same track grouping/order: TransitionRegion, TransitionMarker,
  LoopRegion loop-back), then CTRO [guid].
* TRNB: guid, guid destination, u32 start, u32 end (samples), code (trigger conditions), u32 0, u32 0, f32 100, u32
  flags (TransitionRegion 0, TransitionMarker 1, loop-back 2, loop-back with a transition timeline 0). TRTL: empty, or
  the transition timeline: u32 length, [], [], [], u32 source length, u32 destination length, [guid bus, guid source
  sound] (sorted), [guid bus, guid destination sound], guid 0, []. Transition sound fades are CURV items with
  id = sound, second guid 0, x = u32 sample position, default ramp [start, max(start+length, 1)].
* PMLB: guid layout, guid GameParameter, [guid] instruments on the sheet + instruments with trigger conditions on
  it (sorted), u32 1 global / 0 event; CTRO [guid] curves driven by the parameter (sorted).
* PRMB: guid GP, u32 flags (1 global, 8 discrete, 16 built-in, 32 labeled), u32 type (0 game, 1 distance, 2 cone
  angle), str preset name, f32 min, f32 max (discrete: XML max - 1), f32 initial, f32 0, [str] labels; PRPS (cursor
  seek modulators: PROP 2, a 3, b 0, mapping GP ^ C, points [(0,min),(1,max)]).
* CTRL: guid curve, guid automated object, guid curve, u32 property index. CURV: guid curve, guid curve,
  [f32 x, f32 y, f32 curveShape, u32 flags]; flags = isSCurve (1), or 3 (stepped) for EVERY point when the driving
  parameter is discrete/labeled (parameterType 1/2), whatever isSCurve says. y stored in the mapping domain:
  t = (v-y0)/(y1-y0), x = x0 + t*(x1-x0) in f32.
* WAIT: WAIB {guid SingleSound, guid AudioFile} + INST. MUIT: MUIB {guid} + PLST {u32 playlistMode (default 2), u32 1,
  [guid sound, f32 100/n]} + INST; playlist members are WAIT items with no bus / no trigger.
* SPIT (in SPIS, after MUIS): SPIB {guid SoundScatterer, u32 polyphony (16), u32 totalSounds (33), f32
  minimumTimeBetweenSounds/1000 (500), f32 maximumTimeBetweenSounds/1000 (1000), u32 soundStealing + 2 (0,1 seen),
  f32 spawnRate (100), u32 0, u32 0} + PLST {u32 0, u32 1, [guid scatterer.sound MultiSound, f32 1.0]} + INST (speaker
  mode of the nested MultiSound) + PRPS. The nested MultiSound is a MUIT member (no bus, no trigger).
  spawnDelayType 1 (tempo) zeroes the two times and sets the last two u32 to 1/1 (not mapped). Non-zero scatter
  distance (schema default 1/20!) adds props 5/6/7 + MODB type 1 on the nested MultiSound; volume/pitchRandomization
  add MODB type 1 on the nested MultiSound (pitch amount = semitones/24*100; volume a=1, amount = -dB) with
  derived ids - not implemented (fallback).
* INST: guid timeline (timeline instruments) or 0, f32 volume, f32 pitch, i32 loop count (-1 looping), u32 isCutoff,
  f32 0, f32 trigger probability (triggerProbability when triggerProbabilityEnabled, else 100), f32 min delay, f32 max
  delay (minimum/maximumTimeDelay seconds when delayType = 0, else 0), u32 0, u32 0, guid autopitch parameter, f32
  autopitch root, f32 pitchAtMinimum, u32 maxVoices (0x7fffffff when unset), {guid track bus, u32 instrument speaker
  mode, i32 -1}, u32 stealing (0 when voiceStealing = 3, else 2 - values 1/2/4 also give 2), u32 timelockedOffset
  samples, f32 startOffset, f32 0, code; PRPS (volume 0 / pitch 1 / startOffset 11 automation, ADSR / Randomizer
  modulators).
* Trigger code terms: parameter-sheet range [start, start+length) (0x5220 exclusive when the end < parameter max,
  else 0x5020), then each ParameterCondition (discrete: [min, max+1) 0x5220; continuous [min, max] 0x5020). Terms are
  joined with 0x1002; a code with more than one term ends with 0x30 (the sheet range counts as a term).
* MODB: {guid, guid target, u32 property, u32 type, u32 a, u32 b} + data. ADSR type 0 (a=1,b=0): initial, peak, sustain,
  attack, hold, decay, release, 3 shapes, final (f32); the four times default to 1000 ms (schema), not 0.
  Randomizer type 1 (a=0 for pitch, b=0): f32 amount as in the XML. Seek type 4 (target GameParameter, property 2, a=0,b=1): u32
  asymmetric, f32 speedAscending, f32 speedDescending. MODU = MODB + empty PRPS.
* WAV: guid AudioFile, {u32 0, u32 FSB sample index, u32 0}.
* SNAB: guid, u32 priority (SnapshotList order, top = n-1), [u32 key, guid target, u32 property, f32 value] (XML order),
  u8 1, u32 3 (behavior 1), f32 intensity 100.
* STDT (strings bank): u32 1, [u32 w0, u32 w1] nodes, [guid] (FMOD comparator order), vcount+blob, vcount + 3-byte
  GUID->node, vcount + 3-byte node->parent. Radix tree over the paths inserted in byte order, merged
  case-insensitively (first label wins); node w0 = lowercase first char << 24 | blob offset of the edge label (0xffffff
  root / terminal), w1 = child count << 24 | first child index (leaf: GUID index). DFS pre-order: visiting a node
  allocates all its children contiguously; a path ending at an inner node gets a terminal child first.

## 5. Bus formats (speaker modes: mono 2, stereo 3, quad 4, 5.0 5, 5.1 6, 7.1 7)
* Studio channel enum -> speaker mode: 0->2, 1->3, 2->6, 3->4, 4->5, 5->6, 6->7.
* Input = overridingInputFormat, else the widest of its inputs (instruments: AudioFile channels; child buses' output;
  event mixer inputs = event outputFormat), mono by default. Format after the effect chain: a SpatialiserEffect outputs
  the event output format. Output = panner overridingOutputFormat, else the chain output.
* Panner DSP is emitted when it changes the format, or when the output has >= 4 channels and the panner has a
  non-default surround setting (LFE level/upmix, stereo-to-surround mode, direction, ...).

## 6. Strings
`bank:/<name>`, `bank:/<name>.strings`, `bus:/` (master), `bus:/<group>[/<child>]`, `vca:/<name>`, `snapshot:/<name>`,
`event:/<folders>/<name>`, `parameter:/<preset folders>/<preset>` for every parameter in the bank plus every
parameter that drives another parameter's cursor (GameParameter automators).

## 7. Effects
SFXReverb (19) 13 floats; ThreeEQ (28) 5 f + i crossoverSlope (default 1); Lowpass (3) cutoff 2000, resonance 1;
LowpassSimple (20); Highpass (5) cutoff 2000, resonance 1; ParamEq (12) centreFrequency 2000, bandwidth 1, gain 0;
Gain (7 = fader) gain + empty data; Distortion (9); Compressor (18) threshold, ratio 2.5, attack 20, release 200, gain, data 4 zero, bool
linked; LoudnessMeter (30) i 1, d, d; ChannelMix (33) i grouping + 32 f gains + 32 i channels; pan DSP 27 for
MixerBusPanner (pan blend 0, override range true) and SpatialiserEffect (pan blend 1): 24 params (effects.rs).

## 8. FSB5 (PCM16)
Header 60 bytes: FSB5, 1, n, sample header size, name table size, data size, mode 2, 16 zero bytes, 16-byte MD5 (sect. 3).
Sample header u64 = ((frames<<27 | offset/32)<<2 | chan code{1:0,2:1,6:2,8:3})<<4 | rate code{4000:0,8000:1,11000:2,
12000:3,16000:4,22050:5,24000:6,32000:7,44100:8,48000:9,96000:10})<<1 | more. Extra chunks: u32 (size | type<<24)<<1 |
more, payload. Channel counts outside 1/2/6/8 use code 0 plus a type-14 chunk (u32 channel count) after the peak. Type 13 = peak f32 = max |s| over samples with abs in i16 (-32768 wraps and never wins) / 32768.
WAV cue points (cue + LIST adtl labl): chunk 5 = [u32 sample offset] when no label has text, chunk 4 = [u32 offset,
256-byte NUL-padded name] when any does; offsets read as i32 and clamped to [0, frames]. Names: asset file stems, offset table then NUL-terminated
names, padded so data starts 32-aligned. Sample order = AudioFile GUID order. Data: PCM + 16 zero frames, padded to 32.

## 9. Open items / limits
* HASH chunk: MD5 over Studio's QDataStream serialisation of its model objects (BankHashEncoderRule /
  PersistentEncoderRule); not derivable from the bank's contents (asset mtime does not change it). We write it as an
  empty array; the runtime ignores it. Verification splices Studio's HASH in (hashswap.py) and then compares bytes.
* Streaming assets (MasterAssetFolder streamingAssetThresholdLength >= 0, or isStreaming) are rejected -> fallback.
* Studio nondeterminism: an object with several Automators on the SAME property keeps only one, chosen by QHash
  order (random per process). The converter's ml_gates used to create such duplicates; it now merges them.
* Some events render nondeterministically through fmodstudio.dll even from one bank (FMOD randomSeed does not fix
  it); rendercmp.py compares the sets of outputs over 5 renders.
* Automation mappings are a fixed table (effects.rs `mapping`), filled from Studio builds: fader-domain volume/gain,
  pitch, startOffset, SFXReverb, Lowpass/LowpassSimple cutoff, Distortion level, ThreeEQ gains + lowCrossover, ParamEq
  centre/gain, ChannelMix gains, spatialiser extent, event min/max distance. Anything else -> fallback.
* Unsupported -> fmodstudiocl fallback (any other writer error falls back too, logged as ERROR): see compile*.rs `unsupported(...)` (tempo scatterers, scatter distance,
  randomization, >2 trigger terms, other modulators/automation targets, multiple master banks, ...).

## 10. Property -> field map (differential builds)
| project change | bank change |
|---|---|
| SingleSound.volume / pitch | INST f90 / fa4 |
| start / length | INST code range |
| looping / isCutoff | INST loop count -1 / cutoff 1 |
| isAsync (parameter sheet) | nothing |
| EventMixerMaster.volume | BUS volume |
| EventAutomatableProperties.minimumDistance / maximumDistance | EVTB f0 / f4 |
| voiceStealing / priority / maxVoices / doppler / isPersistent | EVTB e8+u8 / dc / d8 / e0+flag 0x100 / flag 2 |
| Event.outputFormat | IBUS fmt+kind (and the busses it feeds) |
| MixerGroup.overridingInputFormat | GBUS fmt + kind |
| MixerBusPanner.overridingOutputFormat | panner DSP + GBSB kind |
| Event.name | strings bank only (+HASH) |
| triggerProbabilityEnabled + triggerProbability | INST f98 (probability alone: nothing) |
| delayType 0 + minimum/maximumTimeDelay | INST f80 / f84 (delayType 1/2: nothing) |
| voiceStealing 3 / maxVoices | INST stealing word 0 / fb4 |
| SoundScatterer polyphony / soundStealing / spawnDelayType 1 | SPIB +0 / +16 / times zeroed, +28/+32 = 1 |
| spawnQuantizationInterval | nothing |

## 11. Additions from reading the game's own banks (branch own-fmod, 2026-10-17)
Reader: `src/sound/bankread.rs`; Studio project generator: `src/sound/mkfmod.rs` (`aceconv make-fmod-template`,
`aceconv fmod-decompile <content|dir> <bank stem> <out>`). Verified: game bank -> XML -> writer reproduces the game's
items (sample indices, 7.1 platform and one exclusive-at-max region aside), and the writer = fmodstudiocl on the result.
* Trigger code, general form (Studio FUN_ in fmodstudioui.c @819200): with a prior term (sheet range) `0x1002` before the
  conditions and `0x30` after them; every condition after the first is preceded by the join (`0x1002` all, `0x1102`
  any = triggerConditionMode 1) and followed by `0x30`. Range tag `| 0x100` = ParameterCondition.isInverted
  (0x5120 continuous, 0x5320 discrete).
* Module fade curves (FadeCurve startPoint/endPoint on fadeInCurve/fadeOutCurve) = instrument PROP 4 (a 4, b 0, mapping
  {678d3693-...} identity): CTRL/CURV with the FadeCurve id, x = absolute parameter position, driven by the sheet
  parameter (layout CTRO) or the timeline (TLNB CTRO).
* MultiSound PlayPercentage {percentage, sound} -> PLST weight (else 100/n).
* EFFB flags bit 0 = bypass (MixerEffect.bypass; LoudnessMeter always 1). Fields 2..5 = wetMix, baseWet, baseDry, 0.
* ADSR on an event bus: MODB type 0 target = bus, prop 0, a 1 b 0; PROP 0 lists it.
* ChannelMix outputGrouping 6 (ALLLFE) -> 5.1 after the effect; the bus output format follows ChannelMix groupings.
* A bus panner is emitted also when stereoPosition != 0 (stereo or wider output).
* SeekModulator MODB b = clockSource (template: 1; Studio default 0).
* Studio defaults that differ from the writer table: SFXReverb highCut (Studio default is not 20000), GameParameter
  initialValue (Studio: = minimum). The generator therefore writes every effect/pan parameter and min/max/initial
  explicitly.
* Mapping GUIDs seen only in game banks: ChannelMix 1005 {15e37282-...}, 1006 {f01c962e-...}; Spatialiser 1012/1013
  min/max distance {ddd0ed14-...}/{506e7c91-...}; also (not in the writer) Spatialiser 1015 soundSize {a321ae35-...},
  Lowpass-simple variant {5103bef4-...}, instrument props 5/6/7/22 (wheel.bank).
* Plugins: Studio matches a placed plugin by parameter name/type/curve; names, DATA types and piecewise curves come from
  the DLL's FMODGetDSPDescription (x64: numparameters @96, paramdesc** @104; desc: type @0, name[16] @4, float min @48,
  max @52, default @56, mapping type @64, numpoints @72, values* @80, positions* @88; data type @48). AUTO mapping
  (type 1) curves are made by Studio (10..22000 Hz -> the FREQ22K table).
* Kunos' banks are built for a 7.1 platform (bus/spatialiser speaker mode 7); game banks are Vorbis (codec 0x0f).
* Not modelled (noted by the generator when met): numeric user properties, tempo/region timeline data, transition
  probability/quantisation, scatter distance / randomisation, modulators on effects, Studio's implicit transition fade
  ramp for sounds shorter than a few samples (the generator always writes explicit fade curves).

## 12. FMOD Studio 1.08 banks (FMT 0x50: Assetto Corsa's car sounds)
Reader: `src/sound/ac1bank.rs` (into the 2.03 model of `bankread`). Worked out from AC1's own banks and their behaviour in
AC1's fmodstudio64.dll (1.08.12), which `tools/fmodrender.py --rt ac1` renders offline. Same container and list names as
2.03 (no PRTS, SSFX, CMDS, SLNS, LWVS, HASH, PLAT); items differ:
* BUS: u8 flags, u32 input speaker mode, [pre], [post], {f32 volume, f32}, u32 max instances, u32 stealing (no VCA list).
* BEFB: guid, u32 speaker mode, u32 DSP type, u8 bypass (no EFFB). The pan DSP has 21 parameters (2.03: 24; the reader
  appends 2D height blend 0, attenuation range, override range on). PEFB: guid, u32 speaker mode, str identifier, str,
  u8; "FMOD Distortion" is a plugin there (built-in Distortion in 2.03).
* EVTB: id, snapshot, timeline, mixer input, master bus, u32 max voices, u32 priority, u8, u32 scheduling,
  [parameter] (the parameter GUIDs themselves, not event ^ parameter), [str u32], [str str], f32 doppler (0 = off),
  u32 stealing, u32. No event bus list, flags or min / max distance (1.08 spatialisers carry their own range).
* TLNB: guid, guid event, [guid u32 start u32 length] non-timelocked instruments, [same] timelocked ones, [u32 code],
  [guid u32 position str name] markers (no length), [regions]. TRNB: guid, guid destination, u32 start, u32 end,
  [guid parameter, f32 min, f32 max] conditions, u32, u32, f32 probability, u32, u8; start = end is a transition
  marker, else a region (the loop at the end of AC1's horn: a marker back to a marker at 0).
* PMLB: guid layout (= the parameter), guid parameter, guid event, [guid instrument, f32 start, f32 length]: sheet
  placement lives here, not in the instrument's trigger code (code length 0 on every AC1 car bank seen).
* PRMB: guid, u8, u32 type (0 game, 1 distance, 2 event cone angle, 4 direction), str name, f32 min, f32 max,
  f32 initial, f32 velocity, f32 seek speed, [guid], u8.
* CTRL: guid, guid target, guid driving parameter, guid curve, u32 property. CURV, MAP: as 2.03.
* INST (123 bytes): as 2.03 but a u8 where 2.03 has its first reserved f32, the bus block ends with the output
  speaker mode, and the item ends with the f32 after the start offset: no trigger code. PLST weights are plain
  weights (2.03: percentages).
* MODU (a bare chunk): guid, guid target, u32 property, u32 type, u16 size + data. ADSR: initial, peak, sustain,
  attack, hold, decay, release (seconds; 2.03: ms), 3 shapes; volume levels in the fader-mapped domain (-42 = -80 dB).
  Randomizer: (-amount, +amount).
* WAV: guid, {u32 FSB index, u32 sample index, u32}. SNDH: one (offset, size) per FSB5; banks can mix codecs
  (bdp_mazda_rx8: one Vorbis FSB5, one PCM16).

## 13. The car sound stage
`src/sound/map.rs` (+ `slots.rs`, `render.rs`): AC1 event -> EVO role, re-pointed at the game's parameters and buses,
decompiled by `mkfmod` onto the game's mixer, built by `bankwrite`, written as `content\cars\<id>\sfx\master_bank.bank`
+ `.strings.bank` (bank name = the car id; events `event:/evo_cars/<id>/<role>`), and the actor's audio slots
(f103.f9: f1 event, f2 type, f6 bank) pointed at them.
* Roles: engine_int / engine_ext / turbo / transmission whole; AC1 `limiter` merged into both engine events on the
  game's `limiter` parameter [0.5, 1]; gearbox = AC1 `gear_int` (else `gear_ext`) with its `state` sheet split onto
  `gear_up` / `gear_dn` [0.5, 1]; backfire = AC1 `backfire_ext` (else `_int`) on a `throttle_position` lift [0, 0.25)
  above half the AC1 limiter rpm, 60 % of the time (OURS), persistent like the game's; horn on `horn_sound` = 1,
  looping, persistent. Silent AC1 events (no instruments, master <= -60 dB) keep the game's.
* Parameters (same units on stock events): rpms, throttle -> throttle_position, boost, bov, drivetrain_speed, speed,
  brake -> brake_position, distance -> engine_distance, cone angle -> Event Cone Angle. Others stay the event's own.
* Engines get a volume curve by rpm (-80 dB below 100 rpm, 0 dB from 300: AC1 sheets often start at 0 rpm) (OURS).
* Level (OURS): an automated volume ignores the bus value (curves set it, several add up), so offsets go onto a curve.
  The stage builds, renders a 1000 rpm -> limiter full-throttle sweep through the game's fmodstudio.dll (exterior 6 m
  behind the car, interior 1 m), and rebuilds with the engine master offset so the RMS is the median of the 75 stock
  cars measured the same way: exterior -15.6 dBFS, interior -17.2 dBFS (stock IQR -17.8..-13.7 / -18.4..-15.7). The
  other roles take the exterior offset (AC1's own balance between them stays). Without the DLL: fixed role gains.
* GUIDs: every object that is not the game's is re-seeded from (car id, role, AC1 GUID).
* Samples: PCM16, or Vorbis q 0.8 when every AC1 FSB5 is Vorbis or the PCM16 bank would pass 96 MB (OURS: stock car
  banks are 2-20 MB of Vorbis, and the game holds a car's samples in memory); ACECONV_BANK_VORBIS=<q> / 0 forces
  Vorbis / PCM16.
* Left out, noted: AC1 effects the writer has no counterpart for (FMOD Tremolo, Pitch Shifter, Convolution Reverb,
  Highpass Simple, echo ...; FMOD Lowpass Simple / Gain / Distortion become the built-in effects), automation without a
  known Studio mapping (the effect keeps its set value), modulators other than ADSR on volume and randomizers on
  pitch / start offset, automation along an event's timeline, sounds an AC1 bank names but does not hold.
* The level offset sits on the event's mixer input (never automated); the runtime caps a bus at +10 dB, so a boost
  beyond that goes onto the event master (very quiet mod banks can stay a few dB under the target).
