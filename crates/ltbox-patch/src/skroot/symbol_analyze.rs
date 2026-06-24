//! Resolve the specific kernel symbols and code regions the patcher needs.
//!
//! Port of upstream `analyze/symbol_analyze.{h,cpp}`. It sits on top of the
//! [`kallsyms`] decoder: for each function the SKRoot patches touch it resolves
//! a file offset (trying a list of name spellings, with a fuzzy substring
//! fallback), and for the ones whose size matters it computes a
//! [`SymbolRegion`] via the [`func_end`] boundary finder bounded by the kallsyms
//! size.
#![allow(dead_code)]

use super::func_end;
use super::insn;
use super::kallsyms::Symbols;

/// A resolved function: its file `offset` and a best-effort `size` (0 when the
/// size could not be bounded). `offset == 0` means "not found".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SymbolRegion {
    pub offset: u64,
    pub size: u64,
}

impl SymbolRegion {
    pub fn valid(&self) -> bool {
        self.offset != 0
    }
    fn consume(&mut self, n: u64) {
        self.offset += n;
        self.size = self.size.saturating_sub(n);
    }
}

/// Every symbol/region the Lite patches reference. Fields left at their default
/// (`0` / invalid region) were not present in the target kernel.
#[derive(Debug, Clone, Default)]
pub struct KernelSymbolOffset {
    pub text: u64,
    pub stext: u64,
    pub die: SymbolRegion,
    pub drm_puts_coredump: SymbolRegion,
    pub drm_printfn_coredump: SymbolRegion,

    pub do_execve_file: SymbolRegion,
    pub do_execveat_common: SymbolRegion,
    pub do_execve_common: SymbolRegion,
    pub do_execveat: SymbolRegion,
    pub do_execve: SymbolRegion,

    pub avc_denied: SymbolRegion,
    pub audit_log_start: u64,
    pub filldir64: u64,

    pub sys_getuid: SymbolRegion,
    pub prctl_get_seccomp: SymbolRegion,

    pub cfi_check: SymbolRegion,
    pub cfi_check_fail: u64,
    pub cfi_slowpath_diag: u64,
    pub cfi_slowpath: u64,
    pub ubsan_handle_cfi_check_fail_abort: u64,
    pub ubsan_handle_cfi_check_fail: u64,
    pub report_cfi_failure: u64,

    pub hkip_check_uid_root: u64,
    pub hkip_check_gid_root: u64,
    pub hkip_check_xid_root: u64,
    pub kti_randomize_init: SymbolRegion,
}

impl KernelSymbolOffset {
    /// Upstream `find_symbol_offset` success predicate: enough was found to
    /// build the `do_execve` hook and the SELinux/audit/seccomp patches.
    ///
    /// The execve term lists only the three primary hook targets, matching
    /// upstream. `do_execve_common` is resolved (the patch layer may reference
    /// it) but is an inner helper, not a hookable entry, so it is intentionally
    /// not part of this gate.
    pub fn is_complete(&self) -> bool {
        (self.do_execve.valid() || self.do_execveat.valid() || self.do_execveat_common.valid())
            && self.avc_denied.valid()
            && self.audit_log_start != 0
            && self.filldir64 != 0
            && self.sys_getuid.valid()
            && self.prctl_get_seccomp.valid()
    }
}

/// A name to try, and whether to match it as a fuzzy substring.
type Candidate<'a> = (&'a str, bool);

/// Resolves [`KernelSymbolOffset`] from a decoded symbol table over a kernel
/// image.
pub struct SymbolAnalyze<'a> {
    buf: &'a [u8],
    syms: &'a Symbols,
}

impl<'a> SymbolAnalyze<'a> {
    pub fn new(buf: &'a [u8], syms: &'a Symbols) -> Self {
        SymbolAnalyze { buf, syms }
    }

