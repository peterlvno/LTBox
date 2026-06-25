//! `filldir64` hook — port of upstream `patch_filldir64.{h,cpp}`.
//!
//! The first small stub loads the embedded root-key prefix into `x11`; the
//! second stub hides directory entries whose name equals that prefix. Upstream
//! uses two adjacent code caves on pre-6.1 kernels and chains `die` into
//! `__drm_puts_coredump` on 6.1+ kernels.
#![allow(dead_code)]

use super::asm::{Asm, Cond, ZR};
use super::patch_base::PatchBase;
use super::patch_bytes::PatchBytes;
use super::symbol_analyze::SymbolRegion;

const FOLDER_HEAD_ROOT_KEY_LEN: u32 = 16;

pub struct PatchFilldir64 {
    filldir64: u64,
    filldir64_orig_entry_insn: u32,
    pre_6_1: bool,
}

impl PatchFilldir64 {
    pub fn new(base: &PatchBase<'_>, filldir64: u64, pre_6_1: bool) -> Option<Self> {
        let i = usize::try_from(filldir64).ok()?;
        if filldir64 == 0 || i + 4 > base.buf().len() {
            return None;
        }
        let filldir64_orig_entry_insn = rd32(base.buf(), i);
        let filldir64 = base.skip_pac_bti_at_func_start(filldir64);
        Some(PatchFilldir64 {
            filldir64,
            filldir64_orig_entry_insn,
            pre_6_1,
        })
    }

    pub fn patch_filldir64_root_key_guide(
        &self,
        base: &PatchBase<'_>,
        root_key_mem_addr: u64,
        hook_region: &SymbolRegion,
        out: &mut Vec<PatchBytes>,
    ) -> usize {
        let hook_addr = hook_region.offset;
        if hook_addr == 0 {
            return 0;
        }

        let Some(root_key_adr_offset) = root_key_mem_addr
            .checked_sub(hook_addr)
            .map(|v| v as i64)
            .or_else(|| {
                hook_addr
                    .checked_sub(root_key_mem_addr)
                    .map(|v| -(v as i64))
            })
        else {
            return 0;
        };
        let Ok(root_key_adr_offset) = i32::try_from(root_key_adr_offset) else {
            return 0;
        };

        let mut a = Asm::new();
        a.adr(11, root_key_adr_offset);

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
        base.patch_jump(self.filldir64, hook_addr, out);
        size
    }

    pub fn patch_filldir64_core(
        &self,
        base: &PatchBase<'_>,
        hook_region: &SymbolRegion,
        out: &mut Vec<PatchBytes>,
    ) -> usize {
        let hook_addr = hook_region.offset;
        if hook_addr == 0 {
            return 0;
        }
        let jump_back = self.filldir64 + 4;

        let mut a = Asm::new();
        let end = a.new_label();
        let cycle_name = a.new_label();

        a.cmp_imm_w(2, FOLDER_HEAD_ROOT_KEY_LEN);
        a.b_cond(Cond::Ne, end);
        a.mov_imm_x_asmjit(12, 0);
        a.bind(cycle_name);
        a.ldrb_w_reg(13, 1, 12);
        a.ldrb_w_reg(14, 11, 12);
        a.cmp_reg_w(13, 14);
        a.b_cond(Cond::Ne, end);
        a.add_imm_x(12, 12, 1);
        a.cmp_imm_x(12, FOLDER_HEAD_ROOT_KEY_LEN);
        a.b_cond(Cond::Lt, cycle_name);
        if self.pre_6_1 {
            a.mov_reg_x(0, ZR);
        } else {
            a.mov_imm_x_asmjit(0, 1);
        }
        base.emit_ret_by_entry_insn(&mut a, self.filldir64_orig_entry_insn);
        a.bind(end);
        let orig_slot = a.offset();
        a.mov_reg_x(0, 0);
        let branch_pos = a.offset() as i64;
        a.b_off((jump_back as i64 - (hook_addr as i64 + branch_pos)) as i32);

        let mut bytes = match a.to_bytes() {
            Ok(b) if !b.is_empty() => b,
            _ => return 0,
        };

        let Ok(i) = usize::try_from(self.filldir64) else {
            return 0;
        };
        if i + 4 > base.buf().len() || orig_slot + 4 > bytes.len() {
            return 0;
        }
        bytes[orig_slot..orig_slot + 4].copy_from_slice(&base.buf()[i..i + 4]);

        let size = bytes.len();
        if size as u64 > hook_region.size {
            return 0;
        }

        out.push(PatchBytes {
            addr: hook_addr,
            bytes,
        });
        size
    }
}

fn rd32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
