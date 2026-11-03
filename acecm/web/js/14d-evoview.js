/* ACECM UI - evoview. One of the ordered classic scripts listed in
   index.html; they share one global scope (see 01-core.js). */
/* -------------------------------------------------------------- evoview -- */
/* evoview's driving mode: pick a track and a car, set it up, Drive.
   `evoview <content.kspkg> --track <id> --drive-car <car>` plus EVOVIEW_*
   environment variables, launched by the backend (evodrive.rs).

   ⚠ evoview grows features on branches that land at different times, so
   every option here is ONE ENTRY in a table, and it shows only when the
   installed evoview.exe contains its token (an EVOVIEW_* name, a flag or a
   wheel role - the backend reads the exe). Anything this build does not know
   yet sits under "Coming with a newer evoview" instead of being sent and
   silently ignored. Adding a feature = adding a line here, nothing else.

   Knob fields:
     id       profile key (prof.knobs[id])
     pages    which pages draw it: drive / graphics / controls
     group    heading it sits under
     env      the EVOVIEW_* variable it sets
     kind     select | toggle | range | text
     placeholder/max  text: grey hint inside the box, and its length limit
     options  select: [value, label] or {v, label, env: {VAR: value}} when one
              choice sets other variables (weather). '' = evoview's default,
              nothing is sent
     on/off   toggle: the value sent when ticked / unticked (null = nothing)
     min/max/step/def  range: def is evoview's own default, never sent
     when     shown and sent only while this holds (v = another knob's value)
     needs    tokens the exe must contain (default: the variable(s) it sets) */
