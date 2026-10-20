"""Offline FMOD Studio render harness (no audio device: WAVWRITER_NRT / NOSOUND_NRT).

Loads banks into a game's own FMOD Studio runtime - Assetto Corsa EVO's fmodstudio.dll (2.03) or
Assetto Corsa's fmodstudio64.dll (1.08) - lists their events and renders one event to a WAV while
its parameters follow a schedule.

  python tools/fmodrender.py list   --rt evo|ac1 BANK... [--guids GUIDs.txt]
  python tools/fmodrender.py render --rt evo|ac1 BANK... --event PATH|{GUID} --out OUT.wav
         [--secs 4] [--pos x,y,z] [--set name=value@t ...]

`--set rpms=1000@0 --set rpms=8000@4` ramps linearly between the keys of one parameter (a single key
holds). Banks load in the order given (strings banks first). Nothing is played on the speakers.
Environment: ACECONV_EVO_DIR / ACECONV_AC1_DIR override the install folders."""
import argparse
import ctypes as C
import os
import struct
import sys
import time

EVO = os.environ.get('ACECONV_EVO_DIR', r'C:\Program Files (x86)\Steam\steamapps\common\Assetto Corsa EVO')
AC1 = os.environ.get('ACECONV_AC1_DIR', r'C:\Program Files (x86)\Steam\steamapps\common\assettocorsa')
RATE = 48000
BLOCK = 512


class VEC(C.Structure):
    _fields_ = [('x', C.c_float), ('y', C.c_float), ('z', C.c_float)]


class ATTR3D(C.Structure):
    _fields_ = [('position', VEC), ('velocity', VEC), ('forward', VEC), ('up', VEC)]


class GUID(C.Structure):
    _fields_ = [('d1', C.c_uint32), ('d2', C.c_uint16), ('d3', C.c_uint16), ('d4', C.c_ubyte * 8)]

    @staticmethod
    def parse(s):
        h = s.strip('{}').replace('-', '')
        g = GUID()
        g.d1, g.d2, g.d3 = int(h[0:8], 16), int(h[8:12], 16), int(h[12:16], 16)
        b = bytes.fromhex(h[16:])
        for i in range(8):
            g.d4[i] = b[i]
        return g

    def __str__(self):
        d4 = bytes(self.d4).hex()
        return '{%08x-%04x-%04x-%s-%s}' % (self.d1, self.d2, self.d3, d4[:4], d4[4:])


class PDESC2(C.Structure):  # FMOD_STUDIO_PARAMETER_DESCRIPTION, 2.03
    _fields_ = [('name', C.c_char_p), ('id', C.c_uint32 * 2), ('minimum', C.c_float), ('maximum', C.c_float),
                ('defaultvalue', C.c_float), ('type', C.c_int), ('flags', C.c_uint), ('guid', GUID)]


class PDESC1(C.Structure):  # FMOD_STUDIO_PARAMETER_DESCRIPTION, 1.08
    _fields_ = [('name', C.c_char_p), ('index', C.c_int), ('minimum', C.c_float), ('maximum', C.c_float),
                ('defaultvalue', C.c_float), ('type', C.c_int)]


