"""Trampoline hooks: the part every patch we wrote reinvented, done once.

A hook replaces instructions at a site with a jump to a cave, runs your payload,
then executes the ORIGINAL instructions and jumps back. The whole difficulty is
that the displaced instructions no longer live where they were assembled, so
anything position-dependent silently points somewhere else.

This module handles that properly:

  * decodes whole instructions (never splits one) until >= 5 bytes are covered
  * relocates rip-relative operands - `lea rax,[rip+x]` moved into a cave points
    at garbage unless its displacement is recomputed
  * rewrites E8/E9 rel32 call/jmp targets for the new address
  * REFUSES instructions it cannot relocate safely (short Jcc/loop, whose rel8
    range cannot reach back from a cave) rather than emitting something subtly
    wrong

It produces `sites` for patching.Patch, so a hook inherits verification,
build-keying, backup and restore for free.

⚠ Payload rules, learned the hard way:
  * `.text` is r-x at runtime - your payload may not write to itself or to any
    literal it carries. Scratch goes on the stack; tables go in .data page slack.
  * Preserve every register you touch. The displaced instructions run after your
    payload and expect the original machine state.
"""
import struct

try:
    import capstone
except ImportError:                      # keep the app importable without it
    capstone = None

JMP_REL32 = 0xE9
CALL_REL32 = 0xE8
NOP = 0x90


class HookError(Exception):
    pass


def _md(md=None):
    if capstone is None:
        raise HookError("capstone is required for hooking "
                        "(pip install capstone)")
    cs = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    cs.detail = True
    return cs


def decode_site(pe, va, minimum=5):
    """Whole instructions covering at least `minimum` bytes at `va`."""
    off = pe.va_to_off(va)
    if off is None:
        raise HookError(f"0x{va:x} is not in the image")
    cs = _md()
    out, total = [], 0
    for ins in cs.disasm(pe.data[off:off + 64], va):
        out.append(ins)
        total += ins.size
        if total >= minimum:
            break
    if total < minimum:
        raise HookError("could not decode enough instructions")
    return out, total


def _relocate(ins, new_addr):
    """Return this instruction's bytes, corrected for running at `new_addr`."""
    raw = bytearray(ins.bytes)

    # --- relative call/jmp (E8/E9 rel32) ---------------------------------
    if ins.size == 5 and raw[0] in (CALL_REL32, JMP_REL32):
        target = ins.address + 5 + struct.unpack_from("<i", raw, 1)[0]
        rel = target - (new_addr + 5)
        if not -2**31 <= rel < 2**31:
            raise HookError("relocated call/jmp out of rel32 range")
        struct.pack_into("<i", raw, 1, rel)
        return bytes(raw)

    # --- anything with a rip-relative memory operand ----------------------
    # This is the classic silent breakage: the displacement is relative to the
    # END of the instruction, so moving the instruction changes what it reads.
    if capstone is not None:
        for op in ins.operands:
            if (op.type == capstone.x86.X86_OP_MEM
                    and op.mem.base == capstone.x86.X86_REG_RIP):
                target = ins.address + ins.size + op.mem.disp
                rel = target - (new_addr + ins.size)
                if not -2**31 <= rel < 2**31:
                    raise HookError("rip-relative target out of range")
                # displacement sits immediately before any immediate
                disp_size = 4
                imm_size = 0
                for o in ins.operands:
                    if o.type == capstone.x86.X86_OP_IMM:
                        imm_size = ins.imm_size
                pos = ins.size - imm_size - disp_size
                struct.pack_into("<i", raw, pos, rel)
                return bytes(raw)

    # --- things we must not move -----------------------------------------
    m = ins.mnemonic
    if m.startswith("j") or m in ("loop", "loope", "loopne", "call"):
        # short forms have a +/-128 reach that a cave cannot satisfy; widening
        # them changes instruction sizes and is not worth the risk here
        raise HookError(f"cannot safely relocate '{m} {ins.op_str}' "
                        f"at 0x{ins.address:x} - pick another hook site")

    return bytes(raw)


def build(pe, site_va, cave_va, payload=b"", minimum=5):
    """Plan a hook. Returns {sites, listing, ...} ready for patching.Patch.

    payload runs first, then the displaced originals, then control returns.
    """
    ins_list, taken = decode_site(pe, site_va, minimum)
    return_va = site_va + taken

    # cave layout: payload | relocated originals | jmp back
    cave = bytearray(payload)
    for ins in ins_list:
        cave += _relocate(ins, cave_va + len(cave))
    back_from = cave_va + len(cave)
    rel = return_va - (back_from + 5)
    if not -2**31 <= rel < 2**31:
        raise HookError("return jump out of rel32 range")
    cave += bytes([JMP_REL32]) + struct.pack("<i", rel)

    # site: jmp to cave, then NOP out whatever is left of the displaced bytes
    rel_in = cave_va - (site_va + 5)
    if not -2**31 <= rel_in < 2**31:
        raise HookError("cave too far from the hook site for rel32")
    site_bytes = bytes([JMP_REL32]) + struct.pack("<i", rel_in)
    site_bytes += bytes([NOP]) * (taken - 5)

    off = pe.va_to_off(site_va)
    original = pe.data[off:off + taken]
    cave_off = pe.va_to_off(cave_va)
    if cave_off is None:
        raise HookError("cave address is not in the image")
    cave_now = pe.data[cave_off:cave_off + len(cave)]
    if any(b != 0xCC for b in cave_now):
        raise HookError(f"cave at 0x{cave_va:x} is not free "
                        f"({len(cave)} bytes needed)")

    return {
        "sites": [
            {"va": site_va, "expect": original.hex(), "write": site_bytes.hex()},
            {"va": cave_va, "expect": cave_now.hex(), "write": bytes(cave).hex()},
        ],
        "displaced": [f"{i.address:012x}  {i.mnemonic} {i.op_str}"
                      for i in ins_list],
        "bytes_taken": taken,
        "cave_used": len(cave),
        "return_to": f"0x{return_va:x}",
    }


def disasm(pe, va, count=8):
    """Small helper for choosing a hook site by eye."""
    off = pe.va_to_off(va)
    if off is None:
        return []
    cs = _md()
    return [f"{i.address:012x}  {i.bytes.hex():<20} {i.mnemonic} {i.op_str}"
            for i in cs.disasm(pe.data[off:off + count * 15], va, count)]