const EVO_KNOBS = [
  // ---- driving
  { id: 'wheel', pages: ['drive'], group: 'Driving', env: 'EVOVIEW_WHEEL', kind: 'toggle',
    label: 'Steering wheel', on: '1', off: null, def: true,
    hint: 'Wheel, pedals and force feedback (wheel.cfg). Off: keyboard.' },
  { id: 'rigcar', pages: ['drive'], group: 'Driving', env: 'EVOVIEW_RIGCAR', kind: 'toggle',
    label: 'Full physics', on: '1', off: null, def: true,
    hint: 'The per-wheel car: tyre temperatures, TC and ABS. Off: the simpler model.' },
  { id: 'cam', pages: ['drive'], group: 'Driving', env: 'EVOVIEW_CAM', kind: 'select',
    label: 'Camera', options: [['', 'Near chase'], ['cockpit', 'Cockpit'], ['dash', 'Dash'],
      ['bonnet', 'Bonnet'], ['bumper', 'Bumper'], ['farchase', 'Far chase'],
      ['chase', 'Viewer chase'], ['tv', 'TV cameras'], ['free', 'Free camera']],
    hint: 'Where you start. C cycles cameras while driving.' },
  { id: 'units', pages: ['drive'], group: 'Driving', env: 'EVOVIEW_SPEED_UNITS', kind: 'select',
    label: 'Speed', options: [['', 'km/h'], ['mph', 'mph']],
    hint: 'Units on the driving cluster and the pit HUD.' },
  // ---- session
  { id: 'tod', pages: ['drive'], group: 'Session', env: 'EVOVIEW_TOD', kind: 'select',
    label: 'Time of day', options: [['', 'Default light'], ['07:00', '07:00 morning'],
      ['10:00', '10:00'], ['13:00', '13:00 midday'], ['16:00', '16:00 afternoon'],
      ['18:30', '18:30 golden hour'], ['20:00', '20:00 dusk'], ['22:00', '22:00 night']],
    hint: 'O / U move the clock 15 min while driving (Shift: 1 h), ; runs it.' },
  { id: 'weather', pages: ['drive'], group: 'Session', kind: 'select', label: 'Weather',
    options: [{ v: '', label: 'Dry' },
              { v: 'wet', label: 'Wet track', env: { EVOVIEW_WET: '1' } },
              { v: 'light', label: 'Light rain', env: { EVOVIEW_RAIN: '0.4' } },
              { v: 'rain', label: 'Heavy rain', env: { EVOVIEW_RAIN: '1' } }],
    hint: 'F5 changes it while driving.' },
  { id: 'tyres', pages: ['drive'], group: 'Session', env: 'EVOVIEW_TYRES', kind: 'select',
    label: 'Tyres', options: [['', 'Auto (wets in rain)'], ['dry', 'Dry'], ['wet', 'Wet']] },
  // ---- picture (Drive shows the upscaler too: it is the one people change)
  { id: 'upscaler', pages: ['drive', 'graphics'], group: 'Upscaler', env: 'EVOVIEW_UPSCALER',
    kind: 'select', label: 'Upscaler', options: [['', 'TAA'], ['dlss', 'DLSS (NVIDIA)'],
      ['fsr', 'FSR 3.1 (any GPU)']] },
  { id: 'dlss_q', pages: ['drive', 'graphics'], group: 'Upscaler', env: 'EVOVIEW_DLSS_QUALITY',
    kind: 'select', label: 'DLSS quality', when: v => v('upscaler') === 'dlss',
    options: [['', 'Quality'], ['dlaa', 'DLAA (native)'], ['ultraquality', 'Ultra quality'],
      ['balanced', 'Balanced'], ['performance', 'Performance'],
      ['ultraperformance', 'Ultra performance']] },
  { id: 'fsr_q', pages: ['drive', 'graphics'], group: 'Upscaler', env: 'EVOVIEW_FSR_QUALITY',
    kind: 'select', label: 'FSR quality', when: v => v('upscaler') === 'fsr',
    options: [['', 'Quality'], ['native', 'Native AA'], ['balanced', 'Balanced'],
      ['performance', 'Performance'], ['ultraperformance', 'Ultra performance']] },
  { id: 'scale', pages: ['drive', 'graphics'], group: 'Upscaler', env: 'EVOVIEW_RENDER_SCALE',
    kind: 'range', label: 'Render scale', min: 0.5, max: 1, step: 0.05, def: 1,
    when: v => !v('upscaler'), fmt: x => Math.round(x * 100) + ' %',
    hint: 'Below 100 % TAA upsamples to the window.' },
  // ---- pits (gfx-pits)
  { id: 'pit_start', pages: ['drive'], group: 'Pits', env: 'EVOVIEW_PIT_START', kind: 'select',
    label: 'Start', options: [['', 'On track'], ['box', 'Pit box'], ['garage', 'Garage'],
      ['lane', 'Pit lane']] },
  { id: 'pit_rules', pages: ['drive'], group: 'Pits', env: 'EVOVIEW_PIT_RULES', kind: 'select',
    label: 'Pit lane rules', options: [['', 'Practice (60 km/h)'], ['race', 'Race (50 km/h)'],
      ['off', 'Limit only, no penalties']] },
  { id: 'pit_limiter', pages: ['drive'], group: 'Pits', env: 'EVOVIEW_PIT_LIMITER', kind: 'toggle',
    label: 'Start with the pit limiter on', on: '1', off: null, def: false },
  // ---- race: AI opponents (race.rs)
  { id: 'ai', pages: ['drive'], group: 'Race', env: 'EVOVIEW_AI', kind: 'range',
    label: 'Opponents', min: 0, max: 15, step: 1, def: 0,
    hint: 'AI cars on the grid. Each costs about 0.8 ms of CPU per frame.' },
  { id: 'ai_start', pages: ['drive'], group: 'Race', env: 'EVOVIEW_AI_START', kind: 'select',
    label: 'Start', options: [['', 'Standing start'], ['rolling', 'Rolling start']],
    when: v => Number(v('ai')) > 0 },
  { id: 'ai_grid', pages: ['drive'], group: 'Race', env: 'EVOVIEW_AI_GRID_POS', kind: 'select',
    label: 'Your grid slot', options: [['', 'Middle of the grid'], ['1', 'Pole position']],
    when: v => Number(v('ai')) > 0 },
  { id: 'ai_pace', pages: ['drive'], group: 'Race', env: 'EVOVIEW_AI_PACE', kind: 'select',
    label: 'Opponent pace', options: [['', 'Mixed'], ['0.96,1.0', 'Fast'], ['0.85,0.93', 'Easier']],
    when: v => Number(v('ai')) > 0 },
  // ---- online: join a dedicated server (the server's track and layout load
  // whatever track is picked; your Steam account unless a name is set)
  { id: 'mp_server', pages: ['drive'], group: 'Online', env: 'EVOVIEW_MP', kind: 'text',
    label: 'Join server', placeholder: 'ip:port, e.g. 31.44.44.53:9031', max: 64,
    hint: 'Leave empty to drive offline. The server’s track loads automatically; the car you pick must be one it allows.' },
  { id: 'mp_name', pages: ['drive'], group: 'Online', env: 'EVOVIEW_MP_NAME', kind: 'text',
    label: 'Driver name', placeholder: 'your Steam name', max: 32,
    hint: 'What other drivers see in their list and in chat.',
    when: v => !!String(v('mp_server') || '').trim() },
  // ---- driver aids at the start
  { id: 'no_tc', pages: ['drive'], group: 'Aids', env: 'EVOVIEW_NO_TC', kind: 'toggle',
    label: 'Remove the car’s traction control', on: '1', off: null, def: false,
    hint: 'T switches TC off for the drive; this takes it out entirely.' },
  { id: 'no_abs', pages: ['drive'], group: 'Aids', env: 'EVOVIEW_NO_ABS', kind: 'toggle',
    label: 'Remove the car’s ABS', on: '1', off: null, def: false },
  // ---- graphics
  { id: 'max_fps', pages: ['graphics'], group: 'Frame rate', env: 'EVOVIEW_MAX_FPS', kind: 'select',
    label: 'Frame cap', options: [['', '60'], ['90', '90'], ['120', '120'], ['144', '144'],
      ['165', '165'], ['240', '240'], ['0', 'Uncapped']] },
  { id: 'novsync', pages: ['graphics'], group: 'Frame rate', env: 'EVOVIEW_NOVSYNC', kind: 'toggle',
    label: 'Vsync off', on: '1', off: null, def: false, hint: 'Still held to the frame cap.' },
  { id: 'sharpen', pages: ['graphics'], group: 'Image', env: 'EVOVIEW_SHARPEN', kind: 'range',
    label: 'Sharpening', min: 0, max: 2, step: 0.05, def: 1 },
  { id: 'taa_move', pages: ['graphics'], group: 'Image', env: 'EVOVIEW_TAA_MOVE_W', kind: 'range',
    label: 'TAA in motion', min: 0.1, max: 0.35, step: 0.01, def: 0.2,
    hint: 'Higher: sharper moving scenery, more shimmer.' },
  { id: 'mblur', pages: ['graphics'], group: 'Image', env: 'EVOVIEW_MOTION_BLUR', kind: 'range',
    label: 'Motion blur', min: 0, max: 1, step: 0.05, def: 0,
    fmt: x => x ? Math.round(x * 100) + ' %' : 'off',
    hint: 'Shutter as a share of a 60 Hz frame. Off by default while driving; / toggles it.' },
  { id: 'blur_rot', pages: ['graphics'], group: 'Image', env: 'EVOVIEW_BLUR_ROT', kind: 'range',
    label: 'Blur when turning', min: 0, max: 1, step: 0.05, def: 0.35,
    when: v => Number(v('mblur')) > 0, fmt: x => Math.round(x * 100) + ' %' },
  { id: 'dof', pages: ['graphics'], group: 'Image', env: 'EVOVIEW_DOF', kind: 'toggle',
    label: 'Depth of field on TV cameras', on: null, off: '0', def: true },
  { id: 'ev_track', pages: ['graphics'], group: 'Look', env: 'EVOVIEW_EV_TRACK', kind: 'range',
    label: 'Brightness', min: -1.5, max: 1, step: 0.1, def: -0.5,
    fmt: x => (x > 0 ? '+' : '') + Number(x).toFixed(1) + ' EV', hint: 'Outdoors; the cockpit follows.' },
  { id: 'clarity', pages: ['graphics'], group: 'Look', env: 'EVOVIEW_CLARITY', kind: 'range',
    label: 'Local contrast', min: 0, max: 0.5, step: 0.05, def: 0,
    fmt: x => x ? String(x) : 'off', hint: 'Brings out detail in trees and stands; strong values look like a filter.' },
  { id: 'shafts', pages: ['graphics'], group: 'Look', env: 'EVOVIEW_SHAFTS', kind: 'range',
    label: 'Sun shafts', min: 0, max: 0.5, step: 0.05, def: 0,
    fmt: x => x ? String(x) : 'off', hint: 'Light rays through trees and stands when the sun is low.' },
  { id: 'tex_lod', pages: ['graphics'], group: 'Image', env: 'EVOVIEW_TEX_LOD', kind: 'select',
    label: 'Track textures', options: [['', 'Auto (half above 4K)'], ['0', 'Full'], ['1', 'Half']] },
  { id: 'lod_scale', pages: ['graphics'], group: 'Image', env: 'EVOVIEW_LOD_SCALE', kind: 'range',
    label: 'Detail distance', min: 1, max: 5, step: 0.5, def: 3, fmt: x => '×' + x,
    hint: 'How far detailed models are kept. 1 is the old look.' },
  // ---- head movement (gfx-camera); defaults are the game's camerasettings
  { id: 'cam_head', pages: ['graphics'], group: 'Head movement', env: 'EVOVIEW_CAM_HEAD',
    kind: 'toggle', label: 'Head movement in the car', on: null, off: '0', def: true },
  { id: 'cam_move', pages: ['graphics'], group: 'Head movement', env: 'EVOVIEW_CAM_MOVE',
    kind: 'range', label: 'Movement', min: 0, max: 2, step: 0.05, def: 0.8,
    when: v => v('cam_head') !== false },
  { id: 'cam_horizon', pages: ['graphics'], group: 'Head movement', env: 'EVOVIEW_CAM_HORIZON',
    kind: 'range', label: 'Horizon lock', min: 0, max: 1, step: 0.05, def: 0.4,
    when: v => v('cam_head') !== false },
  { id: 'cam_scale', pages: ['graphics'], group: 'Head movement', env: 'EVOVIEW_CAM_HEAD_SCALE',
    kind: 'range', label: 'Head offset', min: 0, max: 3, step: 0.1, def: 1, fmt: x => '×' + x,
    when: v => v('cam_head') !== false },
  { id: 'cam_lp', pages: ['graphics'], group: 'Head movement', env: 'EVOVIEW_CAM_LP_HZ',
    kind: 'range', label: 'Body filter', min: 0, max: 10, step: 0.5, def: 3,
    fmt: x => x ? x + ' Hz' : 'off', hint: 'Smooths kerb and bump jitter out of the view.',
    when: v => v('cam_head') !== false },
  { id: 'cam_lws', pages: ['graphics'], group: 'Head movement', env: 'EVOVIEW_CAM_LWS_GAIN',
    kind: 'range', label: 'Look with steer', min: 0, max: 2, step: 0.05, def: 0,
    fmt: x => x ? String(x) : 'off', when: v => v('cam_head') !== false },
  { id: 'cam_dash', pages: ['graphics'], group: 'Head movement', env: 'EVOVIEW_CAM_DASH',
    kind: 'range', label: 'Dash camera share', min: 0, max: 1, step: 0.05, def: 0,
    when: v => v('cam_head') !== false },
  { id: 'cam_bonnet', pages: ['graphics'], group: 'Head movement', env: 'EVOVIEW_CAM_BONNET',
    kind: 'range', label: 'Bonnet camera share', min: 0, max: 1, step: 0.05, def: 0.1,
    when: v => v('cam_head') !== false },
  // ---- force feedback, on top of the game's own FFB settings
  { id: 'ffb_invert', pages: ['controls'], group: 'Force feedback', env: 'EVOVIEW_FFB_INVERT',
    kind: 'toggle', label: 'Invert force feedback', on: '1', off: null, def: false },
  { id: 'ffb_damper', pages: ['controls'], group: 'Force feedback', env: 'EVOVIEW_FFB_DAMPER',
    kind: 'range', label: 'Damper', min: 0, max: 1, step: 0.05, def: 0.1 },
  { id: 'ffb_smooth', pages: ['controls'], group: 'Force feedback', env: 'EVOVIEW_FFB_SMOOTH',
    kind: 'range', label: 'Smoothing', min: 0, max: 0.9, step: 0.05, def: 0 },
  { id: 'ffb_damp_scale', pages: ['controls'], group: 'Force feedback', env: 'EVOVIEW_FFB_DAMP_SCALE',
    kind: 'range', label: 'Dynamic damping', min: 0, max: 1.5, step: 0.05, def: 1,
    fmt: x => Math.round(x * 100) + ' %',
    hint: 'Scales the game’s Dynamic damping: a light resistance to fast steering that grows with speed. Its effect is small.' },
  { id: 'ffb_softclip', pages: ['controls'], group: 'Force feedback', env: 'EVOVIEW_FFB_SOFTCLIP',
    kind: 'toggle', label: 'Soft clipping', on: '1', off: null, def: false,
    hint: 'Rounds off the strongest forces instead of flattening them, so a slide still has feel at the top end.' },
  { id: 'ffb_min_force', pages: ['controls'], group: 'Force feedback', env: 'EVOVIEW_FFB_MIN_FORCE',
    kind: 'range', label: 'Minimum force', min: 0, max: 0.2, step: 0.01, def: 0,
    fmt: x => x ? Math.round(x * 100) + ' %' : 'off',
    hint: 'Lifts small forces past your wheel’s dead zone (belt and gear wheels).' },
  // the game's FFB enhancements; leftmost = the value in the game's settings
  ...[['ffb_curbs', 'EVOVIEW_FFB_CURBS', 'Kerbs', 'A buzz over kerbs, grass and gravel, set by each surface.'],
      ['ffb_gforce', 'EVOVIEW_FFB_GFORCE', 'Bumps', 'Road texture, stronger the harder the car is jolted up and down.'],
      ['ffb_slips', 'EVOVIEW_FFB_SLIPS', 'Slips', 'A vibration once the tyres slide past their grip: wheelspin, lock-ups and drifts.'],
      ['ffb_abs', 'EVOVIEW_FFB_ABS', 'ABS', 'Pulses while ABS is working.']].map(([id, env, label, hint]) => (
    { id, pages: ['controls'], group: 'Force feedback effects', env,
      kind: 'range', label, min: -0.05, max: 1, step: 0.05, def: -0.05,
      fmt: x => x < 0 ? 'as in game' : Math.round(x * 100) + ' %', hint })),
];

