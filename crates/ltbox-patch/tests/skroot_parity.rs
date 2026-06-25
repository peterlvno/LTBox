//! SKRoot parity coverage against the upstream Lite patcher.
//!
//! This is gated because it needs local real boot images and the upstream
//! Windows executable:
//!
//! ```powershell
//! $env:SKROOT_PARITY_EXE = 'D:\path\patch_kernel_root(2026-6-1).exe'
//! $env:SKROOT_PARITY_BOOTS = 'D:\path\a\boot.img;D:\path\b\boot.img'
//! # Optional: keep unpacked temp dirs for binary diffing.
//! $env:SKROOT_PARITY_KEEP = '1'
//! cargo test -p ltbox-patch --test skroot_parity -- --nocapture
//! ```

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ltbox_patch::boot;
use ltbox_patch::skroot::patch_bytes::PatchBytes;
use ltbox_patch::skroot::patch_plan;
use sha2::{Digest, Sha256};

const ROOT_KEY: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV";

fn apply_write(buf: &mut [u8], write: &PatchBytes) {
    let start = usize::try_from(write.addr).expect("write address fits usize");
    let end = start + write.bytes.len();
    buf[start..end].copy_from_slice(&write.bytes);
}

fn apply_ltbox_kernel_patch(kernel_path: &Path) {
    let mut kernel =
        fs::read(kernel_path).unwrap_or_else(|e| panic!("read {}: {e}", kernel_path.display()));
    let plan = patch_plan::build_core_patch_plan(&kernel)
        .unwrap_or_else(|e| panic!("patch plan {}: {e}", kernel_path.display()));
    for write in &plan.writes {
        apply_write(&mut kernel, write);
    }

    let mut stored_key = [0u8; 48];
    stored_key[..47].copy_from_slice(&ROOT_KEY.as_bytes()[..47]);
    apply_write(
        &mut kernel,
        &PatchBytes {
            addr: plan.root_key_addr,
            bytes: stored_key.to_vec(),
        },
    );
    fs::write(kernel_path, kernel)
        .unwrap_or_else(|e| panic!("write {}: {e}", kernel_path.display()));
}

fn apply_upstream_kernel_patch(exe: &Path, kernel_path: &Path) {
    let mut child = Command::new(exe)
        .arg(kernel_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {}: {e}", exe.display()));
    {
        let stdin = child.stdin.as_mut().expect("upstream stdin");
        stdin
            .write_all(format!("2\n{ROOT_KEY}\n1\n\n").as_bytes())
            .expect("write upstream stdin");
    }
    let output = child.wait_with_output().expect("wait upstream patcher");
    assert!(
        output.status.success(),
        "upstream patcher failed for {}\nstatus={:?}\nstdout={}\nstderr={}",
        kernel_path.display(),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_window(bytes: &[u8], start: usize, len: usize) -> String {
    bytes
        .iter()
        .skip(start)
        .take(len)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn mismatch_summary(left: &[u8], right: &[u8]) -> String {
    let common = left.len().min(right.len());
    let first = (0..common).find(|&i| left[i] != right[i]);
    let diff_count =
        (0..common).filter(|&i| left[i] != right[i]).count() + left.len().max(right.len()) - common;
    match first {
        Some(i) => {
            let start = i.saturating_sub(8);
            let len = 32.min(common.saturating_sub(start));
            format!(
                "first_diff=0x{i:x}, diff_bytes={diff_count}, ltbox_sha256={}, upstream_sha256={}, ltbox[0x{start:x}..]={}, upstream[0x{start:x}..]={}",
                sha256_hex(left),
                sha256_hex(right),
                hex_window(left, start, len),
                hex_window(right, start, len)
            )
        }
        None => format!(
            "length mismatch only: ltbox_len={}, upstream_len={}, diff_bytes={diff_count}, ltbox_sha256={}, upstream_sha256={}",
            left.len(),
            right.len(),
            sha256_hex(left),
            sha256_hex(right)
        ),
    }
}

fn assert_bytes_eq(label: &str, boot_img: &Path, left: &[u8], right: &[u8]) {
    assert!(
        left == right,
        "{label} mismatch for {}\n{}",
        boot_img.display(),
        mismatch_summary(left, right)
    );
}

fn workspace_target_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/skroot-parity")
        .components()
        .collect()
}

fn run_one(exe: &Path, boot_img: &Path, index: usize) {
    let temp_root = workspace_target_dir();
    fs::create_dir_all(&temp_root).expect("create parity target dir");
    let temp = tempfile::Builder::new()
        .prefix(&format!("case-{index}-"))
        .tempdir_in(&temp_root)
        .expect("create parity tempdir");
    let keep_temp = env::var_os("SKROOT_PARITY_KEEP").is_some();
    let temp_path = temp.path().to_path_buf();
    let _temp_guard = if keep_temp {
        eprintln!("keeping SKRoot parity tempdir: {}", temp_path.display());
        let _ = temp.keep();
        None
    } else {
        Some(temp)
    };
    let ltbox_dir = temp_path.join("ltbox");
    let upstream_dir = temp_path.join("upstream");
    fs::create_dir_all(&ltbox_dir).expect("create ltbox dir");
    fs::create_dir_all(&upstream_dir).expect("create upstream dir");

    let ltbox_boot = ltbox_dir.join("boot.img");
    let upstream_boot = upstream_dir.join("boot.img");
    fs::copy(boot_img, &ltbox_boot).expect("copy ltbox boot");
    fs::copy(boot_img, &upstream_boot).expect("copy upstream boot");

    boot::unpack(&ltbox_boot, &ltbox_dir).expect("ltbox unpack");
    boot::unpack(&upstream_boot, &upstream_dir).expect("upstream unpack");

    apply_ltbox_kernel_patch(&ltbox_dir.join("kernel"));
    apply_upstream_kernel_patch(exe, &upstream_dir.join("kernel"));

    boot::repack("boot.img", &ltbox_dir).expect("ltbox repack");
    boot::repack("boot.img", &upstream_dir).expect("upstream repack");

    let ltbox_kernel = fs::read(ltbox_dir.join("kernel")).expect("read ltbox kernel");
    let upstream_kernel = fs::read(upstream_dir.join("kernel")).expect("read upstream kernel");
    assert_bytes_eq("patched kernel", boot_img, &ltbox_kernel, &upstream_kernel);

    let ltbox_new_boot = fs::read(ltbox_dir.join("new-boot.img")).expect("read ltbox new boot");
    let upstream_new_boot =
        fs::read(upstream_dir.join("new-boot.img")).expect("read upstream new boot");
    assert_bytes_eq(
        "patched boot",
        boot_img,
        &ltbox_new_boot,
        &upstream_new_boot,
    );
}

#[test]
fn skroot_lite_matches_upstream_patcher() {
    let Ok(boots) = env::var("SKROOT_PARITY_BOOTS") else {
        eprintln!("SKROOT_PARITY_BOOTS unset; skipping SKRoot parity test");
        return;
    };
    let exe = PathBuf::from(env::var("SKROOT_PARITY_EXE").expect("SKROOT_PARITY_EXE"));
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut count = 0usize;
    for (index, boot) in boots
        .split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        count += 1;
        run_one(&exe, Path::new(boot), index);
    }
    assert!(count > 0, "SKROOT_PARITY_BOOTS had no paths");
}