    /// Resolve every field (upstream `find_symbol_offset`). The result is always
    /// returned; check [`KernelSymbolOffset::is_complete`] for usability.
    pub fn analyze(&self) -> KernelSymbolOffset {
        KernelSymbolOffset {
            text: self.find_addr(&[("_text", false)]),
            stext: self.find_addr(&[("_stext", false)]),
            die: self.find_region(&[("die", false)]),
            drm_puts_coredump: self.find_region(&[("__drm_puts_coredump", false)]),
            drm_printfn_coredump: self.find_region(&[("__drm_printfn_coredump", false)]),

            do_execve_file: self.find_region(&[("__do_execve_file", false)]),
            do_execveat_common: self
                .find_region(&[("do_execveat_common", false), ("do_execveat_common", true)]),
            do_execve_common: self
                .find_region(&[("do_execve_common", false), ("do_execve_common", true)]),
            do_execveat: self.find_region(&[("do_execveat", false)]),
            do_execve: self.find_region(&[("do_execve", false)]),

            avc_denied: self.find_region(&[("avc_denied", false), ("avc_denied", true)]),
            audit_log_start: self.find_addr(&[("audit_log_start", false)]),
            filldir64: self.find_addr(&[("filldir64", false), ("filldir64", true)]),

            sys_getuid: self.find_region(&[
                ("sys_getuid", false),
                ("__arm64_sys_getuid", false),
                ("sys_getuid", true),
            ]),
            prctl_get_seccomp: self.find_region(&[("prctl_get_seccomp", false)]),

            cfi_check: self.find_region(&[("__cfi_check", false)]),
            cfi_check_fail: self.find_addr(&[("__cfi_check_fail", false)]),
            cfi_slowpath_diag: self.find_addr(&[("__cfi_slowpath_diag", false)]),
            cfi_slowpath: self.find_addr(&[("__cfi_slowpath", false)]),
            ubsan_handle_cfi_check_fail_abort: self
                .find_addr(&[("__ubsan_handle_cfi_check_fail_abort", false)]),
            ubsan_handle_cfi_check_fail: self
                .find_addr(&[("__ubsan_handle_cfi_check_fail", false)]),
            report_cfi_failure: self.find_addr(&[("report_cfi_failure", false)]),

            hkip_check_uid_root: self.find_addr(&[("hkip_check_uid_root", false)]),
            hkip_check_gid_root: self.find_addr(&[("hkip_check_gid_root", false)]),
            hkip_check_xid_root: self.find_addr(&[("hkip_check_xid_root", false)]),
            kti_randomize_init: self.find_region(&[("kti_randomize_init", false)]),
        }
    }

    /// First candidate that resolves to a non-zero offset.
    fn find_addr(&self, names: &[Candidate]) -> u64 {
        for &(name, fuzzy) in names {
            let addr = self.lookup_single(name, fuzzy);
            if addr != 0 {
                return addr;
            }
        }
        0
    }

    /// First candidate that resolves to a valid region.
    fn find_region(&self, names: &[Candidate]) -> SymbolRegion {
        for &(name, fuzzy) in names {
            let addr = self.lookup_single(name, fuzzy);
            if addr == 0 {
                continue;
            }
            let region = self.parse_region(addr);
            if region.valid() {
                return region;
            }
        }
        SymbolRegion::default()
    }

    /// Resolve a single name to a file offset, applying the `b`-thunk follow.
    /// `fuzzy` matches the name as a substring and takes the first hit.
    fn lookup_single(&self, name: &str, fuzzy: bool) -> u64 {
        let raw = if fuzzy {
            self.syms.names_like(name).first().map(|&(_, a)| a)
        } else {
            self.syms.lookup(name)
        };
        match raw {
            Some(off) => self.check_convert_b_insn(off),
            None => 0,
        }
    }

    /// If the symbol entry is a lone `b target` trampoline, follow it to the real
    /// function (upstream `check_convert_b_insn`).
    fn check_convert_b_insn(&self, off: u64) -> u64 {
        let idx = off as usize;
        if off == 0 || idx + 4 > self.buf.len() {
            return off;
        }
        let word = u32::from_le_bytes([
            self.buf[idx],
            self.buf[idx + 1],
            self.buf[idx + 2],
            self.buf[idx + 3],
        ]);
        if !insn::is_b(word) {
            return off;
        }
        let disp = insn::branch_b_displacement(word) as i64;
        (off as i64).wrapping_add(disp) as u64
    }

    /// Bound a function's region: `func_end` size, clamped by the kallsyms size
    /// when known (upstream `parse_symbol_region`).
    fn parse_region(&self, off: u64) -> SymbolRegion {
        let mut region = SymbolRegion {
            offset: off,
            size: 0,
        };
        if !region.valid() {
            return region;
        }
        let Some(candidate) = func_end::find_end_func_offset(self.buf, off as usize) else {
            return region;
        };
        let candidate_size = candidate as u64 + 4;
        region.size = match self.syms.size_at(off) {
            Some(ks) if ks != 0 => candidate_size.min(ks),
            _ => candidate_size,
        };
        region
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_validity_and_consume() {
        let mut r = SymbolRegion {
            offset: 0x1000,
            size: 0x40,
        };
        assert!(r.valid());
        r.consume(0x10);
        assert_eq!(r.offset, 0x1010);
        assert_eq!(r.size, 0x30);
        // size saturates rather than underflowing.
        r.consume(0x100);
        assert_eq!(r.size, 0);
        assert!(!SymbolRegion::default().valid());
    }

    #[test]
    fn completeness_predicate() {
        let mut o = KernelSymbolOffset::default();
        assert!(!o.is_complete());
        o.do_execveat_common = SymbolRegion {
            offset: 0x10,
            size: 0x10,
        };
        o.avc_denied = SymbolRegion {
            offset: 0x20,
            size: 0x10,
        };
        o.audit_log_start = 0x30;
        o.filldir64 = 0x40;
        o.sys_getuid = SymbolRegion {
            offset: 0x50,
            size: 0x10,
        };
        assert!(!o.is_complete()); // prctl_get_seccomp still missing
        o.prctl_get_seccomp = SymbolRegion {
            offset: 0x60,
            size: 0x10,
        };
        assert!(o.is_complete());
    }
}
