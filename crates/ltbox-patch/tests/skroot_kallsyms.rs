//! Integration coverage for the SKRoot kallsyms decoder against real kernels.
//!
//! Decoding is only meaningful on genuine kernel images, which are too large to
//! vendor. Point `SKROOT_TEST_KERNELS` at one or more extracted kernels
//! (path-separator–joined: `;` on Windows, `:` elsewhere) to exercise it:
//!
//! ```text
//! magiskboot unpack boot.img            # produces ./kernel
//! SKROOT_TEST_KERNELS=/path/to/kernel cargo test -p ltbox-patch --test skroot_kallsyms
//! ```
//!
//! With the variable unset the test is a no-op so CI stays green.

use ltbox_patch::skroot::kallsyms;
use ltbox_patch::skroot::version::KernelVersion;

/// Function symbols every in-scope kernel exports; used to sanity-check that
/// resolved offsets land inside the image.
const EXPECTED_FUNCS: &[&str] = &[
    "do_execveat_common",
    "avc_denied",
    "audit_log_start",
    "filldir64",
    "prctl_get_seccomp",
    "commit_creds",
];

#[test]
fn decodes_real_kernels() {
    let Ok(list) = std::env::var("SKROOT_TEST_KERNELS") else {
        eprintln!("SKROOT_TEST_KERNELS unset — skipping real-kernel decode test");
        return;
    };

    // Use the platform path separator so Windows drive colons survive.
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut checked = 0;
    for path in list.split(sep).map(str::trim).filter(|p| !p.is_empty()) {
        let buf = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let ver = KernelVersion::from_kernel(&buf)
            .unwrap_or_else(|| panic!("{path}: no Linux version banner"));

        let syms = kallsyms::analyze(&buf)
            .unwrap_or_else(|e| panic!("{path} (v{}): decode failed: {e:?}", ver.raw()));

        assert!(
            syms.len() > 10_000,
            "{path}: implausibly few symbols ({})",
            syms.len()
        );

        // The symbol base is anchored on _stext; it must map back to the start
        // of the static code section (well inside the image).
        let stext = syms
            .lookup("_stext")
            .unwrap_or_else(|| panic!("{path}: missing _stext"));
        assert!(
            stext > 0 && (stext as usize) < buf.len(),
            "{path}: _stext offset 0x{stext:x} out of image"
        );

        // Every expected function must resolve to a real in-image offset.
        for func in EXPECTED_FUNCS {
            let off = syms
                .lookup(func)
                .unwrap_or_else(|| panic!("{path} (v{}): missing {func}", ver.raw()));
            assert!(
                (off as usize) + 4 <= buf.len(),
                "{path}: {func} offset 0x{off:x} out of image"
            );
        }

        eprintln!(
            "ok {path}: v{} — {} symbols, _stext@0x{stext:x}",
            ver.raw(),
            syms.len()
        );
        checked += 1;
    }

    assert!(
        checked > 0,
        "SKROOT_TEST_KERNELS set but no kernel readable"
    );
}