/* Wheel roles in %APPDATA%\evoview\wheel.cfg. `needs` is the literal the exe
   must carry for the role to do anything (the wizard's own role list);
   `game` is the game action whose button evoview borrows when wheel.cfg
   leaves the role out ([InputAction, payload], payload 1 down / 2 up).
   ⚠ cam_next / look_back are PROPOSED names: no evoview branch reads them
   yet, so they stay under "Coming" until one does. */
const EVO_ROLES = [
  { role: 'steer', label: 'Steering', kind: 'axis', group: 'Axes',
    none: 'X axis of the force-feedback base' },
  { role: 'throttle', label: 'Throttle', kind: 'axis', group: 'Axes', none: 'Y axis of the base' },
  { role: 'brake', label: 'Brake', kind: 'axis', group: 'Axes', none: 'Rz axis of the base' },
  { role: 'clutch', label: 'Clutch', kind: 'axis', group: 'Axes' },
  { role: 'handbrake', label: 'Handbrake', kind: 'either', group: 'Axes' },
  { role: 'shift_up', label: 'Shift up', kind: 'button', group: 'Gears' },
  { role: 'shift_down', label: 'Shift down', kind: 'button', group: 'Gears' },
  ...[1, 2, 3, 4, 5, 6, 7].map(n => ({ role: 'h' + n, label: 'H shifter ' + n,
    kind: 'button', group: 'Gears', needs: 'hr' })),
  { role: 'hr', label: 'H shifter reverse', kind: 'button', group: 'Gears' },
  { role: 'starter', label: 'Starter', kind: 'button', group: 'Car' },
  { role: 'bias_up', label: 'Brake bias forward', kind: 'button', group: 'Cockpit aids', game: [148, 2] },
  { role: 'bias_down', label: 'Brake bias rearward', kind: 'button', group: 'Cockpit aids', game: [148, 1] },
  { role: 'tc_up', label: 'TC up', kind: 'button', group: 'Cockpit aids', game: [140, 2] },
  { role: 'tc_down', label: 'TC down', kind: 'button', group: 'Cockpit aids', game: [140, 1] },
  { role: 'abs_up', label: 'ABS up', kind: 'button', group: 'Cockpit aids', game: [144, 2] },
  { role: 'abs_down', label: 'ABS down', kind: 'button', group: 'Cockpit aids', game: [144, 1] },
  { role: 'pit_limiter', label: 'Pit limiter', kind: 'button', group: 'Pits', needs: 'pit_menu', game: [138, null] },
  { role: 'pit_menu', label: 'Pit menu', kind: 'button', group: 'Pits' },
  { role: 'pit_up', label: 'Pit menu up', kind: 'button', group: 'Pits', needs: 'pit_menu', game: [140, 2] },
  { role: 'pit_down', label: 'Pit menu down', kind: 'button', group: 'Pits', needs: 'pit_menu', game: [140, 1] },
  { role: 'pit_left', label: 'Pit menu left', kind: 'button', group: 'Pits', needs: 'pit_menu', game: [144, 1] },
  { role: 'pit_right', label: 'Pit menu right', kind: 'button', group: 'Pits', needs: 'pit_menu', game: [144, 2] },
  { role: 'pit_ok', label: 'Pit menu OK', kind: 'button', group: 'Pits', needs: 'pit_menu' },
  { role: 'pit_return', label: 'Back to pits', kind: 'button', group: 'Pits', needs: 'pit_return' },
  { role: 'cam_next', label: 'Next camera', kind: 'button', group: 'View' },
  { role: 'look_back', label: 'Look back', kind: 'button', group: 'View' },
];

/* Keys evoview handles while driving on a track that its printed key list
   (render.rs KEYS, shown above these) does not spell out. */
const EVO_KEYS_EXTRA = [
  ['cameras', 'C / Shift+C next / previous camera: the car’s own, TV, chase, free'],
  ['car', 'Enter drops the car where the free camera is looking'],
  ['aids', 'T traction control on / off, A ABS on / off'],
  ['wheel', 'Numpad + / − force-feedback strength (kept in ffb.cfg)'],
  ['time', 'O / U time of day ±15 min (Shift: 1 h), ; runs the clock'],
];

let evoProf = null;      // the working Drive profile (config "evoview")
let evoEnvSet = null;    // EVOVIEW_* names the installed exe knows
let evoPoll = null;
let evoSaveT = null;

function evoProfile(saved) {
  const p = Object.assign({ track: '', layout: '', car: '', livery: '', setup: '',
                            knobs: {} }, saved || {});
  if (!p.knobs || typeof p.knobs !== 'object') p.knobs = {};
  return p;
}
function evoSave() {
  clearTimeout(evoSaveT);
  evoSaveT = setTimeout(() => api('evoview/profile', evoProf), 500);
}
function evoVal(id) {
  const k = EVO_KNOBS.find(x => x.id === id);
  const v = evoProf && evoProf.knobs[id];
  return v === undefined ? (k && k.kind === 'select' ? '' : k && k.def) : v;
}
function evoOpt(o) {
  return Array.isArray(o) ? { v: o[0], label: o[1] } : o;
}
// the variables one knob can set - what the exe must know for it to show
function evoNeeds(k) {
  if (k.needs) return [].concat(k.needs);
  if (k.env) return [k.env];
  const out = new Set();
  (k.options || []).forEach(o => Object.keys(evoOpt(o).env || {}).forEach(e => out.add(e)));
  return [...out];
}
function evoHas(k) {
  return !!evoEnvSet && evoNeeds(k).every(n => evoEnvSet.has(n));
}
// the knob table turned into EVOVIEW_* values for one launch
function evoEnv() {
  const env = {};
  EVO_KNOBS.forEach(k => {
    if (!evoHas(k) || (k.when && !k.when(evoVal))) return;
    const v = evoVal(k.id);
    if (k.kind === 'toggle') {
      const out = v ? k.on : k.off;
      if (out != null) env[k.env] = out;
    } else if (k.kind === 'range') {
      if (v != null && Number(v) !== Number(k.def)) env[k.env] = String(v);
    } else {
      const o = (k.options || []).map(evoOpt).find(x => x.v === v);
      if (o && o.env) Object.assign(env, o.env);
      else if (v !== '' && v != null && k.env) env[k.env] = String(v);
    }
  });
  return env;
}

