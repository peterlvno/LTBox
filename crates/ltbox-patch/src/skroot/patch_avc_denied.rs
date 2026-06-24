//! SELinux `avc_denied` hook — port of upstream `patch_avc_denied.{h,cpp}`.
//!
//! A small stub is planted into a spare region. Every return site in
//! `avc_denied` branches to it; the stub calls the current-credential helper
//! and, when it reports an already-root current task, forces the return value to
//! zero before returning with the same PAC flavor as the original return site.
#![allow(dead_code)]

use super::asm::{Asm, SP, ZR};
use super::insn;
use super::patch_base::PatchBase;
use super::patch_bytes::PatchBytes;
use super::symbol_analyze::SymbolRegion;

const CANARY_BYPASS_SIZE: u32 = 0x50;

pub struct PatchAvcDenied {
    avc_denied: SymbolRegion,
}

impl PatchAvcDenied {
    pub fn new(avc_denied: SymbolRegion) -> Self {
        PatchAvcDenied { avc_denied }
    }

    pub fn patch_avc_denied(
        &self,
        base: &PatchBase,
        hook_region: &SymbolRegion,
        current_avc_check_bl_func: u64,
        out: &mut Vec<PatchBytes>,
    ) -> usize {
        let hook_addr = hook_region.offset;
        if hook_addr == 0 {
            return 0;
        }

        let ret_addrs = ret_offsets(base.buf(), self.avc_denied);
        let Some(&first_ret) = ret_addrs.first() else {
            return 0;
        };
        let ret_insn = rd32(base.buf(), first_ret as usize);

        let mut a = Asm::new();
        let end = a.new_label();

        a.sub_imm_x(SP, SP, CANARY_BYPASS_SIZE);
        base.emit_safe_bl(&mut a, hook_addr, current_avc_check_bl_func);
        a.add_imm_x(SP, SP, CANARY_BYPASS_SIZE);
        a.cbz_x(10, end);
        a.mov_reg_w(0, ZR);
        a.bind(end);
        base.emit_ret_by_entry_insn(&mut a, ret_insn);

        let bytes = match a.to_bytes() {
            Ok(b) if !b.is_empty() => b,
            _ => return 0,
        };
        let size = bytes.len();
        if size as u64 > hook_region.size {
            return 0;
        }

        out.push(PatchBytes {
            addr: hook_addr,
            bytes,
        });
        for addr in ret_addrs {
            base.patch_jump(addr, hook_addr, out);
        }
        size
    }
}

fn ret_offsets(buf: &[u8], region: SymbolRegion) -> Vec<u64> {
    if !region.valid() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut off = region.offset as usize;
    let end = (region.offset + region.size).min(buf.len() as u64) as usize;
    while off + 4 <= end {
        let w = rd32(buf, off);
        if insn::is_ret(w) || insn::is_retaa(w) || insn::is_retab(w) {
            out.push(off as u64);
        }
        off += 4;
    }
    out
}

fn rd32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skroot::patch_base::PatchBase;

    const MRS_SP_EL0: u32 = 0xD538_4100;
    const RET: u32 = 0xD65F_03C0;
    const RETAA: u32 = 0xD65F_0BFF;

    fn region(off: u64, size: u64) -> SymbolRegion {
        SymbolRegion { offset: off, size }
    }

    fn fake_kernel() -> Vec<u8> {
        let mut buf = vec![0u8; 0x9000];
        for i in 0..5001 {
            let off = i * 4;
            buf[off..off + 4].copy_from_slice(&MRS_SP_EL0.to_le_bytes());
        }
        let pat = test_init_cred_pattern();
        let anchor = 0x8800;
        buf[anchor - pat.len()..anchor].copy_from_slice(&pat);
        buf[0x3000..0x3004].copy_from_slice(&RET.to_le_bytes());
        buf[0x3010..0x3014].copy_from_slice(&RETAA.to_le_bytes());
        buf
    }

    fn test_init_cred_pattern() -> Vec<u8> {
        let max: u64 = 0x1FF_FFFF_FFFF;
        let mut p = Vec::new();
        p.extend_from_slice(&4u64.to_le_bytes());
        p.extend_from_slice(&[0u8; 32]);
        p.extend_from_slice(&0u64.to_le_bytes());
        p.extend_from_slice(&0u64.to_le_bytes());
        for _ in 0..3 {
            p.extend_from_slice(&max.to_le_bytes());
        }
        p
    }

    #[test]
    fn hooks_every_avc_denied_return() {
        let buf = fake_kernel();
        let base = PatchBase::new(&buf, 8, None).expect("init_cred found");
        let mut out = Vec::new();

        let n = PatchAvcDenied::new(region(0x3000, 0x20)).patch_avc_denied(
            &base,
            &region(0x5000, 0x100),
            0x4800,
            &mut out,
        );

        assert!(n > 0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].addr, 0x5000);
        assert_eq!(out[1].addr, 0x3000);
        assert_eq!(out[2].addr, 0x3010);
        assert!(insn::is_b(rd32(&out[1].bytes, 0)));
        assert!(insn::is_b(rd32(&out[2].bytes, 0)));

        let words: Vec<u32> = out[0]
            .bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(words[0], 0xD101_43FF); // sub sp, sp, #0x50
        assert!(words.iter().any(|&w| insn::is_bl(w)));
        assert!(words.iter().any(|&w| insn::is_cbz(w)));
        assert!(insn::is_ret(*words.last().unwrap()));
    }

    #[test]
    fn missing_ret_fails_without_writes() {
        let buf = fake_kernel();
        let base = PatchBase::new(&buf, 8, None).unwrap();
        let mut out = Vec::new();

        let n = PatchAvcDenied::new(region(0x3040, 0x20)).patch_avc_denied(
            &base,
            &region(0x5000, 0x100),
            0x4800,
            &mut out,
        );

        assert_eq!(n, 0);
        assert!(out.is_empty());
    }
}
