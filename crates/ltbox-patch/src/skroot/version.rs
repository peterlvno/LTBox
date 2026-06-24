//! Kernel version detection from the embedded `Linux version ` banner.
//!
//! Port of upstream `analyze/kernel_version_parser.{h,cpp}`. The version gates
//! which kallsyms layout and which spare-region strategy the patcher uses
//! (pre-6.1 reserved region vs. 6.1+ scratch reuse), so it is parsed before any
//! symbol analysis.
#![allow(dead_code)]

const BANNER: &[u8] = b"Linux version ";

/// A kernel version triple (`major.minor.patch`) recovered from the banner.
///
/// [`raw`](Self::raw) keeps the exact text that followed `Linux version `
/// (e.g. `"6.1.25"`); comparisons use the numeric triple, with any missing
/// component treated as `0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelVersion {
    raw: String,
    triple: (u32, u32, u32),
}

impl KernelVersion {
    /// Locate the first `Linux version <digit>` banner in the kernel image and
    /// parse the version that follows. Returns `None` when no banner is present.
    pub fn from_kernel(buf: &[u8]) -> Option<KernelVersion> {
        let raw = find_banner_version(buf)?;
        let triple = parse_triple(&raw);
        Some(KernelVersion { raw, triple })
    }

    /// The exact version text extracted from the banner.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// `major.minor.patch` as a numeric triple (missing parts are `0`).
    pub fn triple(&self) -> (u32, u32, u32) {
        self.triple
    }

    pub fn major(&self) -> u32 {
        self.triple.0
    }
    pub fn minor(&self) -> u32 {
        self.triple.1
    }
    pub fn patch(&self) -> u32 {
        self.triple.2
    }

    /// True when this version is strictly older than `other` (upstream
    /// `is_kernel_version_less`).
    pub fn is_less_than(&self, other: (u32, u32, u32)) -> bool {
        self.triple < other
    }

    /// True when this version is `other` or newer.
    pub fn is_at_least(&self, other: (u32, u32, u32)) -> bool {
        self.triple >= other
    }
}

/// Scan for `Linux version ` immediately followed by an ASCII digit and return
/// the run of digits and dots that starts there.
fn find_banner_version(buf: &[u8]) -> Option<String> {
    let end = buf.len().checked_sub(BANNER.len())?;
    for i in 0..=end {
        if &buf[i..i + BANNER.len()] == BANNER {
            let start = i + BANNER.len();
            if buf.get(start).is_some_and(u8::is_ascii_digit) {
                return Some(extract_version(buf, start));
            }
        }
    }
    None
}

/// Collect the leading `[0-9.]*` run as a UTF-8 string.
fn extract_version(buf: &[u8], start: usize) -> String {
    let mut s = String::new();
    for &b in &buf[start..] {
        if b.is_ascii_digit() || b == b'.' {
            s.push(b as char);
        } else {
            break;
        }
    }
    s
}

/// Split a dotted version into a `major.minor.patch` triple, ignoring any
/// 4th+ components and treating empty / unparseable parts as `0`.
fn parse_triple(version: &str) -> (u32, u32, u32) {
    let mut it = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = it.next().unwrap_or(0);
    let minor = it.next().unwrap_or(0);
    let patch = it.next().unwrap_or(0);
    (major, minor, patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(banner: &str) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v.extend_from_slice(banner.as_bytes());
        v.extend_from_slice(b"\0 some trailing junk");
        v
    }

    #[test]
    fn parses_typical_banner() {
        let v = KernelVersion::from_kernel(&img(
            "Linux version 6.1.25-android14-11-g0123456 (build@host)",
        ))
        .unwrap();
        assert_eq!(v.raw(), "6.1.25");
        assert_eq!(v.triple(), (6, 1, 25));
        assert_eq!((v.major(), v.minor(), v.patch()), (6, 1, 25));
    }

    #[test]
    fn pads_missing_components() {
        let v = KernelVersion::from_kernel(&img("Linux version 5.10 ...")).unwrap();
        assert_eq!(v.raw(), "5.10");
        assert_eq!(v.triple(), (5, 10, 0));

        let v = KernelVersion::from_kernel(&img("Linux version 4 ...")).unwrap();
        assert_eq!(v.triple(), (4, 0, 0));
    }

    #[test]
    fn missing_banner_is_none() {
        assert!(KernelVersion::from_kernel(b"no banner here").is_none());
        // "Linux version " not followed by a digit is ignored.
        assert!(KernelVersion::from_kernel(b"Linux version unknown").is_none());
    }

    #[test]
    fn finds_first_banner_only() {
        let buf = img("Linux version 3.18.0 ... Linux version 9.9.9");
        let v = KernelVersion::from_kernel(&buf).unwrap();
        assert_eq!(v.triple(), (3, 18, 0));
    }

    #[test]
    fn version_comparisons() {
        let v = KernelVersion::from_kernel(&img("Linux version 6.1.0-x")).unwrap();
        assert!(v.is_less_than((6, 2, 0)));
        assert!(v.is_less_than((6, 1, 1)));
        assert!(v.is_less_than((7, 0, 0)));
        assert!(!v.is_less_than((6, 1, 0))); // equal is not less
        assert!(!v.is_less_than((5, 99, 99)));
        assert!(v.is_at_least((6, 1, 0)));
        assert!(v.is_at_least((6, 0, 0)));
        assert!(!v.is_at_least((6, 1, 1)));

        // the 6.1 boundary that selects the spare-region strategy
        let old = KernelVersion::from_kernel(&img("Linux version 5.15.123-y")).unwrap();
        assert!(old.is_less_than((6, 1, 0)));
        let new = KernelVersion::from_kernel(&img("Linux version 6.1.0-z")).unwrap();
        assert!(!new.is_less_than((6, 1, 0)));
    }
}