/* One knob as a form control, in the style the other pages use: label.f
   with a select, label.ctl with a checkbox, or a slider whose label carries
   its value. `changed()` is called after the profile is updated (the group
   repaints, so knobs with a `when` come and go). */
function evoKnob(k, changed) {
  const set = v => { evoProf.knobs[k.id] = v; evoSave(); };
  const v = evoVal(k.id);
  let node;
  if (k.kind === 'toggle') {
    node = el('label', 'ctl');
    const cb = el('input');
    cb.type = 'checkbox';
    cb.checked = !!v;
    cb.onchange = () => { set(cb.checked); changed(true); };
    node.append(cb, el('span', null, esc(k.label)
      + (k.hint ? `<div class="tiny dim">${esc(k.hint)}</div>` : '')));
    return node;
  }
  node = el('label', 'f evo-knob');
  const cap = el('span');
  node.append(cap);
  if (k.kind === 'text') {
    cap.textContent = k.label;
    const t = el('input');
    t.type = 'text';
    t.value = v || '';
    t.placeholder = k.placeholder || '';
    if (k.max) t.maxLength = k.max;
    t.spellcheck = false;
    // saved as you type; the group repaints (knobs with a `when`) on leaving
    t.oninput = () => set(t.value.trim());
    t.onchange = () => changed(true);
    node.append(t);
  } else if (k.kind === 'range') {
    const shown = x => (k.fmt ? k.fmt(Number(x)) : String(x));
    const paint = x => {
      cap.innerHTML = esc(k.label) + ' <b>' + esc(shown(x)) + '</b>'
        + (Number(x) === Number(k.def) ? ' <i class="tiny dim">default</i>' : '');
    };
    const r = el('input');
    r.type = 'range';
    r.min = k.min; r.max = k.max; r.step = k.step;
    r.value = v;
    r.oninput = () => paint(r.value);
    r.onchange = () => { set(Number(r.value)); changed(true); };
    // double-click puts evoview's own default back
    r.ondblclick = () => { r.value = k.def; paint(k.def); set(k.def); changed(true); };
    r.title = 'Double-click for the default (' + shown(k.def) + ')';
    paint(v);
    node.append(r);
  } else {
    cap.textContent = k.label;
    const s = el('select');
    (k.options || []).map(evoOpt).forEach(o => {
      const op = el('option', null, esc(o.label));
      op.value = o.v;
      if (o.v === v) op.selected = true;
      s.append(op);
    });
    s.onchange = () => { set(s.value); changed(true); };
    node.append(s);
  }
  if (k.hint) node.append(el('div', 'tiny dim evo-hint', esc(k.hint)));
  return node;
}

/* The knobs for one page, grouped. What this evoview does not know yet goes
   into a closed "Coming" list rather than being offered and ignored. */
function evoKnobs(box, page, opts) {
  opts = opts || {};
  const paint = () => {
    box.innerHTML = '';
    const mine = EVO_KNOBS.filter(k => k.pages.includes(page)
      && (!opts.groups || opts.groups.includes(k.group)));
    const groups = [...new Set(mine.map(k => k.group))];
    const later = [];
    groups.forEach(g => {
      const ks = mine.filter(k => k.group === g);
      const live = ks.filter(evoHas);
      ks.filter(k => !evoHas(k)).forEach(k => later.push(k));
      if (!live.length) return;
      const sec = el('div', 'evo-group');
      if (!opts.flat) sec.append(el('div', 'evo-ghead', esc(g)));
      const grid = el('div', 'evo-grid');
      live.filter(k => !k.when || k.when(evoVal))
        .forEach(k => grid.append(evoKnob(k, () => paint())));
      sec.append(grid);
      box.append(sec);
    });
    if (later.length && !opts.noLater) {
      const d = el('details', 'evo-later');
      d.append(el('summary', 'tiny', 'Coming with a newer evoview (' + later.length + ')'));
      d.append(el('div', 'tiny dim', later.map(k => esc(k.label)
        + ' <span class="evo-tok">' + esc(evoNeeds(k).join(', ')) + '</span>').join('<br>')));
      box.append(d);
    }
  };
  paint();
  return paint;
}

/* A searchable picker dialog in the Play page's style (pk-veil / pk-box /
   drive-row). rows: [{key, img, title, sub, tag}] */
function evoPicker(title, rows, cur, onPick) {
  const veil = el('div', 'pk-veil');
  const box = el('div', 'pk-box pk-wide');
  const head = el('div', 'pk-head');
  const search = el('input');
  search.placeholder = 'Filter…';
  const x = el('button', 'pk-x', '×');
  const body = el('div', 'pk-body');
  const list = el('div', 'list drive-list');
  head.append(el('h3', null, esc(title)), search, x);
  body.append(list);
  box.append(head, body);
  veil.append(box);
  document.body.append(veil);
  const close = () => { veil.remove(); document.removeEventListener('keydown', onKey); };
  const onKey = e => { if (e.key === 'Escape') close(); };
  document.addEventListener('keydown', onKey);
  x.onclick = close;
  veil.onclick = e => { if (e.target === veil) close(); };
  const paint = () => {
    const q = search.value.trim().toLowerCase();
    list.innerHTML = '';
    const hits = rows.filter(r => !q || (r.title + ' ' + r.sub).toLowerCase().includes(q));
    if (!hits.length) { list.append(el('div', 'empty', 'No matches')); return; }
    hits.forEach(r => {
      const row = el('div', 'drive-row' + (r.key === cur ? ' on' : ''));
      if (r.img) {
        const img = el('img');
        img.loading = 'lazy';
        img.alt = '';
        img.src = r.img;
        img.onerror = () => { img.style.visibility = 'hidden'; };
        row.append(img);
      }
      const t = el('div', 'grow');
      t.innerHTML = `<div class="name">${esc(r.title)}</div>`
        + `<div class="tiny dim">${esc(r.sub)}</div>`;
      row.append(t);
      if (r.tag) row.append(el('span', 'pill warn', esc(r.tag)));
      row.onclick = () => { close(); onPick(r.key); };
      list.append(row);
    });
    const on = list.querySelector('.on');
    if (on && !q) on.scrollIntoView({ block: 'center' });
  };
  search.oninput = paint;
  paint();
  search.focus();
}

/* The chosen car / track as a picture card that opens its picker (the Play
   page's .drive-pick). */
function evoTile(box, img, title, sub, open) {
  box.innerHTML = '';
  box.classList.toggle('empty-pick', !img);
  if (img) {
    const i = el('img');
    i.alt = '';
    i.src = img;
    i.onerror = () => { i.remove(); box.classList.add('empty-pick'); };
    box.append(i);
  }
  const bar = el('div', 'dp-bar');
  const t = el('div', 'grow');
  t.innerHTML = `<b>${esc(title || 'Nothing selected')}</b>`
    + (sub ? `<div class="dp-sub">${esc(sub)}</div>` : '');
  bar.append(t, el('span', 'dp-kebab', '⋮'));
  box.append(bar);
  box.onclick = open;
  box.title = 'Click to choose';
}

const evoCarImg = c => 'api/thumb/car?id=' + encodeURIComponent(c.id)
  + (c.preset ? '&preset=' + encodeURIComponent(c.preset) : '') + picV();
