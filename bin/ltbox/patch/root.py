import re
import shutil
import sys
import subprocess
from dataclasses import dataclass
from functools import cmp_to_key
from pathlib import Path
from pathlib import PurePosixPath
from typing import List, Optional, Union

from .. import constants as const
from .. import device, downloader, utils
from ..errors import DeviceCommandError, DeviceError, ToolError
from ..i18n import get_string
from ..root_profiles import RootProviderFamily, get_root_provider_profile

_PREINIT_DYNAMIC_MAJOR_MIN = 240
_PREINIT_DYNAMIC_MAJOR_MAX = 254
_PREINIT_DATA = 0
_PREINIT_CACHE = 1
_PREINIT_KLOGDUMP = 2
_PREINIT_METADATA = 3
_PREINIT_PERSIST = 4


@dataclass(frozen=True)
class _MountInfoEntry:
    device_major: int
    root: str
    target: str
    mount_options: str
    fs_type: str
    source: str


def _parse_mountinfo_entry(line: str) -> Optional[_MountInfoEntry]:
    line = line.strip()
    if not line:
        return None

    try:
        left, right = line.split(" - ", 1)
    except ValueError:
        return None

    left_fields = left.split()
    right_fields = right.split()
    if len(left_fields) < 6 or len(right_fields) < 3:
        return None

    try:
        device_major = int(left_fields[2].split(":", 1)[0])
    except (IndexError, ValueError):
        return None

    return _MountInfoEntry(
        device_major=device_major,
        root=left_fields[3],
        target=left_fields[4],
        mount_options=left_fields[5],
        fs_type=right_fields[0],
        source=right_fields[1],
    )


def _compare_preinit_candidates(
    a: tuple[int, _MountInfoEntry], b: tuple[int, _MountInfoEntry]
) -> int:
    a_part, a_info = a
    b_part, b_info = b
    a_ext4 = a_info.fs_type == "ext4"
    b_ext4 = b_info.fs_type == "ext4"

    if (a_part == _PREINIT_METADATA and b_ext4) or (
        b_part == _PREINIT_METADATA and a_ext4
    ):
        return (a_part > b_part) - (a_part < b_part)
    if a_ext4 and not b_ext4:
        return -1
    if not a_ext4 and b_ext4:
        return 1
    return (a_part > b_part) - (a_part < b_part)


def _find_magisk_preinit_device_from_mountinfo(
    mountinfo: str,
    crypto_state: str,
    crypto_type: str,
    crypto_metadata_enabled: str,
) -> str:
    if crypto_state != "encrypted":
        encrypt_type = "none"
    elif crypto_type == "block":
        encrypt_type = "block"
    elif crypto_metadata_enabled == "true":
        encrypt_type = "metadata"
    else:
        encrypt_type = "file"

    candidates: List[tuple[int, _MountInfoEntry]] = []

    for line in mountinfo.splitlines():
        info = _parse_mountinfo_entry(line)
        if info is None:
            continue
        if info.root != "/" or not info.source.startswith("/") or "/dm-" in info.source:
            continue
        if info.fs_type not in {"ext4", "f2fs"}:
            continue
        if "rw" not in info.mount_options.split(","):
            continue

        source_parent = PurePosixPath(info.source).parent.as_posix()
        if not source_parent.endswith("/by-name") and not source_parent.endswith(
            "/block"
        ):
            continue

        if (
            _PREINIT_DYNAMIC_MAJOR_MIN
            <= info.device_major
            <= _PREINIT_DYNAMIC_MAJOR_MAX
            and "/vd" not in info.source
            and "/by-name/" not in info.source
        ):
            continue

        part_id: Optional[int] = None
        if info.target in {"/persist", "/mnt/vendor/persist"}:
            part_id = _PREINIT_PERSIST
        elif info.target == "/metadata":
            part_id = _PREINIT_METADATA
        elif info.target == "/klogdump":
            part_id = _PREINIT_KLOGDUMP
        elif info.target == "/cache":
            part_id = _PREINIT_CACHE
        elif info.target == "/data" and encrypt_type in {"none", "file"}:
            part_id = _PREINIT_DATA

        if part_id is not None:
            candidates.append((part_id, info))

    if not candidates:
        return ""

    _, chosen = sorted(candidates, key=cmp_to_key(_compare_preinit_candidates))[0]
    return PurePosixPath(chosen.source).name


