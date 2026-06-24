//! Patch-record model and the simple "neutralize a function" patches.
//!
//! A [`PatchBytes`] is one write: a byte string at a file offset. Port of
//! upstream's `patch_bytes_data` plus the `patch_ret*` / `patch_data` helpers
//! (`patch_kernel_root.h`) used for the CFI / Huawei bypass.
//!
//! When the target function starts with a PAC sign or `bti`, the replacement is
//! prefixed with a `bti jc` landing pad so the stub stays a valid indirect-call
//! target; otherwise it is emitted bare.
#![allow(dead_code)]

use super::insn;

/// One patch write: `bytes` to be placed at file offset `addr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchBytes {
    pub addr: u64,
    pub bytes: Vec<u8>,
}

/// `bti jc` (hint #38) — landing pad kept ahead of a neutralized PAC/BTI entry.
const BTI_JC: [u8; 4] = [0xDF, 0x24, 0x03, 0xD5];
/// `ret`.
const RET: [u8; 4] = [0xC0, 0x03, 0x5F, 0xD6];
/// `mov x0, #1`.
const MOVZ_X0_1: [u8; 4] = [0x20, 0x00, 0x80, 0xD2];
/// `mov w0, wzr`.
const MOV_W0_WZR: [u8; 4] = [0xE0, 0x03, 0x1F, 0x2A];

/// True when the word at `addr` is a PAC sign or `bti` (so a landing pad must
/// be preserved). Matches upstream's paciaz/paciasp/pacibz/pacibsp/bti set.
fn entry_needs_landing_pad(buf: &[u8], addr: u64) -> bool {
    let i = addr as usize;
    if i + 4 > buf.len() {
        return false;
    }
    insn::is_pac_or_bti(u32::from_le_bytes([
        buf[i],
        buf[i + 1],
        buf[i + 2],
        buf[i + 3],
    ]))
}

/// Build a replacement = optional `bti jc` + `body`, pushed as a write at `addr`.
fn emit(buf: &[u8], addr: u64, body: &[u8], out: &mut Vec<PatchBytes>) -> usize {
    if addr == 0 {
        return 0;
    }
    let mut bytes = Vec::with_capacity(8 + body.len());
    if entry_needs_landing_pad(buf, addr) {
        bytes.extend_from_slice(&BTI_JC);
    }
    bytes.extend_from_slice(body);
    let len = bytes.len();
    out.push(PatchBytes { addr, bytes });
    len
}

/// Replace the function at `start` with an immediate `ret` (upstream
/// `patch_ret_cmd`). Returns the number of bytes written (0 if `start == 0`).
pub fn patch_ret(buf: &[u8], start: u64, out: &mut Vec<PatchBytes>) -> usize {
    emit(buf, start, &RET, out)
}

/// Replace the function at `start` with `mov x0, #1; ret` (upstream
/// `patch_ret_1_cmd`).
pub fn patch_ret_1(buf: &[u8], start: u64, out: &mut Vec<PatchBytes>) -> usize {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&MOVZ_X0_1);
    body.extend_from_slice(&RET);
    emit(buf, start, &body, out)
}

/// Replace the function at `start` with `mov w0, wzr; ret` (upstream
/// `patch_ret_0_cmd`).
pub fn patch_ret_0(buf: &[u8], start: u64, out: &mut Vec<PatchBytes>) -> usize {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&MOV_W0_WZR);
    body.extend_from_slice(&RET);
    emit(buf, start, &body, out)
}

/// Write arbitrary bytes at `start` (upstream `patch_data`). No landing pad.
pub fn patch_data(start: u64, data: &[u8], out: &mut Vec<PatchBytes>) -> usize {
    if start == 0 {
        return 0;
    }
    out.push(PatchBytes {
        addr: start,
        bytes: data.to_vec(),
    });
    data.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACIASP: u32 = 0xD503_233F;
    const NOP: u32 = 0xD503_201F;

    fn img(first: u32) -> Vec<u8> {
        first.to_le_bytes().to_vec()
    }

    #[test]
    fn ret_on_plain_entry() {
        let buf = img(NOP);
        let mut out = Vec::new();
        assert_eq!(patch_ret(&buf, 0, &mut out), 0); // addr 0 → no-op
        let mut out = Vec::new();
        // a non-zero address into a buffer whose word is plain.
        let buf = [NOP.to_le_bytes(), NOP.to_le_bytes()].concat();
        assert_eq!(patch_ret(&buf, 4, &mut out), 4);
        assert_eq!(out[0].bytes, RET);
        assert_eq!(out[0].addr, 4);
    }

    #[test]
    fn ret_on_pac_entry_keeps_landing_pad() {
        let buf = [PACIASP.to_le_bytes(), NOP.to_le_bytes()].concat();
        let mut out = Vec::new();
        assert_eq!(patch_ret(&buf, 0, &mut out), 0); // start 0 is treated as none
        let mut out = Vec::new();
        // place a PAC word at offset 4.
        let buf = [NOP.to_le_bytes(), PACIASP.to_le_bytes()].concat();
        let n = patch_ret(&buf, 4, &mut out);
        assert_eq!(n, 8);
        assert_eq!(&out[0].bytes[..4], &BTI_JC);
        assert_eq!(&out[0].bytes[4..], &RET);
    }

    #[test]
    fn ret_1_and_ret_0_bodies() {
        let buf = [NOP.to_le_bytes(), NOP.to_le_bytes()].concat();
        let mut out = Vec::new();
        assert_eq!(patch_ret_1(&buf, 4, &mut out), 8);
        assert_eq!(&out[0].bytes[..4], &MOVZ_X0_1);
        assert_eq!(&out[0].bytes[4..], &RET);

        let mut out = Vec::new();
        assert_eq!(patch_ret_0(&buf, 4, &mut out), 8);
        assert_eq!(&out[0].bytes[..4], &MOV_W0_WZR);
        assert_eq!(&out[0].bytes[4..], &RET);
    }

    #[test]
    fn data_write_is_verbatim() {
        let mut out = Vec::new();
        assert_eq!(patch_data(0, b"abc", &mut out), 0);
        assert_eq!(patch_data(0x100, b"abc", &mut out), 3);
        assert_eq!(
            out[0],
            PatchBytes {
                addr: 0x100,
                bytes: b"abc".to_vec()
            }
        );
    }
}