const evoTrackImg = f => 'api/thumb/track?folder=' + encodeURIComponent(f);
const evoKm = m => (m ? (m / 1000).toFixed(m < 10000 ? 2 : 1) + ' km' : '');

/* ---------------------------------------------------------------- Drive -- */
async function evoDrivePage() {
  const d = await api('evoview/catalog');
  const p = $('#page');
  p.innerHTML = '';
  if (!d || !d.ok) {
    p.append(el('div', 'err', esc((d && d.error) || 'evoview is not available')));
    return;
  }
  evoProf = evoProfile(d.profile);
  evoEnvSet = new Set(d.env || []);
  if (!d.exe) {
    p.append(el('div', 'err', 'evoview.exe not found. Put it in the tools folder next to '
      + 'ACECM, or set viewer_exe in Settings.'));
  }
  const tracks = d.tracks || [], cars = d.cars || [];
  const sel = evoProf;
  // a saved pick that is no longer installed falls back to the first one
  if (!tracks.some(t => t.folder === sel.track)) {
    const imola = tracks.find(t => t.folder === 'imola');
    sel.track = (imola || tracks[0] || {}).folder || '';
    sel.layout = '';
  }
  if (!cars.some(c => c.id === sel.car)) {
    const m4 = cars.find(c => c.id === 'ks_bmw_m4_gt3');
    sel.car = (m4 || cars[0] || {}).id || '';
    sel.livery = ''; sel.setup = '';
  }

  const wrap = el('div', 'drive evo-drive');
  const left = el('div', 'drive-left');
  const carCol = el('div', 'card drive-col');
  const trkCol = el('div', 'card drive-col');
  const goCol = el('div', 'card drive-col drive-go');
  left.append(carCol, trkCol);
  wrap.append(left, goCol);
  p.append(wrap);
  // the grid fills the window under the header, as Play's does (fitDrive)
  const fit = () => {
    if (!wrap.isConnected) { removeEventListener('resize', fit); return; }
    const top = wrap.getBoundingClientRect().top;
    const pad = parseFloat(getComputedStyle(wrap.parentNode).paddingBottom) || 0;
    wrap.style.height = Math.max(440, Math.round(innerHeight - top - pad)) + 'px';
  };
  addEventListener('resize', fit);
  requestAnimationFrame(fit);

  // ---- car ----------------------------------------------------------------
  carCol.append(el('h2', null, 'Car'));
  const carTile = el('div', 'drive-pick');
  const carOpts = el('div', 'evo-grid evo-under');
  carCol.append(carTile, carOpts);
  const livSel = el('select');
  const setSel = el('select');
  const mkf = (label, node, hint) => {
    const l = el('label', 'f');
    l.append(el('span', null, label), node);
    if (hint) l.append(el('div', 'tiny dim evo-hint', hint));
    return l;
  };
  const setHint = el('div', 'tiny dim evo-hint');
  const setField = mkf('Setup', setSel);
  setField.append(setHint);
  carOpts.append(mkf('Livery', livSel), setField);

  function paintCar() {
    const c = cars.find(x => x.id === sel.car);
    evoTile(carTile, c ? evoCarImg(c) : '', c ? c.label : '', c ? c.id : '', () =>
      evoPicker('Car', cars.map(x => ({ key: x.id, img: evoCarImg(x), title: x.label,
        sub: x.id, tag: x.mod ? 'mod' : '' })), sel.car, id => {
        sel.car = id; sel.livery = ''; sel.setup = '';
        evoSave(); paintCar();
      }));
    paintLiveries();
    paintSetups();
  }
  function paintLiveries() {
    const id = sel.car;
    livSel.innerHTML = '<option value="">Default</option>';
    livSel.disabled = true;
    if (!id) return;
    const load = liveryCache[id]
      || (liveryCache[id] = api('viewer/liveries?id=' + encodeURIComponent(id)));
    load.then(r => {
      if (sel.car !== id) return;
      const list = (r && r.liveries) || [];
      list.slice(1).forEach(l => {
        const o = el('option', null, esc((l.name || l.stem) + (l.number ? '  #' + l.number : '')));
        o.value = l.stem;
        if (l.stem === sel.livery) o.selected = true;
        livSel.append(o);
      });
      livSel.disabled = list.length < 2;
    });
  }
  livSel.onchange = () => { sel.livery = livSel.value; evoSave(); };
  async function paintSetups() {
    const id = sel.car;
    const c = cars.find(x => x.id === id);
    setSel.innerHTML = '<option value="">Stock setup</option>';
    setHint.textContent = '';
    if (!id) return;
    const r = await api('evoview/setups?car=' + encodeURIComponent(id)
      + '&label=' + encodeURIComponent(c ? c.label : ''));
    if (sel.car !== id) return;
    const all = (r && r.setups) || [];
    const mine = all.filter(s => s.fits);
    const byTrack = {};
    mine.forEach(s => { (byTrack[s.track || 'Any track'] = byTrack[s.track || 'Any track'] || []).push(s); });
    Object.keys(byTrack).sort().forEach(t => {
      const g = el('optgroup');
      g.label = t;
      byTrack[t].forEach(s => {
        const o = el('option', null, esc(s.name));
        o.value = s.path;
        if (s.path === sel.setup) o.selected = true;
        g.append(o);
      });
      setSel.append(g);
    });
    setSel.disabled = !d.setups_supported || !mine.length;
    if (!d.setups_supported) {
      setHint.textContent = mine.length
        ? mine.length + ' saved for this car - this evoview cannot load setups yet'
        : 'This evoview cannot load saved setups yet';
    } else if (!mine.length) {
      setHint.textContent = 'No saved setups for this car (Saved Games\\ACE\\Car Setups)';
    }
    if (sel.setup && !mine.some(s => s.path === sel.setup)) { sel.setup = ''; evoSave(); }
  }
  setSel.onchange = () => { sel.setup = setSel.value; evoSave(); };

  // ---- track --------------------------------------------------------------
  trkCol.append(el('h2', null, 'Track'));
  const trkTile = el('div', 'drive-pick');
  const layBox = el('div', 'evo-under');
  trkCol.append(trkTile, layBox);
  function paintTrack() {
    const t = tracks.find(x => x.folder === sel.track);
    const lays = (t && t.layouts) || [];
    if (sel.layout && !lays.some(l => l.id === sel.layout)) sel.layout = '';
    const lay = lays.find(l => l.id === sel.layout) || lays[0];
    evoTile(trkTile, t ? evoTrackImg(t.folder) : '', t ? t.name : '',
      t ? [lay && lays.length > 1 ? lay.label : '', evoKm((lay && lay.length_m) || t.length_m),
           t.folder].filter(Boolean).join(' · ') : '',
      () => evoPicker('Track', tracks.map(x => ({ key: x.folder, img: evoTrackImg(x.folder),
        title: x.name, sub: [x.folder, x.layouts.length > 1 ? x.layouts.length + ' layouts' : '',
          evoKm(x.length_m)].filter(Boolean).join(' · '), tag: x.mod ? 'mod' : '' })),
        sel.track, f => { sel.track = f; sel.layout = ''; evoSave(); paintTrack(); }));
    layBox.innerHTML = '';
    if (lays.length > 1) {
      const s = el('select');
      lays.forEach((l, i) => {
        const o = el('option', null, esc(l.label + (l.length_m ? '  ·  ' + evoKm(l.length_m) : '')));
        o.value = i === 0 ? '' : l.id;
        if ((sel.layout || '') === o.value) o.selected = true;
        s.append(o);
      });
      s.onchange = () => { sel.layout = s.value; evoSave(); paintTrack(); };
      layBox.append(mkf('Layout', s,
        'Sets where the car starts and the line the test driver follows.'));
    }
  }

  // ---- session + Drive ----------------------------------------------------
  goCol.append(el('h2', null, 'Session'));
  const knobBox = el('div');
  evoKnobs(knobBox, 'drive', { groups: ['Driving', 'Session', 'Upscaler'], noLater: true });
  const more = el('details', 'evo-more');
  more.append(el('summary', null, 'Pits and aids'));
  const moreBox = el('div');
  evoKnobs(moreBox, 'drive', { groups: ['Pits', 'Aids'] });
  more.append(moreBox);
  const goBtn = el('button', 'primary go-btn', 'Drive');
  const status = el('div', 'drive-status tiny dim');
  const acts = el('div', 'row wrap evo-acts');
  const stopBtn = el('button', 'sm', 'Stop evoview');
  const cmdBtn = el('button', 'sm', 'Copy command');
  cmdBtn.title = 'The same launch as a PowerShell line, as the test checklists use';
  acts.append(stopBtn, cmdBtn);
  const logBox = el('pre', 'log evo-log');
  logBox.hidden = true;
  goCol.append(knobBox, more, goBtn, status, acts, logBox);
  const dl = d.dlls || {};
  const upNote = () => {
    const u = evoVal('upscaler');
    if (u === 'fsr' && !dl.fsr) {
      return 'FSR is not added yet, so evoview uses TAA. Get AMD’s two FSR files '
        + '(evoview graphics page) and drag them onto this window.';
    }
    if (u === 'dlss' && !dl.dlss) {
      return 'DLSS is not added yet, so evoview uses TAA. Get NVIDIA’s nvngx_dlss.dll '
        + '(evoview graphics page) and drag it onto this window.';
    }
    return '';
  };
  let lastCmd = '';
  cmdBtn.disabled = true;
  cmdBtn.onclick = async () => {
    try { await navigator.clipboard.writeText(lastCmd); toast('Command copied'); }
    catch (e) { toast('Could not copy - it is in the log', true); }
  };

  function paintState(s) {
    s = s || {};
    const busy = s.state === 'starting' || s.state === 'running';
    goBtn.textContent = busy ? 'Driving…' : 'Drive';
    stopBtn.hidden = !busy;
    const lines = {
      starting: `Loading ${s.what || ''}…`,
      running: `Driving ${s.what || ''} · ${s.seconds || 0} s · Esc in evoview quits`,
      exited: s.what ? `Last drive: ${s.what}` : '',
      error: `<b>evoview could not start ${esc(s.what || '')}</b><br>${esc(s.detail || '')}`,
      idle: '',
    };
    status.innerHTML = s.state === 'error' ? lines.error : esc(lines[s.state] || '');
    status.classList.toggle('dim', s.state !== 'error');
    status.style.color = s.state === 'error' ? 'var(--red)' : '';
    const log = (s.log || '').trim();
    logBox.hidden = !log || !(busy || s.state === 'error');
    logBox.textContent = log;
    logBox.scrollTop = logBox.scrollHeight;
    if (!busy && evoPoll) { clearInterval(evoPoll); evoPoll = null; }
    const n = upNote();
    if (n && !busy && s.state !== 'error') status.textContent = n;
  }
  function watch() {
    if (evoPoll) return;
    evoPoll = setInterval(async () => {
      if (_wanted !== 'evodrive') { clearInterval(evoPoll); evoPoll = null; return; }
      paintState(await api('evoview/state'));
    }, 1500);
  }

  async function drive(replace) {
    if (!sel.track || !sel.car) { toast('Pick a track and a car first', true); return; }
    goBtn.disabled = true;
    const r = await api('evoview/launch', { profile: evoProf, env: evoEnv(), replace: !!replace });
    goBtn.disabled = false;
    if (r && r.busy) {
      if (await ask('evoview is already running (' + (r.error || '') + ').\n\nStop it and start '
                    + 'this drive instead?', 'Restart')) drive(true);
      return;
    }
    if (!r || !r.ok) { paintState({ state: 'error', detail: (r && r.error) || 'launch failed' }); return; }
    lastCmd = r.command || '';
    cmdBtn.disabled = !lastCmd;
    if (r.skipped && r.skipped.length) toast('Left out: ' + r.skipped.join(', '), true);
    paintState({ state: 'starting', what: r.what });
    watch();
  }
  goBtn.onclick = () => drive(false);
  stopBtn.onclick = async () => {
    stopBtn.disabled = true;
    await api('evoview/stop', {});
    stopBtn.disabled = false;
    paintState(await api('evoview/state'));
  };

  paintCar();
  paintTrack();
  paintState(d.state);
  if (d.state && (d.state.state === 'starting' || d.state.state === 'running')) watch();
  // a picked default (first visit) is worth remembering straight away
  evoSave();
}

