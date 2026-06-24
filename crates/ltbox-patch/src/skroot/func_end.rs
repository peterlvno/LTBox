//! Function-boundary finder — port of upstream `3rdparty/find_end_func_offset.h`.
//!
//! Estimates where a function ends so a symbol's size can be bounded without a
//! symbol table. Upstream drives Capstone; here the same control-flow walk runs
//! on the bitmask predicates in [`insn`], since it only ever needs to know
//! whether each word is a branch or a return and where a branch points.
//!
//! Walk: linearly decode from the symbol start, following in-function forward
//! branches (so multi-block functions are covered) and recording every `ret`
//! address plus any backward unconditional jump (a tail-return boundary). The
//! largest such address is the function's last instruction; the caller adds one
//! instruction for the size.
#![allow(dead_code)]

use std::collections::HashSet;

use super::insn;

/// 5 MiB — the furthest a branch is trusted to stay within one function.
const MAX_JUMP_REGION: i64 = 5 * 1024 * 1024;
/// Hard caps mirroring upstream, to bound work on non-code input.
const MAX_LINES: usize = 0x10000;

#[derive(Clone, Copy)]
struct Line {
    /// Branch target as a start-relative byte offset (`Some` for PC-relative
    /// branches, including a genuine target of 0), or `None` for non-branches.
    target: Option<i64>,
    /// A PC-relative offset branch (`b`/`b.cond`/`cbz`/`cbnz`/`tbz`/`tbnz`).
    is_jump: bool,
    /// A block terminator with no fall-through: unconditional `b` or indirect
    /// `br`.
    is_force: bool,
    is_ret: bool,
}

fn rd32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Classify one instruction at start-relative address `rel`.
/// Returns the line plus its return kind (0 none / 1 ret / 2 retaa / 3 retab),
/// used only to reject functions mixing return forms.
fn classify(word: u32, rel: i64) -> (Line, u8) {
    let ret_mode = if insn::is_retab(word) {
        3
    } else if insn::is_retaa(word) {
        2
    } else if insn::is_ret(word) {
        1
    } else {
        0
    };

    let (is_jump, disp) = if insn::is_b(word) {
        (true, Some(insn::branch_b_displacement(word)))
    } else if insn::is_bcond(word) || insn::is_cbz(word) || insn::is_cbnz(word) {
        (true, Some(insn::branch_cond_displacement(word)))
    } else if insn::is_tbz(word) || insn::is_tbnz(word) {
        (true, Some(insn::branch_tbz_displacement(word)))
    } else {
        (false, None)
    };

    // Unconditional `b` and indirect `br`/`braa`/`brab` end a block with no
    // fall-through.
    let is_force = insn::is_b(word) || insn::is_br(word) || insn::is_br_auth(word);
    let target = disp.map(|d| rel + d as i64);

    (
        Line {
            target,
            is_jump,
            is_force,
            is_ret: ret_mode != 0,
        },
        ret_mode,
    )
}

/// Return the start-relative offset of the function's last instruction, or
/// `None` if no boundary is found within the caps. Upstream `find_end_func_offset`.
pub fn find_end_func_offset(buf: &[u8], start: usize) -> Option<usize> {
    let mut lines: Vec<Line> = Vec::new();
    let mut ret_mode = 0u8;

    let mut i = 0usize;
    loop {
        let rel = (i as i64) * 4;
        let off = start + i * 4;
        if off + 4 > buf.len() || i >= MAX_LINES || rel >= MAX_JUMP_REGION {
            break;
        }
        let (line, rm) = classify(rd32(buf, off), rel);
        if rm != 0 {
            if ret_mode == 0 {
                ret_mode = rm;
            } else if ret_mode != rm {
                // Mixed ret/retaa/retab: stop scanning (upstream breaks).
                break;
            }
        }
        // Re-evaluate after any block terminator. Upstream only re-checks on new
        // returns and so misses functions ending in a backward jump or `br`;
        // triggering on every force jump as well resolves those shapes.
        let boundary = rm != 0 || line.is_force;
        lines.push(line);
        i += 1;

        // Stop at the first fully-resolved candidate.
        if boundary && let Some(c) = handle_candidate(&lines) {
            return Some(c);
        }
    }
    None
}

/// Compute the boundary from the decoded lines, following forward branch forks.
/// `None` means either no return was reached or a fork points past what has been
/// decoded so far (caller keeps decoding).
fn handle_candidate(lines: &[Line]) -> Option<usize> {
    let mut ret_addrs: Vec<i64> = Vec::new();
    let mut anchors: Vec<i64> = Vec::new();
    let mut history: HashSet<i64> = HashSet::new();

    scan_from(lines, 0, &mut history, &mut ret_addrs, &mut anchors);
    while let Some(fork) = anchors.pop() {
        let idx = index_by_addr(lines, fork)?;
        scan_from(lines, idx, &mut history, &mut ret_addrs, &mut anchors);
    }
    ret_addrs.into_iter().max().map(|m| m as usize)
}

