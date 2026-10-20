//! Hashes Studio uses while compiling a bank.

use super::prj::Guid;

/// Bob Jenkins lookup3 `hashlittle2` (pc, pb in/out), the hash FMOD Studio uses here.
pub fn hashlittle2(k: &[u8], pc: u32, pb: u32) -> (u32, u32) {
    let mut length = k.len();
    let mut a = 0xdeadbeefu32.wrapping_add(length as u32).wrapping_add(pc);
    let mut b = a;
    let mut c = a.wrapping_add(pb);
    let rd = |i: usize| u32::from_le_bytes([k[i], k[i + 1], k[i + 2], k[i + 3]]);
    let mut i = 0;
    while length > 12 {
        a = a.wrapping_add(rd(i));
        b = b.wrapping_add(rd(i + 4));
        c = c.wrapping_add(rd(i + 8));
        a = a.wrapping_sub(c); a ^= c.rotate_left(4); c = c.wrapping_add(b);
        b = b.wrapping_sub(a); b ^= a.rotate_left(6); a = a.wrapping_add(c);
        c = c.wrapping_sub(b); c ^= b.rotate_left(8); b = b.wrapping_add(a);
        a = a.wrapping_sub(c); a ^= c.rotate_left(16); c = c.wrapping_add(b);
        b = b.wrapping_sub(a); b ^= a.rotate_left(19); a = a.wrapping_add(c);
        c = c.wrapping_sub(b); c ^= b.rotate_left(4); b = b.wrapping_add(a);
        length -= 12;
        i += 12;
    }
    if length == 0 {
        return (c, b);
    }
    let mut t = [0u8; 12];
    t[..length].copy_from_slice(&k[i..i + length]);
    let tw = |j: usize| u32::from_le_bytes([t[j], t[j + 1], t[j + 2], t[j + 3]]);
    a = a.wrapping_add(tw(0));
    if length > 4 {
        b = b.wrapping_add(tw(4));
    }
    if length > 8 {
        c = c.wrapping_add(tw(8));
    }
    c ^= b; c = c.wrapping_sub(b.rotate_left(14));
    a ^= c; a = a.wrapping_sub(c.rotate_left(11));
    b ^= a; b = b.wrapping_sub(a.rotate_left(25));
    c ^= b; c = c.wrapping_sub(b.rotate_left(16));
    a ^= c; a = a.wrapping_sub(c.rotate_left(4));
    b ^= a; b = b.wrapping_sub(a.rotate_left(14));
    c ^= b; c = c.wrapping_sub(b.rotate_left(24));
    (c, b)
}

/// `calculatePublicID`: the FMOD_STUDIO_PARAMETER_ID of a parameter GUID.
pub fn public_id(g: &Guid) -> (u32, u32) {
    let (mut c, mut b) = (0, 0);
    for (o, n) in [(0usize, 4usize), (4, 2), (6, 2), (8, 8)] {
        let r = hashlittle2(&g.0[o..o + n], c, b);
        c = r.0;
        b = r.1;
    }
    (c, b)
}

#[cfg(test)]
mod t {
    use super::*;
    #[test]
    fn pid() {
        let g = Guid::parse("{4f00907e-d80c-4e98-829c-989ba26fe988}").unwrap();
        assert_eq!(public_id(&g), (0x573c97e5, 0x8bce5e3b));
    }
}