/* ------------------------------------------------------------- Controls -- */
async function evoControlsPage() {
  const tokens = [...new Set(EVO_ROLES.map(r => r.needs || r.role))].join(',');
  const [c, caps, cfg] = await Promise.all([api('evoview/controls'),
    api('evoview/caps?t=' + encodeURIComponent(tokens)), api('config')]);
  const p = $('#page');
  p.innerHTML = '';
  if (!c || !c.ok) { p.append(el('div', 'err', esc((c && c.error) || 'cannot read the controls'))); return; }
  evoProf = evoProfile(cfg && cfg.evoview);
  evoEnvSet = new Set((caps && caps.env) || []);
  const has = (caps && caps.has) || {};
  const roleOk = r => !!has[r.needs || r.role];
  const binds = c.binds || [];
  const bindOf = role => binds.find(b => b.role === role);
  const devs = c.devices || [];
  const cfgDevs = [...new Set(binds.map(b => b.dev).filter(Boolean))];
  const busy = !!c.drive_running;

  // the game's own button for a role: what evoview falls back to
  const gameOf = r => {
    if (!r.game) return null;
    for (const g of c.game || []) {
      const hit = (g.commands || []).find(x => !x.shift && x.action === r.game[0]
        && (r.game[1] == null || x.payload === r.game[1]));
      if (hit) return { dev: g.device, button: hit.button };
    }
    return null;
  };

  async function wizard(roles, what) {
    if (!await ask('evoview’s wheel setup opens in a console window. Follow its '
      + 'prompts there: for each control, rest then move, pressing Enter each time.\n\n'
      + (roles.length ? 'Only ' + what + ' is asked for; the rest of wheel.cfg is kept.'
                      : 'This walks through every control and replaces wheel.cfg.'),
      'Open setup')) return;
    const r = await api('evoview/wheel_setup', { roles });
    if (r && r.ok) {
      toast('Wheel setup is open in its own window - come back here when it says saved');
      // wheel.cfg changes when the wizard finishes; look again on return
      addEventListener('focus', () => { if (_page === 'evocontrols') evoControlsPage(); }, { once: true });
    }
  }

  // ---- wheel --------------------------------------------------------------
  const wc = el('div', 'card');
  wc.append(el('h2', null, 'Wheel'));
  wc.append(el('div', 'tiny dim', 'evoview reads its own mapping, <code>'
    + esc(c.wheel_cfg) + '</code>, written by its wheel setup. Buttons are DirectInput '
    + 'numbers counted from 0, the same as the game’s controls file.'));
  const dl = el('div', 'evo-devs');
  if (!devs.length) dl.append(el('div', 'tiny dim', 'No game controller is answering right now.'));
  devs.forEach(dv => {
    const used = cfgDevs.some(n => n.toLowerCase() === dv.name.toLowerCase());
    dl.append(el('div', 'evo-dev', `<span class="pill ${used ? 'on' : 'off'}"><i class="dot"></i>`
      + `${used ? 'mapped' : 'connected'}</span> <b>${esc(dv.name)}</b> `
      + `<span class="tiny dim">${dv.axes} axes · ${dv.buttons} buttons · VID ${esc(dv.vid)} PID ${esc(dv.pid)}</span>`));
  });
  cfgDevs.filter(n => !devs.some(dv => dv.name.toLowerCase() === n.toLowerCase())).forEach(n => {
    dl.append(el('div', 'evo-dev', '<span class="pill warn"><i class="dot"></i>not found</span> '
      + `<b>${esc(n)}</b> <span class="tiny dim">in wheel.cfg, not connected</span>`));
  });
  wc.append(dl);
  const wrow = el('div', 'row wrap');
  wrow.style.marginTop = '10px';
  const full = el('button', 'sm primary', c.exists ? 'Run wheel setup again…' : 'Run wheel setup…');
  full.disabled = busy;
  full.onclick = () => wizard([], 'everything');
  wrow.append(full);
  if (busy) wrow.append(el('span', 'tiny dim', 'Stop the drive first: the setup needs the wheel to itself.'));
  wc.append(wrow);
  p.append(wc);

  // ---- bindings -----------------------------------------------------------
  const bc = el('div', 'card');
  bc.append(el('h2', null, 'Bindings'));
  bc.append(el('div', 'tiny dim', 'Detect runs evoview’s setup for that one control. '
    + 'Set enters a button number by hand. A role left empty falls back to the game’s own '
    + 'button where evoview knows one, or to the default shown.'));
  const t = el('table', 'evo-binds');
  t.innerHTML = '<thead><tr><th>Control</th><th>Binding</th><th></th></tr></thead>';
  const tb = el('tbody');
  t.append(tb);
  bc.append(t);
  const later = [];
  let group = null;
  const devChoices = [...new Set([...cfgDevs, ...devs.map(dv => dv.name)])];
  EVO_ROLES.forEach(r => {
    if (!roleOk(r)) { later.push(r); return; }
    if (r.group !== group) {
      group = r.group;
      tb.append(el('tr', 'evo-ghrow', `<td colspan="3">${esc(group)}</td>`));
    }
    const b = bindOf(r.role);
    const g = !b && gameOf(r);
    let what;
    if (b && b.kind === 'axis') {
      what = `axis ${b.axis} <span class="tiny dim">(${b.rest} → ${b.full}) · ${esc(b.dev)}</span>`;
    } else if (b) {
      what = `button ${b.button} <span class="tiny dim">· ${esc(b.dev)}</span>`;
    } else if (g) {
      what = `button ${g.button} <span class="tiny dim">· ${esc(g.dev)} · the game’s</span>`;
    } else {
      what = `<span class="dim">${esc(r.none || 'not bound')}</span>`;
    }
    const tr = el('tr', null, `<td>${esc(r.label)}<div class="tiny dim">${esc(r.role)}</div></td>`
      + `<td>${what}</td>`);
    const td = el('td', 'evo-bact');
    const det = el('button', 'sm', 'Detect');
    det.disabled = busy;
    det.onclick = () => wizard([r.role], r.label);
    td.append(det);
    if (r.kind !== 'axis') {
      const setb = el('button', 'sm', 'Set');
      setb.onclick = () => {
        td.innerHTML = '';
        const n = el('input');
        n.type = 'number'; n.min = 0; n.max = 127;
        n.value = b && b.kind === 'button' ? b.button : (g ? g.button : '');
        n.style.width = '76px';
        const dsel = el('select');
        dsel.style.width = 'auto';
        (devChoices.length ? devChoices : ['']).forEach(name => {
          const o = el('option', null, esc(name || 'no device found'));
          o.value = name;
          if (name === ((b && b.dev) || (g && g.dev))) o.selected = true;
          dsel.append(o);
        });
        const ok = el('button', 'sm primary', 'Save');
        const no = el('button', 'sm', 'Cancel');
        ok.onclick = async () => {
          const res = await api('evoview/bind', { role: r.role, button: Number(n.value), dev: dsel.value });
          if (res && res.ok) { toast(r.label + ': button ' + n.value); evoControlsPage(); }
        };
        no.onclick = () => evoControlsPage();
        td.append(n, dsel, ok, no);
        n.focus();
      };
      td.append(setb);
    }
    if (b) {
      const clr = el('button', 'sm danger', 'Clear');
      clr.onclick = async () => {
        const res = await api('evoview/bind', { role: r.role, clear: true });
        if (res && res.ok) evoControlsPage();
      };
      td.append(clr);
    }
    tr.append(td);
    tb.append(tr);
  });
  if (later.length) {
    const d = el('details', 'evo-later');
    d.append(el('summary', 'tiny', 'Coming with a newer evoview (' + later.length + ')'));
    d.append(el('div', 'tiny dim', later.map(r => esc(r.label)
      + ' <span class="evo-tok">' + esc(r.role) + '</span>').join('<br>')));
    bc.append(d);
  }
  p.append(bc);

  // ---- force feedback -----------------------------------------------------
  const fc = el('div', 'card');
  fc.append(el('h2', null, 'Force feedback'));
  fc.append(el('div', 'tiny dim', 'On top of the game’s own FFB settings, which evoview reads '
    + 'from Saved Games. Strength is the same number Numpad + / − changes in the car.'));
  const boost = c.ffb_boost || 1;
  const lab = el('label', 'f evo-knob');
  const cap = el('span');
  const r = el('input');
  r.type = 'range'; r.min = -1.6; r.max = 1.6; r.step = 0.02;   // log2: x0.33 .. x3
  r.value = Math.log2(boost);
  const shown = () => {
    const v = Math.pow(2, Number(r.value));
    cap.innerHTML = 'Strength <b>×' + v.toFixed(2) + '</b>'
      + (Math.abs(v - 1) < 0.01 ? ' <i class="tiny dim">default</i>' : '');
    return v;
  };
  r.oninput = shown;
  r.onchange = async () => {
    const v = shown();
    const res = await api('evoview/ffb', { boost: Math.round(v * 100) / 100 });
    if (res && res.ok) toast('FFB strength ×' + res.boost);
  };
  r.ondblclick = () => { r.value = 0; r.onchange(); };
  shown();
  lab.append(cap, r);
  const fgrid = el('div', 'evo-grid');
  fgrid.append(lab);
  fc.append(fgrid);
  const fk = el('div');
  evoKnobs(fk, 'controls', { flat: true });
  fc.append(fk);
  fc.append(el('div', 'tiny dim evo-hint', 'Damper, smoothing and invert apply from the next Drive.'));
  p.append(fc);

  // ---- remembered cockpit aids (gfx-controls writes aids.cfg) -------------
  if ((c.aids || []).length) {
    const ac = el('div', 'card');
    ac.append(el('h2', null, 'Cockpit aids per car'));
    ac.append(el('div', 'tiny dim', 'What evoview remembered from your last drive in each car '
      + '(aids.cfg). Change them in the car with the aid buttons or the numpad.'));
    const at = el('table');
    at.innerHTML = '<thead><tr><th>Car</th><th>Brake bias</th><th>TC</th><th>ABS</th></tr></thead>';
    const ab = el('tbody');
    c.aids.forEach(a => ab.append(el('tr', null, `<td>${esc(a.car)}</td><td>${esc(a.bias)} %</td>`
      + `<td>${a.tc === '-1' ? '—' : esc(a.tc)}</td><td>${a.abs === '-1' ? '—' : esc(a.abs)}</td>`)));
    at.append(ab);
    ac.append(at);
    p.append(ac);
  }

  // ---- keyboard -----------------------------------------------------------
  const kc = el('div', 'card');
  kc.append(el('h2', null, 'Keyboard'));
  kc.append(el('div', 'tiny dim', 'evoview’s keys, as this evoview.exe prints them at start '
    + '(F1 shows the same list in the window).'));
  kc.append(evoKeyCard((caps && caps.keys) || ''));
  p.append(kc);

  // ---- the game's own bindings (reference) --------------------------------
  const gc = el('div', 'card');
  gc.append(el('h2', null, 'The game’s bindings'));
  gc.append(el('div', 'tiny dim', 'Read only, from <code>' + esc(c.game_file) + '</code>. '
    + 'Use these numbers to bind the same buttons in evoview.'));
  const gd = el('details');
  // the file lists every controller ever seen; only those with bindings matter
  const gdevs = (c.game || []).filter(g => (g.commands || []).length);
  const n = gdevs.reduce((a, g) => a + g.commands.length, 0);
  gd.append(el('summary', 'tiny', n ? n + ' bindings on ' + gdevs.length + ' device(s)'
                                       : 'No controller bindings found'));
  const gt = el('table');
  gt.innerHTML = '<thead><tr><th>Device</th><th>Button</th><th>Action</th></tr></thead>';
  const gb = el('tbody');
  gdevs.forEach(g => g.commands.forEach(x => {
    const act = (x.name || ('action ' + x.action)).replace(/_/g, ' ')
      + (x.payload ? ' <span class="tiny dim">' + (x.payload === 2 ? 'up' : x.payload === 1 ? 'down' : '#' + x.payload) + '</span>' : '');
    gb.append(el('tr', null, `<td class="tiny">${esc(g.device)}</td><td>${x.button}`
      + `${x.shift ? ' <span class="tiny dim">+ shift</span>' : ''}</td><td>${act}</td>`));
  }));
  gt.append(gb);
  gd.append(gt);
  gc.append(gd);
  p.append(gc);
}