class Rt:
    def __init__(self, kind, out_wav=None):
        self.kind = kind
        if kind == 'evo':
            os.add_dll_directory(EVO)
            self.core = C.WinDLL(os.path.join(EVO, 'fmod.dll'))
            self.st = C.WinDLL(os.path.join(EVO, 'fmodstudio.dll'))
            ver = 0x00020313
        else:
            os.add_dll_directory(AC1)
            self.core = C.WinDLL(os.path.join(AC1, 'fmod64.dll'))
            self.st = C.WinDLL(os.path.join(AC1, 'fmodstudio64.dll'))
            ver = 0x00010812
        st = self.st
        self.sys = C.c_void_p()
        self.ck(st.FMOD_Studio_System_Create(C.byref(self.sys), ver), 'create')
        cs = C.c_void_p()
        get_core = st.FMOD_Studio_System_GetCoreSystem if kind == 'evo' else st.FMOD_Studio_System_GetLowLevelSystem
        self.ck(get_core(self.sys, C.byref(cs)), 'core system')
        # 5 = WAVWRITER_NRT, 2 = NOSOUND_NRT: no audio device, the update loop is the clock
        self.ck(self.core.FMOD_System_SetOutput(cs, 5 if out_wav else 2), 'output')
        self.ck(self.core.FMOD_System_SetSoftwareFormat(cs, RATE, 3, 0), 'format')
        self.ck(self.core.FMOD_System_SetDSPBufferSize(cs, BLOCK, 4), 'dsp buffer')
        if kind == 'evo':
            for pl in ('fmod_distance_filter.dll', 'fmod_gain.dll', 'resonanceaudio.dll'):
                h = C.c_uint()
                self.core.FMOD_System_LoadPlugin(cs, os.path.join(EVO, pl).encode(), C.byref(h), 0)
        self.wav = C.create_string_buffer(os.path.abspath(out_wav).encode()) if out_wav else None
        # studio SYNCHRONOUS_UPDATE (2), core NORMAL; extradriverdata = the WAV file name
        self.ck(st.FMOD_Studio_System_Initialize(self.sys, 256, 2, 0, self.wav), 'initialize')
        self.banks = []

    @staticmethod
    def ck(r, what):
        if r:
            raise RuntimeError('%s: FMOD error %d' % (what, r))

    def load(self, path):
        b = C.c_void_p()
        self.ck(self.st.FMOD_Studio_System_LoadBankFile(self.sys, os.path.abspath(path).encode(), 0, C.byref(b)), 'load ' + path)
        self.banks.append(b)
        return b

    def event(self, ref):
        ed = C.c_void_p()
        if ref.startswith('{'):
            self.ck(self.st.FMOD_Studio_System_GetEventByID(self.sys, C.byref(GUID.parse(ref)), C.byref(ed)), 'event ' + ref)
        else:
            self.ck(self.st.FMOD_Studio_System_GetEvent(self.sys, ref.encode(), C.byref(ed)), 'event ' + ref)
        return ed

    def events(self):
        out = []
        for b in self.banks:
            n = C.c_int()
            self.st.FMOD_Studio_Bank_GetEventCount(b, C.byref(n))
            if n.value == 0:
                continue
            arr = (C.c_void_p * n.value)()
            got = C.c_int()
            self.st.FMOD_Studio_Bank_GetEventList(b, arr, n.value, C.byref(got))
            out += [C.c_void_p(arr[i]) for i in range(got.value)]
        return out

    def ev_id(self, ed):
        g = GUID()
        self.st.FMOD_Studio_EventDescription_GetID(ed, C.byref(g))
        return str(g)

    def ev_path(self, ed):
        buf = C.create_string_buffer(512)
        n = C.c_int()
        r = self.st.FMOD_Studio_EventDescription_GetPath(ed, buf, 512, C.byref(n))
        return buf.value.decode() if r == 0 else None

    def params(self, ed):
        n = C.c_int()
        cnt = self.st.FMOD_Studio_EventDescription_GetParameterDescriptionCount if self.kind == 'evo' else self.st.FMOD_Studio_EventDescription_GetParameterCount
        cnt(ed, C.byref(n))
        out = []
        for i in range(n.value):
            if self.kind == 'evo':
                d = PDESC2()
                self.ck(self.st.FMOD_Studio_EventDescription_GetParameterDescriptionByIndex(ed, i, C.byref(d)), 'parameter %d' % i)
            else:
                d = PDESC1()
                self.ck(self.st.FMOD_Studio_EventDescription_GetParameterByIndex(ed, i, C.byref(d)), 'parameter %d' % i)
            out.append((d.name.decode(), d.minimum, d.maximum, d.defaultvalue, d.type))
        return out

    def set_param(self, inst, name, v):
        if self.kind == 'evo':
            return self.st.FMOD_Studio_EventInstance_SetParameterByName(inst, name.encode(), C.c_float(v), 0)
        return self.st.FMOD_Studio_EventInstance_SetParameterValue(inst, name.encode(), C.c_float(v))

    def update(self):
        self.ck(self.st.FMOD_Studio_System_Update(self.sys), 'update')

    def release(self):
        self.st.FMOD_Studio_System_Release(self.sys)


def guid_names(path):
    m = {}
    if path and os.path.isfile(path):
        for line in open(path, encoding='utf-8', errors='replace'):
            p = line.split(None, 1)
            if len(p) == 2:
                m[p[0].lower()] = p[1].strip()
    return m


def schedule(sets):
    keys = {}
    for s in sets:
        k, v = s.split('=', 1)
        t = 0.0
        if '@' in v:
            v, t = v.split('@')
        keys.setdefault(k, []).append((float(t), float(v)))
    for k in keys:
        keys[k].sort()
    return keys


