//! Pre-GUI SKRoot patch orchestration.
//!
//! This module resolves the kernel facts that the ported SKRoot Lite patches
//! need and emits one byte-write plan for the currently ported core: the
//! `do_execve` root hook plus the SELinux `avc_denied` allow hook. It
//! intentionally stops before boot-image/root-pipeline or GUI wiring.
#![allow(dead_code)]

use super::kallsyms::{self, KallsymsError};
use super::offsets;
use super::patch_avc_denied::PatchAvcDenied;
use super::patch_base::PatchBase;
use super::patch_bytes::{self, PatchBytes};
use super::patch_current_avc_check::PatchCurrentAvcCheck;
use super::patch_do_execve::PatchDoExecve;
use super::symbol_analyze::{KernelSymbolOffset, SymbolAnalyze, SymbolRegion};
use super::version::KernelVersion;

/// Offsets recovered from live kernel code and `init_cred`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkrootKernelOffsets {
    pub cred_offset: u64,
    pub cred_uid_offset: u64,
    pub seccomp_offset: u64,
    pub huawei_kti_addr: Option<u64>,
    pub init_cred_offset: u64,
    pub cap_ability_max: u64,
}

/// Which code-cave strategy was selected for the emitted core hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpareRegionStrategy {
    /// Upstream's pre-6.1 fixed image slot at `0x200`.
    EarlyImageSlot,
    /// A consumed `__cfi_check` region was larger than the fixed slot.
    CfiCheck,
    /// 6.1+ path: `die` for the root key/execve hook and
    /// `__drm_printfn_coredump` for the SELinux helper/hook.
    DieAndDrmPrintfCoredump,
}