/* render.rs KEYS ("  section  text" + continuation lines) as a two-column
   card, plus the track-drive keys that list does not spell out. */
function evoKeyCard(text) {
  const rows = [];
  (text || '').split('\n').slice(1).forEach(line => {
    const m = line.match(/^ {2}(\S+(?: \S+)?)\s{2,}(.*)$/);
    if (m && !/^\s{4,}/.test(line)) rows.push([m[1], m[2].trim()]);
    else if (line.trim() && rows.length && /^\s{4,}/.test(line)) rows[rows.length - 1][1] += ' ' + line.trim();
    else if (line.trim()) rows.push(['', line.trim()]);
  });
  const box = el('div', 'evo-keys');
  const add = (k, v, extra) => {
    box.append(el('div', 'evo-kk' + (extra ? ' extra' : ''), esc(k)),
               el('div', 'evo-kv', esc(v)));
  };
  if (!rows.length) box.append(el('div', 'tiny dim', 'This evoview.exe carries no key list.'));
  rows.forEach(([k, v]) => add(k, v));
  box.append(el('div', 'evo-ksep tiny dim', 'On a track, also'));
  EVO_KEYS_EXTRA.forEach(([k, v]) => add(k, v, true));
  return box;
}

/* ------------------------------------------------------------- Graphics -- */
async function evoGraphicsPage() {
  const [caps, cfg] = await Promise.all([api('evoview/caps'), api('config')]);
  const p = $('#page');
  p.innerHTML = '';
  if (!caps || !caps.ok) { p.append(el('div', 'err', esc((caps && caps.error) || 'evoview.exe not found'))); return; }
  evoProf = evoProfile(cfg && cfg.evoview);
  evoEnvSet = new Set(caps.env || []);
  const dl = caps.dlls || {};
  const intro = el('div', 'card');
  intro.append(el('h2', null, 'Graphics'));
  intro.append(el('div', 'tiny dim', 'Applied from the next Drive. Sliders show evoview’s '
    + 'own default; double-click one to go back to it. Upscaler DLLs beside evoview.exe: DLSS '
    + (dl.dlss ? 'yes' : 'no') + ', FSR ' + (dl.fsr ? 'yes' : 'no') + '.'));
  p.append(intro);
  p.append(evoFsrCard(dl), evoDlssCard(dl));
  const groups = [...new Set(EVO_KNOBS.filter(k => k.pages.includes('graphics')).map(k => k.group))];
  groups.forEach(g => {
    const c = el('div', 'card');
    c.append(el('h2', null, esc(g)));
    const box = el('div');
    evoKnobs(box, 'graphics', { groups: [g], flat: true });
    c.append(box);
    p.append(c);
  });
}