def value_at(ks, t):
    if t <= ks[0][0]:
        return ks[0][1]
    for (t0, v0), (t1, v1) in zip(ks, ks[1:]):
        if t <= t1:
            return v0 + (v1 - v0) * (t - t0) / max(t1 - t0, 1e-9)
    return ks[-1][1]


def trim(path, frames):
    """Drop the first `frames` frames of a RIFF WAV (in place)."""
    b = bytearray(open(path, 'rb').read())
    p, fmt = 12, None
    while p + 8 <= len(b):
        cid, n = bytes(b[p:p + 4]), struct.unpack_from('<I', b, p + 4)[0]
        if cid == b'fmt ':
            fmt = struct.unpack_from('<HHIIHH', b, p + 8)
        if cid == b'data' and fmt:
            align = fmt[4]
            n = min(n, len(b) - p - 8)
            cut = min(frames * align, n)
            del b[p + 8:p + 8 + cut]
            struct.pack_into('<I', b, p + 4, n - cut)
            struct.pack_into('<I', b, 4, len(b) - 8)
            break
        p += 8 + n + (n & 1)
    open(path, 'wb').write(b)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('cmd', choices=['list', 'render'])
    ap.add_argument('banks', nargs='+')
    ap.add_argument('--rt', default='evo', choices=['evo', 'ac1'])
    ap.add_argument('--event')
    ap.add_argument('--out')
    ap.add_argument('--secs', type=float, default=4.0)
    ap.add_argument('--guids')
    ap.add_argument('--pos', default='0,0,3')
    ap.add_argument('--set', action='append', default=[])
    a = ap.parse_args()
    rt = Rt(a.rt, a.out if a.cmd == 'render' else None)
    names = guid_names(a.guids)
    for b in a.banks:
        rt.load(b)
    if a.cmd == 'list':
        for ed in rt.events():
            gid = rt.ev_id(ed)
            print('%s %s' % (gid, rt.ev_path(ed) or names.get(gid, '?')))
            for p in rt.params(ed):
                print('    %-26s %10.3f %10.3f  default %.3f  type %d' % p)
        rt.release()
        return
    ed = rt.event(a.event)
    rt.ck(rt.st.FMOD_Studio_EventDescription_LoadSampleData(ed), 'load sample data')
    # sample loading runs on FMOD's loader thread and finishes in an update; every update renders a
    # block into the WAV, so the blocks spent loading are cut from the file afterwards
    pre = 0
    for _ in range(4000):
        s = C.c_int()
        rt.st.FMOD_Studio_EventDescription_GetSampleLoadingState(ed, C.byref(s))
        if s.value == 3:  # LOADED
            break
        rt.update()
        pre += 1
        time.sleep(0.001)
    else:
        print('note: sample data still loading', file=sys.stderr)
    inst = C.c_void_p()
    rt.ck(rt.st.FMOD_Studio_EventDescription_CreateInstance(ed, C.byref(inst)), 'create instance')
    lis = ATTR3D(VEC(0, 0, 0), VEC(0, 0, 0), VEC(0, 0, 1), VEC(0, 1, 0))
    if a.rt == 'evo':
        rt.st.FMOD_Studio_System_SetListenerAttributes(rt.sys, 0, C.byref(lis), None)
    else:
        rt.st.FMOD_Studio_System_SetListenerAttributes(rt.sys, 0, C.byref(lis))
    x, y, z = (float(v) for v in a.pos.split(','))
    rt.st.FMOD_Studio_EventInstance_Set3DAttributes(inst, C.byref(ATTR3D(VEC(x, y, z), VEC(0, 0, 0), VEC(0, 0, 1), VEC(0, 1, 0))))
    keys = schedule(a.set)
    bad = set()

    def apply(t):
        for k, ks in keys.items():
            if rt.set_param(inst, k, value_at(ks, t)) and k not in bad:
                bad.add(k)
                print('note: the event has no parameter %s' % k, file=sys.stderr)
    apply(0.0)
    rt.ck(rt.st.FMOD_Studio_EventInstance_Start(inst), 'start')
    t, dt = 0.0, BLOCK / RATE
    while t < a.secs:
        apply(t)
        rt.update()
        t += dt
    rt.st.FMOD_Studio_EventInstance_Stop(inst, 1)
    rt.update()
    rt.release()
    trim(a.out, pre * BLOCK)
    print('wrote %s (%d bytes)' % (a.out, os.path.getsize(a.out)))


if __name__ == '__main__':
    main()