/// A complete pre-GUI patch plan for the ported SKRoot core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkrootCorePatchPlan {
    pub version: String,
    pub version_triple: (u32, u32, u32),
    pub offsets: SkrootKernelOffsets,
    pub strategy: SpareRegionStrategy,
    /// Address of the 48-byte root-key placeholder in the `do_execve` stub.
    pub root_key_addr: u64,
    pub writes: Vec<PatchBytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkrootPatchPlanError {
    #[error("kernel version banner not found")]
    NoKernelVersion,
    #[error("kallsyms decode failed: {0:?}")]
    Kallsyms(KallsymsError),
    #[error("missing required symbol: {0}")]
    MissingSymbol(&'static str),
    #[error("failed to resolve {0}")]
    Resolve(&'static str),
    #[error("failed to locate init_cred")]
    InitCred,
    #[error("no spare region available for kernel {0}")]
    NoSpareRegion(String),
    #[error("patch emission failed: {0}")]
    PatchFailed(&'static str),
}

/// Build the byte-write plan for the ported pre-GUI SKRoot core.
pub fn build_core_patch_plan(kernel: &[u8]) -> Result<SkrootCorePatchPlan, SkrootPatchPlanError> {
    let version =
        KernelVersion::from_kernel(kernel).ok_or(SkrootPatchPlanError::NoKernelVersion)?;
    let syms = kallsyms::analyze(kernel).map_err(SkrootPatchPlanError::Kallsyms)?;
    let mut symbols = SymbolAnalyze::new(kernel, &syms).analyze();
    let mut writes = Vec::new();

    let mut offsets = resolve_offsets(kernel, &version, &symbols)?;
    let mut base = PatchBase::new(kernel, offsets.cred_uid_offset, offsets.huawei_kti_addr)
        .ok_or(SkrootPatchPlanError::InitCred)?;
    offsets.init_cred_offset = base.init_cred().offset;
    offsets.cap_ability_max = base.init_cred().cap_ability_max;

    patch_existing_bypasses(kernel, &mut symbols, &mut writes);
    let (root_key_addr, strategy) =
        patch_core_hooks(kernel, &version, &mut base, &symbols, &offsets, &mut writes)?;

    Ok(SkrootCorePatchPlan {
        version: version.raw().to_string(),
        version_triple: version.triple(),
        offsets,
        strategy,
        root_key_addr,
        writes,
    })
}

fn resolve_offsets(
    kernel: &[u8],
    version: &KernelVersion,
    symbols: &KernelSymbolOffset,
) -> Result<SkrootKernelOffsets, SkrootPatchPlanError> {
    if !(symbols.do_execve.valid()
        || symbols.do_execveat.valid()
        || symbols.do_execveat_common.valid())
    {
        return Err(SkrootPatchPlanError::MissingSymbol("do_execve*"));
    }
    if !symbols.avc_denied.valid() {
        return Err(SkrootPatchPlanError::MissingSymbol("avc_denied"));
    }
    if !symbols.sys_getuid.valid() {
        return Err(SkrootPatchPlanError::MissingSymbol("sys_getuid"));
    }
    if !symbols.prctl_get_seccomp.valid() {
        return Err(SkrootPatchPlanError::MissingSymbol("prctl_get_seccomp"));
    }

    let cred_offset =
        offsets::find_cred_offset(kernel, symbols.sys_getuid.offset, symbols.sys_getuid.size)
            .ok_or(SkrootPatchPlanError::Resolve("cred offset"))?;
    let seccomp_offset = offsets::find_seccomp_offset(
        kernel,
        symbols.prctl_get_seccomp.offset,
        symbols.prctl_get_seccomp.size,
    )
    .ok_or(SkrootPatchPlanError::Resolve("seccomp offset"))?;
    let cred_uid_offset = offsets::find_cred_uid_offset(
        kernel,
        symbols.sys_getuid.offset,
        symbols.sys_getuid.size,
        cred_offset,
        offsets::cred_uid_min_off(version.triple()),
    )
    .ok_or(SkrootPatchPlanError::Resolve("cred uid offset"))?;

    let huawei_kti_addr = if symbols.kti_randomize_init.valid() {
        offsets::find_huawei_kti_addr(
            kernel,
            symbols.kti_randomize_init.offset,
            symbols.kti_randomize_init.size,
        )
    } else {
        None
    };

    Ok(SkrootKernelOffsets {
        cred_offset,
        cred_uid_offset,
        seccomp_offset,
        huawei_kti_addr,
        init_cred_offset: 0,
        cap_ability_max: 0,
    })
}

fn patch_existing_bypasses(
    kernel: &[u8],
    symbols: &mut KernelSymbolOffset,
    writes: &mut Vec<PatchBytes>,
) {
    if symbols.cfi_check.valid() {
        let n = patch_bytes::patch_ret(kernel, symbols.cfi_check.offset, writes);
        if n != 0 {
            symbols.cfi_check.consume(n as u64);
        }
    }
    patch_bytes::patch_ret(kernel, symbols.cfi_check_fail, writes);
    patch_bytes::patch_ret(kernel, symbols.cfi_slowpath_diag, writes);
    patch_bytes::patch_ret(kernel, symbols.cfi_slowpath, writes);
    patch_bytes::patch_ret(kernel, symbols.ubsan_handle_cfi_check_fail_abort, writes);
    patch_bytes::patch_ret(kernel, symbols.ubsan_handle_cfi_check_fail, writes);
    patch_bytes::patch_ret_1(kernel, symbols.report_cfi_failure, writes);

    patch_bytes::patch_ret_0(kernel, symbols.hkip_check_uid_root, writes);
    patch_bytes::patch_ret_0(kernel, symbols.hkip_check_gid_root, writes);
    patch_bytes::patch_ret_0(kernel, symbols.hkip_check_xid_root, writes);
}

fn patch_core_hooks(
    kernel: &[u8],
    version: &KernelVersion,
    base: &mut PatchBase<'_>,
    symbols: &KernelSymbolOffset,
    offsets: &SkrootKernelOffsets,
    writes: &mut Vec<PatchBytes>,
) -> Result<(u64, SpareRegionStrategy), SkrootPatchPlanError> {
    let execve = PatchDoExecve::new(base, symbols, version.triple())
        .ok_or(SkrootPatchPlanError::MissingSymbol("do_execve*"))?;

    if version.is_less_than((6, 1, 0)) {
        return patch_pre_6_1(base, symbols, offsets, writes, execve);
    }

    patch_6_1_plus(kernel, base, symbols, offsets, writes, execve)
}

fn patch_pre_6_1(
    base: &mut PatchBase<'_>,
    symbols: &KernelSymbolOffset,
    offsets: &SkrootKernelOffsets,
    writes: &mut Vec<PatchBytes>,
    execve: PatchDoExecve,
) -> Result<(u64, SpareRegionStrategy), SkrootPatchPlanError> {
    let mut region = SymbolRegion {
        offset: 0x200,
        size: 0x300,
    };
    let mut strategy = SpareRegionStrategy::EarlyImageSlot;
    if symbols.cfi_check.valid() && symbols.cfi_check.size > region.size {
        region = symbols.cfi_check;
        strategy = SpareRegionStrategy::CfiCheck;
    }

    let start_branch_addr = region.offset;
    consume_region(&mut region, 4, "reserve pre-6.1 branch slot")?;
    let root_key_addr = region.offset;

    let n = execve.patch_do_execve(
        base,
        &region,
        offsets.cred_offset,
        offsets.seccomp_offset,
        writes,
    );
    consume_region(&mut region, n, "do_execve hook")?;

    let current_avc_check_addr = region.offset;
    let n = PatchCurrentAvcCheck::new().patch_current_avc_check_bl_func(
        base,
        &region,
        offsets.cred_offset,
        writes,
    );
    consume_region(&mut region, n, "current avc check helper")?;

    let n = PatchAvcDenied::new(symbols.avc_denied).patch_avc_denied(
        base,
        &region,
        current_avc_check_addr,
        writes,
    );
    consume_region(&mut region, n, "avc_denied hook")?;

    let n = base.patch_jump(start_branch_addr, region.offset, writes);
    if n == 0 {
        return Err(SkrootPatchPlanError::PatchFailed("pre-6.1 branch guard"));
    }

    Ok((root_key_addr, strategy))
}

fn patch_6_1_plus(
    kernel: &[u8],
    base: &mut PatchBase<'_>,
    symbols: &KernelSymbolOffset,
    offsets: &SkrootKernelOffsets,
    writes: &mut Vec<PatchBytes>,
    execve: PatchDoExecve,
) -> Result<(u64, SpareRegionStrategy), SkrootPatchPlanError> {
    if !symbols.die.valid() {
        return Err(SkrootPatchPlanError::NoSpareRegion("missing die".into()));
    }
    if !symbols.drm_printfn_coredump.valid() {
        return Err(SkrootPatchPlanError::NoSpareRegion(
            "missing __drm_printfn_coredump".into(),
        ));
    }

    let mut exec_region = symbols.die;
    let root_key_addr = exec_region.offset;
    let n = execve.patch_do_execve(
        base,
        &exec_region,
        offsets.cred_offset,
        offsets.seccomp_offset,
        writes,
    );
    consume_region(&mut exec_region, n, "do_execve hook")?;

    let mut avc_region = symbols.drm_printfn_coredump;
    let n = patch_bytes::patch_ret(kernel, avc_region.offset, writes);
    consume_region(&mut avc_region, n, "__drm_printfn_coredump entry guard")?;

    let current_avc_check_addr = avc_region.offset;
    let n = PatchCurrentAvcCheck::new().patch_current_avc_check_bl_func(
        base,
        &avc_region,
        offsets.cred_offset,
        writes,
    );
    consume_region(&mut avc_region, n, "current avc check helper")?;

    let n = PatchAvcDenied::new(symbols.avc_denied).patch_avc_denied(
        base,
        &avc_region,
        current_avc_check_addr,
        writes,
    );
    consume_region(&mut avc_region, n, "avc_denied hook")?;

    Ok((root_key_addr, SpareRegionStrategy::DieAndDrmPrintfCoredump))
}

fn consume_region(
    region: &mut SymbolRegion,
    n: usize,
    step: &'static str,
) -> Result<(), SkrootPatchPlanError> {
    if n == 0 || !region.valid() {
        return Err(SkrootPatchPlanError::PatchFailed(step));
    }
    region.consume(n as u64);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_6_1_prefers_large_consumed_cfi_region() {
        let mut symbols = KernelSymbolOffset {
            cfi_check: SymbolRegion {
                offset: 0x1000,
                size: 0x400,
            },
            ..Default::default()
        };
        let mut writes = Vec::new();
        let kernel = vec![0u8; 0x2000];

        patch_existing_bypasses(&kernel, &mut symbols, &mut writes);

        assert_eq!(symbols.cfi_check.offset, 0x1004);
        assert_eq!(symbols.cfi_check.size, 0x3fc);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].addr, 0x1000);
    }
}