/* ------------------------------------------------------- upscaler DLLs -- */
/* The vendors' upscaler files are not shipped with ACECM: AMD's two FSR
   DLLs (36 MB) and NVIDIA's nvngx_dlss.dll (its terms keep it out of a GPL
   app). The player gets them from the vendor and drops them anywhere on the
   window - loose, or the zip they came in - or picks them on the evoview
   graphics page. The app keeps only files signed by their maker. */
function evoIsUpscalerDrop(files) {
  return files.length > 0 && files.every(f => {
    const n = f.name || '';
    return /^amd_fidelityfx_.*\.dll$/i.test(n) || /^nvngx_dlss\.dll$/i.test(n)
      || /(fidelityfx|dlss).*\.zip$/i.test(n);
  });
}

async function evoAddDlls(files) {
  const id = dropId();
  const prog = dropProgress();
  const total = files.reduce((n, f) => n + (f.size || 0), 0);
  const before = (await api('evoview/caps') || {}).dlls || {};
  let done = 0;
  prog.show('Adding upscaler files…');
  progClaim('local');
  try {
    for (const f of files) {
      const r = await putFile('/api/drop/part?id=' + encodeURIComponent(id) + '&name=' + encodeURIComponent(f.name), f,
        loaded => prog.set(total ? (done + loaded) / total : 0, `${f.name} - ${mb(done + loaded)} of ${mb(total)}`));
      if (!r.ok) { toast(r.error || 'Upload failed', true); return; }
      done += f.size || 0;
    }
    prog.busy('Checking the signature…', '');
    const r = await api('evoview/add_dlls', { id });
    const errs = (r && r.errors) || [];
    const now = (r && r.dlls) || {};
    const added = (r && r.added) || [];
    const ready = [now.fsr && !before.fsr && 'FSR', now.dlss && !before.dlss && 'DLSS'].filter(Boolean);
    if (ready.length) toast(ready.join(' and ') + ' is ready - pick it under Upscaler');
    else if (added.some(n => /fidelityfx/.test(n)) && !now.fsr) toast('Added ' + added.join(', ') + ' - FSR still needs '
      + (added.some(n => /loader/.test(n)) ? 'amd_fidelityfx_upscaler_vk.dll' : 'amd_fidelityfx_loader_vk.dll'), true);
    else if (added.length) toast('Added ' + added.join(', '));
    if (errs.length) toast(errs.join('; '), true);
    if (_wanted === 'evographics' || _wanted === 'evodrive') go(_wanted);
  } finally {
    prog.hide();
    progRelease('local');
  }
}

/* One card per vendor: what to get, a button to the vendor's page, a picker. */
function evoDllCard(title, have, what, files, page, how) {
  const c = el('div', 'card');
  c.append(el('h2', null, title));
  if (have) {
    c.append(el('div', 'tiny dim', 'Added. Pick it under Upscaler below.'));
    return c;
  }
  c.append(el('div', 'tiny', what + ' needs ' + files + '. Download '
    + (files.includes(' and ') ? 'both' : 'it') + ' from the maker’s page, then drag '
    + (files.includes(' and ') ? 'them' : 'it') + ' onto this window.'));
  const row = el('div', 'row wrap');
  const get = el('button', 'sm primary', 'Open the download page');
  get.onclick = async () => {
    const r = await api(page, {});
    if (!(r && r.ok)) toast('Could not open a browser', true);
  };
  const pick = el('button', 'sm', 'Choose files…');
  const inp = el('input');
  inp.type = 'file';
  inp.multiple = true;
  inp.accept = '.dll,.zip';
  inp.hidden = true;
  inp.onchange = () => { const f = [...inp.files]; inp.value = ''; if (f.length) evoAddDlls(f); };
  pick.onclick = () => inp.click();
  row.append(get, pick, inp);
  c.append(row);
  c.append(el('div', 'tiny dim', how + ' Only files signed by their maker are kept.'));
  return c;
}

function evoFsrCard(dl) {
  return evoDllCard('AMD FSR', dl.fsr, 'FSR (a sharper upscaler for any GPU)',
    '<b>amd_fidelityfx_loader_vk.dll</b> and <b>amd_fidelityfx_upscaler_vk.dll</b> from AMD',
    'evoview/fsr_page', 'On AMD’s page, open each file and use “Download raw file”.');
}

function evoDlssCard(dl) {
  return evoDllCard('NVIDIA DLSS', dl.dlss, 'DLSS (NVIDIA RTX graphics cards only)',
    '<b>nvngx_dlss.dll</b> from NVIDIA',
    'evoview/dlss_page', 'On NVIDIA’s page, open nvngx_dlss.dll and use “Download raw file”.');
}
