import json
import shutil
import subprocess
from pathlib import Path
from typing import Any, Dict, List, Optional

from .. import constants as const
from .. import utils
from ..i18n import get_string


def _run_avbtool(*args: Any) -> str:
    """Run an avbtool-rs subcommand and return captured stdout."""
    cmd = [str(const.AVBTOOL_RS)] + [str(a) for a in args]
    result = subprocess.run(cmd, capture_output=True, text=True, check=True)
    return result.stdout


def _resolve_signing_key(pubkey_sha1: Optional[str], image_name: str) -> Optional[Path]:
    if not pubkey_sha1:
        return None
    key_file = const.KEY_MAP.get(str(pubkey_sha1))
    if not key_file:
        raise KeyError(
            get_string("img_err_unknown_key").format(key=pubkey_sha1, name=image_name)
        )
    return key_file


def _get_avb_info(image_path: Path) -> Dict[str, Any]:
    """Run info_image --format json and return parsed Avb entry."""
    raw = _run_avbtool("info_image", "--image", image_path, "--format", "json")
    data = json.loads(raw)
    return data[0]["result"]["Avb"]


def extract_image_avb_info(image_path: Path) -> Dict[str, Any]:
    avb = _get_avb_info(image_path)
    header = avb["header"]
    footer = avb.get("footer")

    info: Dict[str, Any] = {
        "partition_size": str(avb["vbmeta_offset"] + avb["vbmeta_size"])
        if footer
        else str(avb["vbmeta_size"]),
        "algorithm": avb["algorithm_name"],
        "rollback": str(header["rollback_index"]),
        "flags": str(header["flags"]),
    }

    if footer is not None:
        info["data_size"] = str(footer["original_image_size"])
        info["partition_size"] = str(
            footer["original_image_size"] + footer["vbmeta_size"]
        )

    pubkey_sha1 = avb.get("public_key_sha1")
    if pubkey_sha1:
        info["pubkey_sha1"] = pubkey_sha1

    if info["flags"] != "0":
        utils.ui.info(get_string("img_info_flags").format(flags=info["flags"]))

    props_args: List[str] = []
    for descriptor in avb["descriptors"]:
        if "Property" in descriptor:
            prop = descriptor["Property"]
            info[prop["key"]] = prop["value"]
            props_args.extend(["--prop", f"{prop['key']}:{prop['value']}"])
        elif "Hash" in descriptor:
            if "name" not in info:
                h = descriptor["Hash"]
                info["name"] = h["partition_name"]
                info["salt"] = h["salt"]
                if "data_size" not in info:
                    info["data_size"] = str(h["image_size"])
        elif "Hashtree" in descriptor:
            if "name" not in info:
                ht = descriptor["Hashtree"]
                info["name"] = ht["partition_name"]
                info["salt"] = ht["salt"]
                if "data_size" not in info:
                    info["data_size"] = str(ht["image_size"])

    info["props_args"] = props_args
    if props_args:
        utils.ui.info(get_string("img_info_props").format(count=len(props_args) // 2))

    return info


def vbmeta_has_chain_partition(vbmeta_path: Path, partition_name: str) -> bool:
    avb = _get_avb_info(vbmeta_path)
    return any(
        "ChainPartition" in d
        and d["ChainPartition"]["partition_name"] == partition_name
        for d in avb["descriptors"]
    )


def apply_avb_integrity_footer(
    image_path: Path,
    image_info: Dict[str, Any],
    key_file: Optional[Path],
    new_rollback_index: Optional[str] = None,
) -> None:
    rollback_index = (
        new_rollback_index if new_rollback_index is not None else image_info["rollback"]
    )

    utils.ui.info(get_string("img_footer_adding").format(name=image_path.name))
    utils.ui.info(
        get_string("img_footer_details").format(
            part=image_info["name"], rb=rollback_index
        )
    )

    image_size = image_path.stat().st_size
    partition_size = int(image_info["partition_size"])
    # AOSP avbtool requires partition_size to be block-aligned and reserves
    # MAX_VBMETA_SIZE (64 KiB) + MAX_FOOTER_SIZE (4 KiB) = 69632 bytes.
    # Fall back to --dynamic_partition_size when these constraints are not met.
    _BLOCK_SIZE = 4096
    _MAX_METADATA = 65536 + 4096
    use_dynamic = (
        partition_size % _BLOCK_SIZE != 0 or image_size + _MAX_METADATA > partition_size
    )

    cmd: List[Any] = [
        "add_hash_footer",
        "--image",
        image_path,
        "--algorithm",
        image_info["algorithm"],
        *(
            ["--dynamic_partition_size"]
            if use_dynamic
            else ["--partition_size", partition_size]
        ),
        "--partition_name",
        image_info["name"],
        "--rollback_index",
        str(rollback_index),
        "--salt",
        image_info["salt"],
        *image_info.get("props_args", []),
    ]

    if key_file:
        cmd.extend(["--key", key_file])

    if "flags" in image_info:
        cmd.extend(["--flags", image_info.get("flags", "0")])
        utils.ui.info(
            get_string("img_footer_restore_flags").format(
                flags=image_info.get("flags", "0")
            )
        )

    _run_avbtool(*cmd)
    utils.ui.info(get_string("img_footer_success").format(name=image_path.name))


def resign_avb_image(
    image_path: Path,
    key_file: Path,
    algorithm: str,
    rollback_index: Optional[int] = None,
    auto_resize: bool = False,
) -> None:
    cmd: List[Any] = [
        "resign_image",
        "--image",
        image_path,
        "--key",
        key_file,
        "--algorithm",
        algorithm,
    ]
    if auto_resize:
        cmd.append("--auto_resize")
    if rollback_index is not None:
        cmd.extend(["--rollback_index", rollback_index])
    _run_avbtool(*cmd)


def _update_vbmeta_partition_descriptor(
    output_path: Path,
    original_vbmeta_path: Path,
    partition_image: Path,
    key_file: Path,
    algorithm: str,
    rollback_index: str,
    flags: str,
) -> None:
    _run_avbtool(
        "update_partition_descriptor",
        "--image",
        original_vbmeta_path,
        "--partition_image",
        partition_image,
        "--output",
        output_path,
        "--key",
        key_file,
        "--algorithm",
        algorithm,
        "--rollback_index",
        rollback_index,
        "--flags",
        flags,
    )


def require_info_keys(
    info: Dict[str, Any],
    required_keys: List[str],
    image_path: Path,
    defaults: Optional[Dict[str, str]] = None,
) -> None:
    for key in required_keys:
        if key not in info:
            if key == "partition_size" and "data_size" in info:
                info["partition_size"] = info["data_size"]
            elif defaults and key in defaults:
                info[key] = defaults[key]
            else:
                raise KeyError(
                    get_string("img_err_missing_key").format(
                        key=key, name=image_path.name
                    )
                )


def _analyze_rollback_target(
    image_name: str,
    current_rb_index: int,
    new_image_path: Path,
    patched_image_path: Path,
) -> Optional[Dict[str, Any]]:
    utils.ui.info(get_string("img_analyze_new").format(name=image_name))
    info = extract_image_avb_info(new_image_path)
    new_rb_index = int(info.get("rollback", "0"))
    utils.ui.info(get_string("img_new_index").format(index=new_rb_index))

    if new_rb_index == current_rb_index:
        utils.ui.info(get_string("img_index_ok").format(name=image_name))
        shutil.copy(new_image_path, patched_image_path)
        return None

    utils.ui.info(
        get_string("img_patch_bypass").format(
            name=image_name, old=new_rb_index, new=current_rb_index
        )
    )

    return info


def patch_chained_image_rollback(
    image_name: str,
    current_rb_index: int,
    new_image_path: Path,
    patched_image_path: Path,
) -> None:
    try:
        info = _analyze_rollback_target(
            image_name, current_rb_index, new_image_path, patched_image_path
        )
        if info is None:
            return

        require_info_keys(
            info, ["partition_size", "name", "salt", "algorithm"], new_image_path
        )

        key_file = _resolve_signing_key(info.get("pubkey_sha1"), new_image_path.name)

        shutil.copy(new_image_path, patched_image_path)

        if key_file and info["algorithm"] != "NONE":
            resign_avb_image(
                image_path=patched_image_path,
                key_file=key_file,
                algorithm=info["algorithm"],
                rollback_index=current_rb_index,
            )
        else:
            apply_avb_integrity_footer(
                image_path=patched_image_path,
                image_info=info,
                key_file=key_file,
                new_rollback_index=str(current_rb_index),
            )

    except (KeyError, FileNotFoundError) as e:
        utils.ui.error(get_string("img_err_processing").format(name=image_name, e=e))
        raise


def patch_vbmeta_image_rollback(
    image_name: str,
    current_rb_index: int,
    new_image_path: Path,
    patched_image_path: Path,
) -> None:
    try:
        info = _analyze_rollback_target(
            image_name, current_rb_index, new_image_path, patched_image_path
        )
        if info is None:
            return

        require_info_keys(info, ["algorithm", "pubkey_sha1"], new_image_path)

        key_file = _resolve_signing_key(info["pubkey_sha1"], new_image_path.name)
        assert key_file is not None, (
            f"Resolved key_file cannot be None for {new_image_path.name}"
        )

        shutil.copy(new_image_path, patched_image_path)
        resign_avb_image(
            image_path=patched_image_path,
            key_file=key_file,
            algorithm=info["algorithm"],
            rollback_index=current_rb_index,
        )
        utils.ui.info(get_string("img_patch_success").format(name=image_name))

    except (KeyError, FileNotFoundError) as e:
        utils.ui.error(get_string("img_err_processing").format(name=image_name, e=e))
        raise


def process_boot_image_avb(
    image_to_process: Path,
    gki: bool = False,
    backup_dir: Optional[Path] = None,
) -> None:
    utils.ui.info(get_string("img_verify_boot"))

    bak_name = "boot.bak.img" if gki else "init_boot.bak.img"
    base_dir = backup_dir or const.BASE_DIR
    boot_bak_img = base_dir / bak_name

    if not boot_bak_img.exists():
        msg = get_string("img_err_boot_bak_missing").format(name=boot_bak_img.name)
        utils.ui.error(msg)
        raise FileNotFoundError(msg)

    utils.ui.info(get_string("img_avb_extract_info").format(name=boot_bak_img.name))
    boot_info = extract_image_avb_info(boot_bak_img)

    require_info_keys(
        boot_info,
        ["partition_size", "name", "rollback", "salt", "algorithm"],
        boot_bak_img,
    )

    key_file = None
    if gki:
        boot_pubkey = boot_info.get("pubkey_sha1")

        if boot_pubkey:
            key_file = const.KEY_MAP.get(boot_pubkey)

            if not key_file:
                utils.ui.error(
                    get_string("img_err_boot_key_mismatch").format(key=boot_pubkey)
                )
                raise KeyError(
                    get_string("img_err_boot_key_mismatch").format(key=boot_pubkey)
                )

            utils.ui.info(get_string("img_key_matched").format(name=key_file.name))
        else:
            utils.ui.info(get_string("img_warn_no_sig_key"))

    try:
        utils.ui.info(
            get_string("img_avb_erase_footer").format(name=image_to_process.name)
        )
        _run_avbtool("erase_footer", "--image", image_to_process)
        utils.ui.info(get_string("img_avb_erase_footer_ok"))
    except Exception as e:
        utils.ui.info(get_string("img_avb_erase_footer_fail").format(e=e))

    apply_avb_integrity_footer(
        image_path=image_to_process, image_info=boot_info, key_file=key_file
    )


def _resolve_vbmeta_key_and_algorithm(
    avb_info: Dict[str, Any],
    key_file: Optional[Path],
    algorithm: Optional[str],
) -> tuple[Path, str]:
    resolved_key_file = key_file
    if resolved_key_file is None:
        vbmeta_pubkey = avb_info.get("public_key_sha1")
        resolved_key_file = const.KEY_MAP.get(str(vbmeta_pubkey))

        utils.ui.info(get_string("act_verify_vbmeta_key"))
        if not resolved_key_file:
            utils.ui.info(
                get_string("act_err_vbmeta_key_mismatch").format(key=vbmeta_pubkey)
            )
            raise KeyError(get_string("act_err_unknown_key").format(key=vbmeta_pubkey))
        utils.ui.info(get_string("img_key_matched").format(name=resolved_key_file.name))

    resolved_algorithm = algorithm or avb_info["algorithm_name"]
    return resolved_key_file, resolved_algorithm


def rebuild_vbmeta_with_chained_images(
    output_path: Path,
    original_vbmeta_path: Path,
    chained_images: List[Path],
    padding_size: str = "8192",
    key_file: Optional[Path] = None,
    algorithm: Optional[str] = None,
) -> None:
    utils.ui.info(get_string("act_remake_vbmeta"))
    avb_info = _get_avb_info(original_vbmeta_path)

    resolved_key_file, resolved_algorithm = _resolve_vbmeta_key_and_algorithm(
        avb_info, key_file, algorithm
    )

    rollback_str = str(avb_info["header"]["rollback_index"])
    flags_str = str(avb_info["header"]["flags"])

    utils.ui.info(get_string("act_remaking_vbmeta"))

    if len(chained_images) == 1:
        try:
            _update_vbmeta_partition_descriptor(
                output_path=output_path,
                original_vbmeta_path=original_vbmeta_path,
                partition_image=chained_images[0],
                key_file=resolved_key_file,
                algorithm=resolved_algorithm,
                rollback_index=rollback_str,
                flags=flags_str,
            )
            return
        except Exception as e:
            utils.ui.warn(
                f"update_partition_descriptor failed for "
                f"{chained_images[0].name}, falling back to "
                f"make_vbmeta_image: {e}"
            )

    cmd: List[Any] = [
        "make_vbmeta_image",
        "--output",
        output_path,
        "--key",
        resolved_key_file,
        "--algorithm",
        resolved_algorithm,
        "--padding_size",
        padding_size,
        "--flags",
        flags_str,
        "--rollback_index",
        rollback_str,
        "--include_descriptors_from_image",
        original_vbmeta_path,
    ]

    for img in chained_images:
        cmd.extend(["--include_descriptors_from_image", img])

    _run_avbtool(*cmd)