/// Walk forward from `start_idx`, collecting return addresses and discovering
/// new forward-branch forks until the block ends at a forced jump.
fn scan_from(
    lines: &[Line],
    start_idx: usize,
    history: &mut HashSet<i64>,
    ret_addrs: &mut Vec<i64>,
    anchors: &mut Vec<i64>,
) {
    for (x, line) in lines.iter().enumerate().skip(start_idx) {
        let addr = (x as i64) * 4;
        if line.is_jump {
            if let Some(t) = line.target
                && t > 0
                && t < addr + MAX_JUMP_REGION
                && history.insert(t)
            {
                anchors.push(t);
            }
            if line.is_force {
                // Unconditional `b` ends the block. A backward target (within
                // the function, `0..addr`) marks this as a tail-return boundary.
                if let Some(t) = line.target
                    && (0..addr).contains(&t)
                {
                    ret_addrs.push(addr);
                }
                break;
            }
            continue;
        }
        // Indirect `br`/`braa`/`brab` ends the block too (tail call / jump-table
        // exit); its target is unknowable, so it terminates without a ret.
        if line.is_force {
            break;
        }
        if line.is_ret {
            // A return has no fall-through: record it and stop this path. Bytes
            // after it are only reachable through a branch, handled via anchors.
            ret_addrs.push(addr);
            break;
        }
    }
}

/// Index of the line at start-relative `addr`. Lines are decoded densely every
/// 4 bytes, so this is exact; `None` means the address is beyond what has been
/// decoded.
fn index_by_addr(lines: &[Line], addr: i64) -> Option<usize> {
    if addr < 0 || addr % 4 != 0 {
        return None;
    }
    let idx = (addr / 4) as usize;
    (idx < lines.len()).then_some(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACIASP: u32 = 0xD503_233F;
    const RET: u32 = 0xD65F_03C0;
    const NOP: u32 = 0xD503_201F;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn straight_line_function_ends_at_ret() {
        // paciasp; nop; ret  → last instruction at offset 8.
        let buf = asm(&[PACIASP, NOP, RET]);
        assert_eq!(find_end_func_offset(&buf, 0), Some(8));
    }

    #[test]
    fn follows_forward_branch_to_later_ret() {
        // cbz w0, +8 ; ret ; ret  → the branch reaches the second ret at 8,
        // which is the true end even though a ret appears earlier at 4.
        let cbz_plus8 = 0x3400_0000 | (2 << 5); // imm19 = 2 → +8 bytes
        let buf = asm(&[cbz_plus8, RET, RET]);
        assert_eq!(find_end_func_offset(&buf, 0), Some(8));
    }

    #[test]
    fn respects_start_offset() {
        // Garbage word, then the function — decoding starts at offset 4.
        let buf = asm(&[0x1234_5678, PACIASP, NOP, RET]);
        assert_eq!(find_end_func_offset(&buf, 4), Some(8));
    }

    #[test]
    fn no_ret_yields_none() {
        let buf = asm(&[NOP, NOP, NOP]);
        assert_eq!(find_end_func_offset(&buf, 0), None);
    }

    #[test]
    fn forward_branch_into_backward_jump_tail() {
        // A function with no `ret`: a forward branch jumps over a block that
        // ends in a backward unconditional `b` (a tail loop). The boundary is
        // the backward jump at offset 8, which must be found even though no
        // return is ever decoded.
        //   0: cbz w0, +8   → target 8
        //   4: nop
        //   8: b -4         → backward, boundary
        let cbz_plus8 = 0x3400_0000 | (2 << 5); // imm19 = 2 → +8
        let b_minus4 = 0x1400_0000 | (((-1i32) as u32) & 0x03FF_FFFF); // imm26 = -1 → -4
        let buf = asm(&[cbz_plus8, NOP, b_minus4]);
        assert_eq!(find_end_func_offset(&buf, 0), Some(8));
    }

    #[test]
    fn backward_branch_to_entry_is_a_boundary() {
        // 0: cbz w0, +8 ; 4: nop ; 8: b 0  (loop back to entry, target 0).
        let cbz_plus8 = 0x3400_0000 | (2 << 5);
        let b_to_0 = 0x1400_0000 | (((-2i32) as u32) & 0x03FF_FFFF); // -8 from 8 → 0
        let buf = asm(&[cbz_plus8, NOP, b_to_0]);
        assert_eq!(find_end_func_offset(&buf, 0), Some(8));
    }

    #[test]
    fn indirect_br_terminates_without_overrun() {
        // 0: br x8  → block ends; nothing else is scanned. No recorded return,
        // so no boundary — and crucially it does not run into the trailing ret.
        let br_x8 = 0xD61F_0100;
        let buf = asm(&[br_x8, RET]);
        assert_eq!(find_end_func_offset(&buf, 0), None);
    }
}
