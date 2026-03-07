import json
import os
import shutil
import subprocess
import urllib.request
from pathlib import Path

TOOLS_DIR = Path(__file__).resolve().parents[2] / "bin" / "tools"
MAGISKBOOT_EXE = TOOLS_DIR / "magiskboot.exe"
VERSION_FILE = TOOLS_DIR / "magiskboot.version"
REPO_URL = "https://github.com/Anatdx/MagiskbootAlone.git"
API_URL = "https://api.github.com/repos/Anatdx/MagiskbootAlone/commits/main"


def get_latest_sha():
    req = urllib.request.Request(API_URL, headers={"User-Agent": "LTBox-Builder"})
    with urllib.request.urlopen(req, timeout=10) as response:
        data = json.loads(response.read().decode("utf-8"))
        return data["sha"]


def build():
    TOOLS_DIR.mkdir(parents=True, exist_ok=True)
    try:
        latest_sha = get_latest_sha()
    except Exception as e:
        print(f"[WARN] Failed to fetch latest SHA: {e}")
        latest_sha = None

    if MAGISKBOOT_EXE.exists() and VERSION_FILE.exists() and latest_sha:
        if VERSION_FILE.read_text(encoding="utf-8").strip() == latest_sha:
            print("[INFO] magiskboot.exe is up-to-date. Skipping build.")
            return

    print("[INFO] Building magiskboot from source via MSYS2 (POSIX Emulation)...")
    build_dir = TOOLS_DIR / "magiskboot_src"
    if build_dir.exists():
        shutil.rmtree(build_dir, ignore_errors=True)

    subprocess.run(["git", "clone", REPO_URL, str(build_dir)], check=True)

    if os.name == "nt":
        msys_root = Path("C:/msys64")
        bash_exe = msys_root / "usr/bin/bash.exe"

        if not bash_exe.exists():
            print(
                "[WARN] MSYS2 not found at C:/msys64. Cannot build POSIX C++ code on Windows."
            )
            return

        print(
            "[INFO] Installing MSYS2 dependencies (gcc, cmake, make, zlib-devel, git)..."
        )
        subprocess.run(
            [
                str(bash_exe),
                "-lc",
                "pacman -S --noconfirm --needed gcc cmake make zlib-devel git",
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

        compiled_exe = build_dir / "build" / "magiskboot.exe"
        if compiled_exe.exists():
            shutil.copy(compiled_exe, MAGISKBOOT_EXE)

            for dll in ["msys-2.0.dll", "msys-z.dll"]:
                src_dll = msys_root / "usr/bin" / dll
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

    if latest_sha:
        VERSION_FILE.write_text(latest_sha, encoding="utf-8")

    shutil.rmtree(build_dir, ignore_errors=True)
    print("[INFO] Successfully built and cached magiskboot.")


if __name__ == "__main__":
    build()
