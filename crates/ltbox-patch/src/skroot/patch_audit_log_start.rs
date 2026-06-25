//! `audit_log_start` hook — port of upstream `patch_audit_log_start.{h,cpp}`.
//!
//! When the current task already has SKRoot-granted credentials, this hook
//! suppresses audit logging by returning zero from `audit_log_start`; otherwise
//! it executes the original skipped instruction and returns to the function.
#![allow(dead_code)]

use super::asm::{Asm, SP, ZR};
use super::patch_base::PatchBase;
use super::patch_bytes::PatchBytes;
use super::symbol_analyze::SymbolRegion;

const CANARY_BYPASS_SIZE: u32 = 0x50;

pub struct PatchAuditLogStart {
    audit_log_start: u64,
    audit_log_start_orig_entry_insn: u32,
}

impl PatchAuditLogStart {
    pub fn new(base: &PatchBase<'_>, audit_log_start: u64) -> Option<Self> {
        let i = usize::try_from(audit_log_start).ok()?;
        if audit_log_start == 0 || i + 4 > base.buf().len() {
            return None;
        }
        let audit_log_start_orig_entry_insn = rd32(base.buf(), i);
        let audit_log_start = base.skip_pac_bti_at_func_start(audit_log_start);
        Some(PatchAuditLogStart {
            audit_log_start,
            audit_log_start_orig_entry_insn,
        })
    }

    pub fn patch_audit_log_start(
        &self,
        base: &PatchBase<'_>,
        hook_region: &SymbolRegion,
        current_avc_check_bl_func: u64,
        out: &mut Vec<PatchBytes>,
    ) -> usize {
        let hook_addr = hook_region.offset;
        if hook_addr == 0 {
            return 0;
        }
        let jump_back = self.audit_log_start + 4;

        let mut a = Asm::new();
        let end = a.new_label();

        a.sub_imm_x(SP, SP, CANARY_BYPASS_SIZE);
        base.emit_safe_bl(&mut a, hook_addr, current_avc_check_bl_func);
        a.add_imm_x(SP, SP, CANARY_BYPASS_SIZE);
        a.cbz_x(10, end);
        a.mov_reg_w(0, ZR);
        base.emit_ret_by_entry_insn(&mut a, self.audit_log_start_orig_entry_insn);
        a.bind(end);
        let orig_slot = a.offset();
        a.mov_reg_x(0, 0);
        let branch_pos = a.offset() as i64;
        a.b_off((jump_back as i64 - (hook_addr as i64 + branch_pos)) as i32);

        let mut bytes = match a.to_bytes() {
            Ok(b) if !b.is_empty() => b,
            _ => return 0,
        };

        let Ok(i) = usize::try_from(self.audit_log_start) else {
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
        base.patch_jump(self.audit_log_start, hook_addr, out);
        size
    }
}

fn rd32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
