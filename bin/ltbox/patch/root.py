import re
import shutil
import sys
import subprocess
import lzma
from pathlib import Path
from typing import Optional, Union

from .. import constants as const
from .. import device, downloader, utils
from ..i18n import get_string


def _detect_preinit_device(
    dev: Optional[device.DeviceController],
) -> Optional[str]:
    if not dev or dev.skip_adb:
        return None

    try:
        output = dev.adb.shell("magisk --preinit-device").strip()
    except Exception:
        return None

    if not output:
        return None

    if "not found" in output.lower() or "no such file" in output.lower():
        return None

    if output.startswith("/dev/"):
        return output

    match = re.search(r"(/dev/[^\s]+)", output)
    return match.group(1) if match else None


def patch_boot_with_root_algo(
    work_dir: Path,
    magiskboot_exe: Path,
    dev: Optional[device.DeviceController] = None,
    gki: bool = False,
    lkm_kernel_version: Optional[str] = None,
    root_type: str = "ksu",
    skip_lkm_download: bool = False,
    superkey: Optional[str] = None,
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

    if root_type == "folkpatch":
        if superkey is None:
            utils.ui.error(get_string("folkpatch_err_superkey_required"))
            return None

        kptools_exe = const.DOWNLOAD_DIR / "kptools.exe"
        kpimg_file = work_dir / "kpimg"

        utils.ui.echo(get_string("folkpatch_unpack_start").format(name=img_name))

        cmd_unpack = [str(kptools_exe), "unpack", img_name]
        res_unpack = subprocess.run(
            cmd_unpack, cwd=work_dir, capture_output=True, text=True
        )
        if res_unpack.stdout:
            utils.ui.echo(res_unpack.stdout.strip())

        if res_unpack.returncode != 0:
            utils.ui.error(
                get_string("folkpatch_unpack_failed").format(error=res_unpack.stderr)
            )
            return None

        kernel_file = work_dir / "kernel"
        kernel_ori = work_dir / "kernel.ori"

        if not kernel_file.exists():
            utils.ui.error(get_string("folkpatch_unpack_kernel_missing"))
            return None

        shutil.move(kernel_file, kernel_ori)

        utils.ui.echo(get_string("folkpatch_check_kernel"))
        cmd_check = [str(kptools_exe), "-i", str(kernel_ori), "-f"]
        res_check = subprocess.run(
            cmd_check, cwd=work_dir, capture_output=True, text=True
        )

        if "CONFIG_KALLSYMS=y" not in res_check.stdout:
            utils.ui.error(get_string("folkpatch_err_kallsyms_required"))
            return None
        if "CONFIG_KALLSYMS_ALL=y" not in res_check.stdout:
            utils.ui.echo(get_string("folkpatch_warn_kallsyms_all"))

        utils.ui.echo(get_string("folkpatch_patch_start"))
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
        res_patch = subprocess.run(
            cmd_patch, cwd=work_dir, capture_output=True, text=True
        )
        if res_patch.stdout:
            utils.ui.echo(res_patch.stdout.strip())

        if res_patch.returncode != 0:
            utils.ui.error(
                get_string("folkpatch_patch_failed").format(error=res_patch.stderr)
            )
            return None

        utils.ui.echo(get_string("folkpatch_repack_start").format(name=img_name))
        cmd_repack = [str(kptools_exe), "repack", img_name]
        res_repack = subprocess.run(
            cmd_repack, cwd=work_dir, capture_output=True, text=True
        )
        if res_repack.stdout:
            utils.ui.echo(res_repack.stdout.strip())

        if res_repack.returncode != 0:
            utils.ui.error(
                get_string("folkpatch_repack_failed").format(error=res_repack.stderr)
            )
            return None

        patched_file = work_dir / "new-boot.img"
        if not patched_file.exists():
            utils.ui.error(get_string("folkpatch_repack_output_missing"))
            return None

        shutil.move(patched_file, patched_boot_path)
        utils.ui.echo(get_string("folkpatch_repack_success"))
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

        kernel_image_path = downloader.get_gki_kernel(target_kernel_version, work_dir)

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
        print(get_string("img_root_step1_init_boot").format(name=img_name))
        mb.run("unpack", img_name, cwd=work_dir)
        if not (work_dir / "ramdisk.cpio").exists():
            print(get_string("img_root_unpack_fail"))
            return None
        print(get_string("img_root_unpack_ok"))

        if not skip_lkm_download:
            try:
                if root_type == "magisk":
                    print(get_string("img_root_magisk_download"))
                    apk_path = downloader.download_magisk_apk(work_dir)
                    downloader.extract_magisk_libs(apk_path, work_dir)
                else:
                    print(get_string("img_root_lkm_download"))
                    ksuinit_path = work_dir / "init"
                    kmod_path = work_dir / "kernelsu.ko"

                    if root_type == "sukisu":
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

            except Exception as e:
                error_key = (
                    "img_root_magisk_download_fail"
                    if root_type == "magisk"
                    else "img_root_lkm_download_fail"
                )
                print(get_string(error_key).format(e=e), file=sys.stderr)
                return None
        else:
            print(get_string("img_root_skip_download"))

        if root_type == "magisk":
            print(get_string("img_root_magisk_patch"))
        else:
            print(get_string("img_root_lkm_patch"))

        init_exists_proc = mb.run(
            "cpio",
            "ramdisk.cpio",
            "exists init",
            cwd=work_dir,
            check=False,
            capture=True,
        )

        if init_exists_proc.returncode == 0 and root_type != "magisk":
            print(get_string("img_root_lkm_backup_init"))
            mb.run("cpio", "ramdisk.cpio", "mv init init.real", cwd=work_dir)

        if root_type == "magisk":
            required_files = [
                "magiskinit",
                "magisk",
                "init-ld",
                "stub.apk",
            ]
            missing_files = [
                name for name in required_files if not (work_dir / name).exists()
            ]
            if missing_files:
                print(
                    get_string("img_root_magisk_missing").format(
                        files=", ".join(missing_files)
                    ),
                    file=sys.stderr,
                )
                return None

            print(get_string("img_root_magisk_add_files"))
            config_path = work_dir / "config"
            config_entries = [
                "KEEPVERITY=true",
                "KEEPFORCEENCRYPT=true",
                "RECOVERYMODE=false",
            ]
            preinit_device = _detect_preinit_device(dev)
            if preinit_device:
                config_entries.append(f"PREINITDEVICE={preinit_device}")
            sha1_proc = mb.run(
                "sha1", img_name, cwd=work_dir, check=False, capture=True
            )
            if sha1_proc.returncode == 0:
                sha1 = sha1_proc.stdout.strip()
                if sha1:
                    config_entries.append(f"SHA1={sha1}")
            config_path.write_text(
                "\n".join(config_entries) + "\n",
                encoding="utf-8",
            )
            ramdisk_backup = work_dir / "ramdisk.cpio.orig"
            if not ramdisk_backup.exists():
                shutil.copy(work_dir / "ramdisk.cpio", ramdisk_backup)

            for fname in ["magisk", "stub.apk", "init-ld"]:
                src_path = work_dir / fname
                dst_path = work_dir / f"{fname}.xz"
                with (
                    open(src_path, "rb") as f_in,
                    lzma.open(dst_path, "wb", format=lzma.FORMAT_XZ) as f_out,
                ):
                    shutil.copyfileobj(f_in, f_out)

            mb.run(
                "cpio",
                "ramdisk.cpio",
                "add 0750 init magiskinit",
                "mkdir 0750 overlay.d",
                "mkdir 0750 overlay.d/sbin",
                "add 0644 overlay.d/sbin/magisk.xz magisk.xz",
                "add 0644 overlay.d/sbin/stub.xz stub.xz",
                "add 0644 overlay.d/sbin/init-ld.xz init-ld.xz",
                "patch",
                "backup ramdisk.cpio.orig",
                "mkdir 000 .backup",
                "add 000 .backup/.magisk config",
                cwd=work_dir,
            )
            for temp_name in [
                "ramdisk.cpio.orig",
                "config",
                "magisk.xz",
                "stub.xz",
                "init-ld.xz",
            ]:
                temp_path = work_dir / temp_name
                if temp_path.exists():
                    temp_path.unlink()
        else:
            print(get_string("img_root_lkm_add_files"))
            mb.run("cpio", "ramdisk.cpio", "add 0755 init init", cwd=work_dir)
            mb.run(
                "cpio", "ramdisk.cpio", "add 0755 kernelsu.ko kernelsu.ko", cwd=work_dir
            )

        print(get_string("img_root_step6_init_boot").format(name=img_name))
        mb.run("repack", img_name, cwd=work_dir)
        if not (work_dir / "new-boot.img").exists():
            print(get_string("img_root_repack_fail"))
            return None
        shutil.move(work_dir / "new-boot.img", patched_boot_path)
        print(get_string("img_root_repack_ok"))

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

    except Exception as e:
        print(get_string("unexpected_error").format(e=e), file=sys.stderr)
        return None