def _resolve_magisk_preinit_device(
    dev: Optional[device.DeviceController] = None,
) -> str:
    if dev is None or dev.skip_adb:
        return ""

    try:
        crypto_state = dev.adb.shell("getprop ro.crypto.state").strip()
        crypto_type = dev.adb.shell("getprop ro.crypto.type").strip()
        crypto_metadata_enabled = dev.adb.shell(
            "getprop ro.crypto.metadata.enabled"
        ).strip()
        mountinfo = dev.adb.shell("cat /proc/self/mountinfo")
    except DeviceError:
        return ""

    return _find_magisk_preinit_device_from_mountinfo(
        mountinfo=mountinfo,
        crypto_state=crypto_state,
        crypto_type=crypto_type,
        crypto_metadata_enabled=crypto_metadata_enabled,
    )


def patch_boot_with_root_algo(
    work_dir: Path,
    magiskboot_exe: Path,
    dev: Optional[device.DeviceController] = None,
    gki: bool = False,
    lkm_kernel_version: Optional[str] = None,
    root_type: str = "ksu",
    skip_lkm_download: bool = False,
    superkey: Optional[str] = None,
    kpm_paths: Optional[List[Path]] = None,
    custom_kernel_zip: Optional[Path] = None,
) -> Optional[Path]:

    img_name = const.FN_BOOT if gki else const.FN_INIT_BOOT
    out_img_name = const.FN_BOOT_ROOT if gki else const.FN_INIT_BOOT_ROOT

    patched_boot_path = const.BASE_DIR / out_img_name
    work_img_path = work_dir / img_name

    if not work_img_path.exists():
        print(
            get_string("img_root_err_img_not_found").format(name=img_name),
            file=sys.stderr,
        )
        return None

    mb = utils.MagiskBootWrapper(magiskboot_exe)

    provider = get_root_provider_profile(root_type)

    if provider.family == RootProviderFamily.APATCH:
        display_name = provider.display_name

        if superkey is None:
            utils.ui.error(
                get_string("apatch_err_superkey_required").format(name=display_name)
            )
            return None

        kptools_exe = const.TOOLS_DIR / "kptools.exe"
        kpimg_file = work_dir / "kpimg"

        utils.ui.echo(
            get_string("apatch_kptools_step").format(
                action="Unpacking", file=img_name, name=display_name
            )
        )

        cmd_unpack = [str(kptools_exe), "unpack", img_name]
        res_unpack = subprocess.run(
            cmd_unpack, cwd=work_dir, capture_output=True, text=True
        )
        if res_unpack.stdout:
            utils.ui.echo(res_unpack.stdout.strip())

        if res_unpack.returncode != 0:
            utils.ui.error(
                get_string("apatch_kptools_failed").format(
                    action="unpack", error=res_unpack.stderr, name=display_name
                )
            )
            return None

        kernel_file = work_dir / "kernel"
        kernel_ori = work_dir / "kernel.ori"

        if not kernel_file.exists():
            utils.ui.error(get_string("apatch_unpack_kernel_missing"))
            return None

        shutil.move(kernel_file, kernel_ori)

        utils.ui.echo(get_string("apatch_check_kernel").format(name=display_name))
        cmd_check = [str(kptools_exe), "-i", str(kernel_ori), "-f"]
        res_check = subprocess.run(
            cmd_check, cwd=work_dir, capture_output=True, text=True
        )

        if "CONFIG_KALLSYMS=y" not in res_check.stdout:
            utils.ui.error(
                get_string("apatch_err_kallsyms_required").format(name=display_name)
            )
            return None
        if "CONFIG_KALLSYMS_ALL=y" not in res_check.stdout:
            utils.ui.echo(get_string("apatch_warn_kallsyms_all"))

        utils.ui.echo(get_string("apatch_patch_start"))
        cmd_patch = [
            str(kptools_exe),
            "-p",
            "-i",
            str(kernel_ori),
            "-S",
            superkey,
            "-k",
            str(kpimg_file),
            "-o",
            str(kernel_file),
        ]
        if kpm_paths:
            for kpm_path in kpm_paths:
                cmd_patch.extend(["-M", str(kpm_path), "-T", "kpm"])
        res_patch = subprocess.run(
            cmd_patch, cwd=work_dir, capture_output=True, text=True
        )
        if res_patch.stdout:
            utils.ui.echo(res_patch.stdout.strip())

        if res_patch.returncode != 0:
            utils.ui.error(
                get_string("apatch_patch_failed").format(
                    error=res_patch.stderr, name=display_name
                )
            )
            return None

        utils.ui.echo(
            get_string("apatch_kptools_step").format(
                action="Repacking", file=img_name, name=display_name
            )
        )
        cmd_repack = [str(kptools_exe), "repack", img_name]
        res_repack = subprocess.run(
            cmd_repack, cwd=work_dir, capture_output=True, text=True
        )
        if res_repack.stdout:
            utils.ui.echo(res_repack.stdout.strip())

        if res_repack.returncode != 0:
            utils.ui.error(
                get_string("apatch_kptools_failed").format(
                    action="repack", error=res_repack.stderr, name=display_name
                )
            )
            return None

        patched_file = work_dir / "new-boot.img"
        if not patched_file.exists():
            utils.ui.error(get_string("apatch_repack_output_missing"))
            return None

        shutil.move(patched_file, patched_boot_path)
        utils.ui.echo(get_string("apatch_repack_success").format(name=display_name))
        return patched_boot_path

    elif gki:
        print(get_string("img_root_step1").format(name=img_name))
        mb.run("unpack", img_name, cwd=work_dir)
        if not (work_dir / "kernel").exists():
            print(get_string("img_root_unpack_fail"))
            return None
        print(get_string("img_root_unpack_ok"))

        print(get_string("img_root_step2"))
        target_kernel_version = get_kernel_version(work_dir / "kernel")

        if not target_kernel_version:
            print(get_string("img_root_kernel_ver_fail"))
            return None

        if not re.match(r"\d+\.\d+\.\d+", target_kernel_version):
            print(
                get_string("img_root_kernel_invalid").format(ver=target_kernel_version)
            )
            return None

        print(get_string("img_root_target_ver").format(ver=target_kernel_version))

        if custom_kernel_zip is None:
            print(get_string("gki_custom_cancelled"))
            return None

        kernel_image_path = downloader.extract_kernel_from_anykernel3_zip(
            custom_kernel_zip, work_dir
        )

        print(get_string("img_root_step5"))
        shutil.move(str(kernel_image_path), work_dir / "kernel")
        print(get_string("img_root_kernel_replaced"))

        print(get_string("img_root_step6").format(name=img_name))
        mb.run("repack", img_name, cwd=work_dir)
        if not (work_dir / "new-boot.img").exists():
            print(get_string("img_root_repack_fail"))
            return None
        shutil.move(work_dir / "new-boot.img", patched_boot_path)
        print(get_string("img_root_repack_ok"))

        return patched_boot_path

    else:
        print(get_string("img_root_step1").format(name="init_boot"))
        mb.run("unpack", img_name, cwd=work_dir)
        if not (work_dir / "ramdisk.cpio").exists():
            print(get_string("img_root_unpack_fail"))
            return None
        print(get_string("img_root_unpack_ok"))

        if not skip_lkm_download:
            try:
                print(get_string("img_root_lkm_download"))
                ksuinit_path = work_dir / "init"
                kmod_path = work_dir / "kernelsu.ko"

                if provider.provider_id == "sukisu":
                    if not lkm_kernel_version:
                        print(get_string("img_root_lkm_no_dev"), file=sys.stderr)
                        return None

                    downloader.download_nightly_artifacts(
                        repo=const.SUKISU_REPO,
                        workflow_id=const.SUKISU_WORKFLOW,
                        manager_name="Spoofed-Manager.zip",
                        mapped_name=lkm_kernel_version,
                        target_dir=work_dir,
                    )
                else:
                    downloader.download_ksuinit_release(ksuinit_path)
                    if not lkm_kernel_version:
                        print(get_string("img_root_lkm_no_dev"), file=sys.stderr)
                        return None
                    downloader.get_lkm_kernel_release(kmod_path, lkm_kernel_version)

            except (ToolError, OSError) as e:
                print(
                    get_string("img_root_lkm_download_fail").format(e=e),
                    file=sys.stderr,
                )
                return None
        else:
            print(get_string("img_root_skip_download"))

        print(get_string("img_root_lkm_patch"))

        init_exists_proc = mb.run(
            "cpio",
            "ramdisk.cpio",
            "exists init",
            cwd=work_dir,
            check=False,
            capture=True,
        )

        if init_exists_proc.returncode == 0:
            print(get_string("img_root_lkm_backup_init"))
            mb.run("cpio", "ramdisk.cpio", "mv init init.real", cwd=work_dir)

        print(get_string("img_root_lkm_add_files"))
        mb.run("cpio", "ramdisk.cpio", "add 0755 init init", cwd=work_dir)
        mb.run("cpio", "ramdisk.cpio", "add 0755 kernelsu.ko kernelsu.ko", cwd=work_dir)

        print(get_string("img_root_step6").format(name="init_boot"))
        mb.run("repack", img_name, cwd=work_dir)
        if not (work_dir / "new-boot.img").exists():
            print(get_string("img_root_repack_fail"))
            return None
        shutil.move(work_dir / "new-boot.img", patched_boot_path)
        print(get_string("img_root_repack_ok"))

        return patched_boot_path


