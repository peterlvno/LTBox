//! Locate `init_cred` by byte-pattern scan — port of upstream
//! `analyze/init_cred_searcher.{h,cpp}`.
//!
//! `init_cred` is the kernel's root credential: usage counter, an all-zero
//! `{uid,gid,…}` block, default securebits, and a full capability set. The exact
//! field widths and the "full set" value vary by kernel, so candidate byte
//! patterns are built for every combination and scanned for. The match confirms
//! which layout this kernel uses and yields the credential bytes the `do_execve`
//! hook copies to grant root.
#![allow(dead_code)]

/// `ATOMIC_INIT(4)` — the value of `init_cred.usage`.
const ATOMIC_INIT_4: u64 = 4;

/// Candidate "full capability set" values across kernel versions, most
/// permissive first (upstream order).
const CAP_FULL_SETS: [u64; 5] = [
    0x1FF_FFFF_FFFF, // 5.9.0
    0x0FF_FFFF_FFFF, // 5.8.0
    0x03F_FFFF_FFFF, // 3.16.0
    0x07F_FFFF_FFFF, // Huawei 4.9.x
    0x01F_FFFF_FFFF, // baseline CAP_FULL_SET
];

/// A matched `init_cred`: the exact bytes plus the layout it confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitCred {
    /// The credential pattern bytes (head block + capability block).
    pub head: Vec<u8>,
    /// Width of the atomic usage counter (4 or 8).
    pub atomic_usage_size: usize,
    /// Width of the securebits field (4 or 8).
    pub securebits_size: usize,
    /// Number of capability sets present in `/proc` status (4 or 5).
    pub cap_cnt: i32,
    /// The full capability value that matched.
    pub cap_ability_max: u64,
    /// File offset where the pattern was found.
    pub offset: u64,
}

/// Find `init_cred`. `cred_uid_offset` (4 or 8, from the cred/uid finder) selects
/// the atomic-usage width. Returns `None` if no candidate pattern matches.
pub fn find_init_cred(buf: &[u8], cred_uid_offset: u64) -> Option<InitCred> {
    let usage_size = if cred_uid_offset == 4 { 4 } else { 8 };
    let cap_cnt = get_cap_cnt(buf);
    let candidates = build_candidates(usage_size, cap_cnt);

    for mut cand in candidates {
        let pat = &cand.head;
        if pat.is_empty() || pat.len() > buf.len() {
            continue;
        }
        let mut off = 0;
        while off + pat.len() <= buf.len() {
            if &buf[off..off + pat.len()] == pat.as_slice() {
                cand.offset = off as u64;
                return Some(cand);
            }
            off += 4;
        }
    }
    None
}

/// `/proc/<pid>/status` exposes `CapAmb:` from kernel 4.3 — its presence means
/// five capability sets rather than four.
fn get_cap_cnt(buf: &[u8]) -> i32 {
    if contains(buf, b"CapAmb:") { 5 } else { 4 }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Build every candidate credential pattern for a given usage width, in the
/// upstream order (each securebits width is added twice, harmlessly).
///
/// The capability block is fixed at four `u64`s (inheritable/permitted/
/// effective/bset), matching upstream's `cred_cap_info4` — this is enough to
/// uniquely locate `init_cred`. `cap_cnt` (4 or 5) is carried as metadata for
/// the `do_execve` hook to decide whether to also write `cap_ambient`; it is
/// deliberately *not* part of the search pattern, since the four-field pattern
/// already matches even on five-capability kernels.
fn build_candidates(usage_size: usize, cap_cnt: i32) -> Vec<InitCred> {
    let mut out = Vec::with_capacity(CAP_FULL_SETS.len() * 4);
    for &cap_max in &CAP_FULL_SETS {
        let cap_block = capability_block(cap_max);
        for &sec_size in &[4usize, 4, 8, 8] {
            let mut head = head_block(usage_size, sec_size);
            head.extend_from_slice(&cap_block);
            out.push(InitCred {
                head,
                atomic_usage_size: usage_size,
                securebits_size: sec_size,
                cap_cnt,
                cap_ability_max: cap_max,
                offset: 0,
            });
        }
    }
    out
}

/// `[usage=4][32 zero bytes (uid/gid/…)][securebits=0]`.
fn head_block(usage_size: usize, sec_size: usize) -> Vec<u8> {
    let mut b = Vec::with_capacity(usage_size + 32 + sec_size);
    push_int(&mut b, ATOMIC_INIT_4, usage_size);
    b.extend_from_slice(&[0u8; 32]); // cred_uid_info: 8 × u32, all zero
    push_int(&mut b, 0, sec_size);
    b
}

/// `[cap_inheritable=0][cap_permitted=max][cap_effective=max][cap_bset=max]`,
/// all `u64`.
fn capability_block(cap_max: u64) -> Vec<u8> {
    let mut b = Vec::with_capacity(32);
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(&cap_max.to_le_bytes());
    b.extend_from_slice(&cap_max.to_le_bytes());
    b.extend_from_slice(&cap_max.to_le_bytes());
    b
}

fn push_int(out: &mut Vec<u8>, value: u64, size: usize) {
    let le = value.to_le_bytes();
    out.extend_from_slice(&le[..size]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_cap_block_sizes() {
        assert_eq!(head_block(4, 4).len(), 40);
        assert_eq!(head_block(8, 8).len(), 48);
        assert_eq!(capability_block(0).len(), 32);
        // usage value is 4, little-endian.
        assert_eq!(&head_block(8, 4)[..8], &4u64.to_le_bytes());
    }

    #[test]
    fn finds_embedded_pattern() {
        // Build a buffer with one candidate pattern at a 4-aligned offset.
        let cand = &build_candidates(8, 4)[0]; // usage8, sec4, cap 5.9.0
        let pat = cand.head.clone();
        let mut buf = vec![0xAAu8; 64];
        buf.extend_from_slice(&pat);
        buf.extend_from_slice(&[0xBB; 16]);

        let found = find_init_cred(&buf, 8).expect("pattern found");
        assert_eq!(found.offset, 64);
        assert_eq!(found.atomic_usage_size, 8);
        assert_eq!(found.cap_ability_max, 0x1FF_FFFF_FFFF);
        assert_eq!(found.head, pat);
    }

    #[test]
    fn cred_uid_offset_selects_usage_width() {
        // A 4-byte-usage search builds and matches 4-byte-usage patterns.
        let pat4 = build_candidates(4, 4)[0].head.clone();
        let mut buf = vec![0x77u8; 16];
        buf.extend_from_slice(&pat4);
        let found = find_init_cred(&buf, 4).expect("4-byte usage pattern found");
        assert_eq!(found.atomic_usage_size, 4);
        assert_eq!(found.offset, 16);
    }

    #[test]
    fn no_match_returns_none() {
        let buf = vec![0x11u8; 200];
        assert_eq!(find_init_cred(&buf, 8), None);
    }
}
