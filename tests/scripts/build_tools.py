import os
import shutil
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_DIR = REPO_ROOT / "bin" / "tools"
SUBMODULE_DIR = REPO_ROOT / "vendor" / "magiskboot-rs"
MAGISKBOOT_EXE = TOOLS_DIR / "magiskboot.exe"
VERSION_FILE = TOOLS_DIR / "magiskboot.version"


def _get_submodule_sha() -> str:
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=str(SUBMODULE_DIR),
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def _find_cargo_exe() -> str | None:
    candidates = [
        shutil.which("cargo.exe"),
        shutil.which("cargo"),
        str(Path.home() / ".cargo" / "bin" / "cargo.exe"),
        str(Path.home() / ".cargo" / "bin" / "cargo"),
    ]
    for candidate in candidates:
        if candidate and Path(candidate).exists():
            return candidate
    return None


def build():
    TOOLS_DIR.mkdir(parents=True, exist_ok=True)

    if not SUBMODULE_DIR.exists() or not (SUBMODULE_DIR / "Cargo.toml").exists():
        print("[ERROR] vendor/magiskboot-rs submodule not initialized.")
        print("       Run: git submodule update --init vendor/magiskboot-rs")
        raise SystemExit(1)

    current_sha = _get_submodule_sha()

    if MAGISKBOOT_EXE.exists() and VERSION_FILE.exists():
        cached_sha = VERSION_FILE.read_text(encoding="utf-8").strip()
        if cached_sha == current_sha:
            print("[INFO] magiskboot is up-to-date. Skipping build.")
            return

    cargo_exe = _find_cargo_exe()
    if cargo_exe is None:
        raise RuntimeError("cargo not found; required to build magiskboot-rs")

    print("[INFO] Building magiskboot from vendor/magiskboot-rs...")

    subprocess.run(
        [cargo_exe, "build", "--release"],
        cwd=str(SUBMODULE_DIR),
        check=True,
    )

    if os.name == "nt":
        compiled_exe = SUBMODULE_DIR / "target" / "release" / "magiskboot.exe"
    else:
        compiled_exe = SUBMODULE_DIR / "target" / "release" / "magiskboot"

    if not compiled_exe.exists():
        raise RuntimeError(f"Build did not produce {compiled_exe}")

    shutil.copy(compiled_exe, MAGISKBOOT_EXE)
    VERSION_FILE.write_text(current_sha, encoding="utf-8")
    print("[INFO] Successfully built and cached magiskboot.")


if __name__ == "__main__":
    build()