def patch_magisk_boot(
    work_dir: Path,
    magiskboot_exe: Path,
    dev: Optional[device.DeviceController] = None,
    preinit_device: str = "",
) -> Optional[Path]:
    """Patch init_boot.img with Magisk. Replicates boot_patch.sh logic."""

    img_name = const.FN_INIT_BOOT
    out_img_name = const.FN_INIT_BOOT_ROOT
    patched_boot_path = const.BASE_DIR / out_img_name

    work_img_path = work_dir / img_name
    if not work_img_path.exists():
        print(
            get_string("img_root_err_img_not_found").format(name=img_name),
            file=sys.stderr,
        )
        return None

    mb = utils.MagiskBootWrapper(magiskboot_exe)

    def reboot_system_and_abort() -> None:
        if dev is None or dev.skip_adb:
            return
        print(get_string("magisk_rebooting_system"))
        try:
            dev.adb.reboot("system")
        except DeviceCommandError as error:
            print(get_string("device_err_reboot").format(e=error), file=sys.stderr)

    # --- 1. Unpack ---
    print(get_string("img_root_step1").format(name="init_boot"))
    mb.run("unpack", img_name, cwd=work_dir)

    # --- 2. Find ramdisk ---
    ramdisk = "ramdisk.cpio"
    skip_backup = False
    if not (work_dir / ramdisk).exists():
        skip_backup = True

    # --- 3. Check ramdisk status ---
    sha1 = ""
    if (work_dir / ramdisk).exists():
        status_proc = mb.run(
            "cpio",
            ramdisk,
            "test",
            cwd=work_dir,
            check=False,
            capture=True,
        )
        status = status_proc.returncode

        if status == 0:
            # Stock boot image
            sha1_proc = mb.run(
                "sha1",
                img_name,
                cwd=work_dir,
                check=False,
                capture=True,
            )
            sha1 = sha1_proc.stdout.strip() if sha1_proc.stdout else ""
            shutil.copy(work_dir / ramdisk, work_dir / "ramdisk.cpio.orig")
        elif status == 1:
            print(get_string("magisk_already_patched_image"), file=sys.stderr)
            reboot_system_and_abort()
            return None
        elif status == 2:
            print(get_string("magisk_unsupported_patcher"), file=sys.stderr)
            return None
    else:
        print(get_string("magisk_ramdisk_not_found"), file=sys.stderr)
        return None

    # --- 4. Compress binaries ---
    print(get_string("magisk_compressing_binaries"))
    mb.run("compress=xz", "magisk", "magisk.xz", cwd=work_dir)
    mb.run("compress=xz", "stub.apk", "stub.xz", cwd=work_dir)
    mb.run("compress=xz", "init-ld", "init-ld.xz", cwd=work_dir)

    # --- 5. Create config ---
    print(get_string("magisk_creating_config"))
    keepverity = "true"
    keepforceencrypt = "true"
    if not preinit_device:
        preinit_device = _resolve_magisk_preinit_device(dev)
    config_lines = [
        f"KEEPVERITY={keepverity}",
        f"KEEPFORCEENCRYPT={keepforceencrypt}",
        "RECOVERYMODE=false",
        "VENDORBOOT=false",
    ]
    if preinit_device:
        config_lines.append(f"PREINITDEVICE={preinit_device}")
    if sha1:
        config_lines.append(f"SHA1={sha1}")
    (work_dir / "config").write_text("\n".join(config_lines) + "\n", newline="\n")

    # --- 6. Patch ramdisk ---
    print(get_string("magisk_patching_ramdisk"))
    cpio_cmds = [
        "add 0750 init magiskinit",
        "mkdir 0750 overlay.d",
        "mkdir 0750 overlay.d/sbin",
        "add 0644 overlay.d/sbin/magisk.xz magisk.xz",
        "add 0644 overlay.d/sbin/stub.xz stub.xz",
        "add 0644 overlay.d/sbin/init-ld.xz init-ld.xz",
        "patch",
    ]
    if not skip_backup:
        cpio_cmds.append("backup ramdisk.cpio.orig")
    cpio_cmds.extend(
        [
            "mkdir 000 .backup",
            "add 000 .backup/.magisk config",
        ]
    )
    patch_env = {
        **utils._get_tool_env(),
        "KEEPVERITY": keepverity,
        "KEEPFORCEENCRYPT": keepforceencrypt,
    }
    mb.run("cpio", ramdisk, *cpio_cmds, cwd=work_dir, env=patch_env)

    # --- 7. Cleanup temp files ---
    for f in ["ramdisk.cpio.orig", "config", "magisk.xz", "stub.xz", "init-ld.xz"]:
        p = work_dir / f
        if p.exists():
            p.unlink()

    # --- 8. Repack ---
    print(get_string("img_root_step6").format(name="init_boot"))
    mb.run("repack", img_name, cwd=work_dir)

    if not (work_dir / "new-boot.img").exists():
        print(get_string("img_root_repack_fail"))
        return None
    shutil.move(work_dir / "new-boot.img", patched_boot_path)
    print(get_string("magisk_patch_success"))

    return patched_boot_path


def get_kernel_version(file_path: Union[str, Path]) -> Optional[str]:
    kernel_file = Path(file_path)
    if not kernel_file.exists():
        print(
            get_string("img_kv_err_not_found").format(path=file_path), file=sys.stderr
        )
        return None

    try:
        content = kernel_file.read_bytes()
        potential_strings = re.findall(b"[ -~]{10,}", content)

        found_version = None
        for string_bytes in potential_strings:
            try:
                line = string_bytes.decode("ascii", errors="ignore")
                if "Linux version " in line:
                    base_version_match = re.search(r"(\d+\.\d+\.\d+)", line)
                    if base_version_match:
                        found_version = base_version_match.group(1)
                        print(
                            get_string("img_kv_found").format(line=line.strip()),
                            file=sys.stderr,
                        )
                        break
            except UnicodeDecodeError:
                continue

        if found_version:
            return found_version
        else:
            print(get_string("img_kv_err_parse"), file=sys.stderr)
            return None

    except (UnicodeDecodeError, OSError) as e:
        print(get_string("unexpected_error").format(e=e), file=sys.stderr)
        return None
