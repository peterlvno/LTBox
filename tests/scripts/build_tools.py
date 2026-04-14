import os
import shutil
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_DIR = REPO_ROOT / "bin" / "tools"
SUBMODULE_DIR = REPO_ROOT / "vendor" / "MagiskbootAlone"
MAGISKBOOT_EXE = TOOLS_DIR / "magiskboot.exe"
MAGISKBOOT_XZ_HELPER_EXE = TOOLS_DIR / "magiskboot_xz_helper.exe"
OPENSSL_EXE = TOOLS_DIR / "openssl.exe"
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

    if not SUBMODULE_DIR.exists() or not (SUBMODULE_DIR / "CMakeLists.txt").exists():
        print("[ERROR] vendor/MagiskbootAlone submodule not initialized.")
        print("       Run: git submodule update --init vendor/MagiskbootAlone")
        raise SystemExit(1)

    current_sha = _get_submodule_sha()

    openssl_ready = OPENSSL_EXE.exists() if os.name == "nt" else True
    helper_ready = MAGISKBOOT_XZ_HELPER_EXE.exists() if os.name == "nt" else True
    if (
        MAGISKBOOT_EXE.exists()
        and VERSION_FILE.exists()
        and openssl_ready
        and helper_ready
    ):
        cached_sha = VERSION_FILE.read_text(encoding="utf-8").strip()
        if cached_sha == current_sha:
            print("[INFO] Tools are up-to-date. Skipping build.")
            return

    print("[INFO] Building magiskboot from vendor/MagiskbootAlone...")

    # Copy submodule source to a temp build dir (avoid polluting the submodule)
    build_dir = TOOLS_DIR / "magiskboot_build"
    if build_dir.exists():
        shutil.rmtree(build_dir, ignore_errors=True)
        if build_dir.exists() and os.name == "nt":
            subprocess.run(
                ["cmd", "/c", "rmdir", "/s", "/q", str(build_dir)], check=False
            )

    shutil.copytree(SUBMODULE_DIR, build_dir, dirs_exist_ok=True)

    cpio_cpp_path = build_dir / "src" / "cpio.cpp"
    if cpio_cpp_path.exists():
        content = cpio_cpp_path.read_text(encoding="utf-8")
        content = content.replace(
            '"/tmp/magiskboot-cpio-XXXXXX"', '"magiskboot-cpio-XXXXXX"'
        )
        cpio_cpp_path.write_text(content, encoding="utf-8")
        print("[INFO] Patched hardcoded /tmp/ path in cpio.cpp")

    if os.name == "nt":
        msys_root = Path("C:/msys64")
        bash_exe = msys_root / "usr/bin/bash.exe"

        if not bash_exe.exists():
            print(
                "[WARN] MSYS2 not found at C:/msys64. Cannot build POSIX C++ code on Windows."
            )
            return

        print(
            "[INFO] Installing MSYS2 dependencies (gcc, cmake, make, zlib-devel, openssl)..."
        )
        subprocess.run(
            [
                str(bash_exe),
                "-lc",
                "pacman -S --noconfirm --overwrite '*' --needed gcc cmake make zlib-devel openssl",
            ],
            check=True,
        )

        src_dir_msys = build_dir.as_posix()

        build_cmd = (
            f"cd '{src_dir_msys}' && "
            f"cmake -S . -B build -G 'Unix Makefiles' "
            f"-DMBEDTLS_FATAL_WARNINGS=OFF "
            f"-DCMAKE_EXE_LINKER_FLAGS='-static-libgcc -static-libstdc++' "
            f"-DCMAKE_CXX_FLAGS='-D_GNU_SOURCE -Wno-error' -DCMAKE_C_FLAGS='-D_GNU_SOURCE -Wno-error' && "
            f"cmake --build build --config Release"
        )
        subprocess.run([str(bash_exe), "-lc", build_cmd], check=True)

        cargo_exe = _find_cargo_exe()
        if cargo_exe is None:
            raise RuntimeError(
                "cargo not found; required to build magiskboot_xz_helper.exe"
            )
        helper_manifest = build_dir / "rust" / "magiskboot_xz_helper" / "Cargo.toml"
        subprocess.run(
            [
                cargo_exe,
                "build",
                "--manifest-path",
                str(helper_manifest),
                "--release",
            ],
            check=True,
        )

        compiled_exe = build_dir / "build" / "magiskboot.exe"
        compiled_helper = (
            build_dir
            / "rust"
            / "magiskboot_xz_helper"
            / "target"
            / "release"
            / "magiskboot_xz_helper.exe"
        )
        if compiled_exe.exists():
            shutil.copy(compiled_exe, MAGISKBOOT_EXE)
            if not compiled_helper.exists():
                raise RuntimeError("magiskboot_xz_helper.exe was not produced")
            shutil.copy(compiled_helper, MAGISKBOOT_XZ_HELPER_EXE)

            msys_openssl = msys_root / "usr/bin/openssl.exe"
            if msys_openssl.exists():
                shutil.copy(msys_openssl, OPENSSL_EXE)
                print("[INFO] Copied openssl.exe from MSYS2.")

            dlls_to_copy = ["msys-2.0.dll", "msys-z.dll"]
            usr_bin = msys_root / "usr/bin"

            for dll_file in usr_bin.glob("msys-crypto-*.dll"):
                dlls_to_copy.append(dll_file.name)
            for dll_file in usr_bin.glob("msys-ssl-*.dll"):
                dlls_to_copy.append(dll_file.name)

            for dll in list(set(dlls_to_copy)):
                src_dll = usr_bin / dll
                if src_dll.exists():
                    shutil.copy(src_dll, TOOLS_DIR / dll)
                    print(f"[INFO] Copied {dll} for standalone execution.")
        else:
            print("[WARN] Build failed to produce magiskboot.exe.")
    else:
        subprocess.run(
            ["cmake", "-S", str(build_dir), "-B", str(build_dir / "build")], check=True
        )
        subprocess.run(
            ["cmake", "--build", str(build_dir / "build"), "--config", "Release"],
            check=True,
        )

        compiled_exe = build_dir / "build" / "magiskboot"
        if compiled_exe.exists():
            shutil.copy(compiled_exe, MAGISKBOOT_EXE)

    VERSION_FILE.write_text(current_sha, encoding="utf-8")

    shutil.rmtree(build_dir, ignore_errors=True)
    print("[INFO] Successfully built and cached tools.")


if __name__ == "__main__":
    build()
