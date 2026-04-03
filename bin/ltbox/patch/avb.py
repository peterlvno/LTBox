import importlib.util
import re
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from types import ModuleType
from typing import Any, Dict, List, Optional

from .. import constants as const
from .. import utils
from ..i18n import get_string


@dataclass
class _ParsedAvbImage:
    path: Path
    partition_name: str
    footer: Any
    header: Any
    descriptors: List[Any]
    image_size: int
    public_key: bytes
    public_key_metadata: bytes


def _resolve_avbtool_source_path() -> Path:
    candidates = [
        const.AVBTOOL_PY,
        const.BASE_DIR.parent / "avb" / "avbtool.py",
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    raise FileNotFoundError(
        "Unable to locate avbtool.py source for preserved vbmeta rebuild."
    )


def _resolve_avbtool_openssl_binary(source_path: Path) -> Optional[str]:
    tool_dir = source_path.resolve().parent
    candidates = (
        "avb_openssl",
        "avb_openssl.exe",
        "openssl",
        "openssl.exe",
    )
    for candidate in candidates:
        candidate_path = tool_dir / candidate
        if candidate_path.exists():
            return str(candidate_path)
    return None


@lru_cache(maxsize=4)
def _load_avbtool_module(source_path: str) -> ModuleType:
    module_name = f"_ltbox_avbtool_{abs(hash(source_path))}"
    spec = importlib.util.spec_from_file_location(module_name, source_path)
    if spec is None or spec.loader is None:
        raise ImportError(f"Unable to load avbtool module from {source_path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _get_avbtool_module() -> ModuleType:
    source_path = _resolve_avbtool_source_path()
    module = _load_avbtool_module(str(source_path))
    openssl_binary = _resolve_avbtool_openssl_binary(source_path)
    if openssl_binary:
        setattr(module, "AVB_OPENSSL", openssl_binary)
        mldsa_cls = getattr(module, "MLDSAPublicKey", None)
        if mldsa_cls is not None:
            mldsa_cls._IS_SUPPORTED = None
    return module


def _close_image_handler(image_handler: Any) -> None:
    image_file = getattr(image_handler, "_image", None)
    if image_file is not None:
        image_file.close()


def _parse_avb_image(
    image_path: Path,
    partition_name: Optional[str] = None,
) -> _ParsedAvbImage:
    avb_module = _get_avbtool_module()
    avb = avb_module.Avb()
    image_handler = avb_module.ImageHandler(str(image_path), read_only=True)

    try:
        footer, header, descriptors, image_size = avb._parse_image(image_handler)
        vbmeta_offset = footer.vbmeta_offset if footer else 0
        aux_block_offset = (
            vbmeta_offset
            + avb_module.AvbVBMetaHeader.SIZE
            + header.authentication_data_block_size
        )

        public_key = b""
        if header.public_key_size:
            image_handler.seek(aux_block_offset + header.public_key_offset)
            public_key = image_handler.read(header.public_key_size)

        public_key_metadata = b""
        if header.public_key_metadata_size:
            image_handler.seek(aux_block_offset + header.public_key_metadata_offset)
            public_key_metadata = image_handler.read(header.public_key_metadata_size)

        resolved_partition_name = partition_name or image_path.stem
        partition_descriptors = [
            descriptor
            for descriptor in descriptors
            if isinstance(
                descriptor,
                (avb_module.AvbHashDescriptor, avb_module.AvbHashtreeDescriptor),
            )
        ]
        if len(partition_descriptors) == 1:
            resolved_partition_name = partition_descriptors[0].partition_name

        return _ParsedAvbImage(
            path=image_path,
            partition_name=resolved_partition_name,
            footer=footer,
            header=header,
            descriptors=descriptors,
            image_size=image_size,
            public_key=public_key,
            public_key_metadata=public_key_metadata,
        )
    finally:
        _close_image_handler(image_handler)


def _build_chain_partition_descriptor(
    avb_module: ModuleType,
    original_descriptor: Any,
    public_key: bytes,
) -> Any:
    descriptor = avb_module.AvbChainPartitionDescriptor()
    descriptor.rollback_index_location = original_descriptor.rollback_index_location
    descriptor.partition_name = original_descriptor.partition_name
    descriptor.public_key = public_key
    descriptor.flags = original_descriptor.flags
    return descriptor


def _select_partition_descriptor(
    avb_module: ModuleType,
    parsed_image: _ParsedAvbImage,
) -> Any:
    partition_descriptors = [
        descriptor
        for descriptor in parsed_image.descriptors
        if isinstance(
            descriptor,
            (avb_module.AvbHashDescriptor, avb_module.AvbHashtreeDescriptor),
        )
    ]
    if not partition_descriptors:
        raise ValueError(
            f"{parsed_image.path.name} does not contain a hash or hashtree descriptor."
        )
    if len(partition_descriptors) == 1:
        return partition_descriptors[0]

    for descriptor in partition_descriptors:
        if descriptor.partition_name == parsed_image.partition_name:
            return descriptor

    raise ValueError(
        f"Unable to determine replacement descriptor for {parsed_image.path.name}."
    )


def _replace_vbmeta_descriptors(
    avb_module: ModuleType,
    original_descriptors: List[Any],
    parsed_images: List[_ParsedAvbImage],
) -> int:
    required_minor = 0
    hash_descriptor_types = (
        avb_module.AvbHashDescriptor,
        avb_module.AvbHashtreeDescriptor,
    )

    for parsed_image in parsed_images:
        chain_indexes = [
            index
            for index, descriptor in enumerate(original_descriptors)
            if isinstance(descriptor, avb_module.AvbChainPartitionDescriptor)
            and descriptor.partition_name == parsed_image.partition_name
        ]

        if len(chain_indexes) > 1:
            raise ValueError(
                f"Multiple chain descriptors found for {parsed_image.partition_name}."
            )
        if chain_indexes:
            if not parsed_image.public_key:
                raise ValueError(
                    f"{parsed_image.path.name} does not expose a public key for chain replacement."
                )
            descriptor_index = chain_indexes[0]
            original_descriptors[descriptor_index] = _build_chain_partition_descriptor(
                avb_module,
                original_descriptors[descriptor_index],
                parsed_image.public_key,
            )
            required_minor = max(
                required_minor,
                int(parsed_image.header.required_libavb_version_minor),
            )
            continue

        replacement_descriptor = _select_partition_descriptor(avb_module, parsed_image)
        matching_indexes = [
            index
            for index, descriptor in enumerate(original_descriptors)
            if isinstance(descriptor, hash_descriptor_types)
            and getattr(descriptor, "partition_name", None)
            == replacement_descriptor.partition_name
        ]

        if len(matching_indexes) > 1:
            raise ValueError(
                f"Multiple hash descriptors found for {replacement_descriptor.partition_name}."
            )
        if not matching_indexes:
            continue

        original_descriptors[matching_indexes[0]] = replacement_descriptor
        required_minor = max(
            required_minor,
            int(parsed_image.header.required_libavb_version_minor),
        )

    return required_minor


def _generate_preserved_vbmeta_blob(
    avb_module: ModuleType,
    original_image: _ParsedAvbImage,
    descriptors: List[Any],
    key_file: Path,
    algorithm: str,
    required_minor: int,
) -> bytes:
    avb = avb_module.Avb()
    metadata_temp_path: Optional[str] = None

    try:
        if original_image.public_key_metadata:
            metadata_file = tempfile.NamedTemporaryFile(delete=False)
            try:
                metadata_file.write(original_image.public_key_metadata)
                metadata_temp_path = metadata_file.name
            finally:
                metadata_file.close()

        return avb._generate_vbmeta_blob(
            algorithm,
            str(key_file),
            metadata_temp_path,
            descriptors,
            None,
            None,
            original_image.header.rollback_index,
            original_image.header.flags,
            original_image.header.rollback_index_location,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            original_image.header.release_string,
            None,
            max(
                int(original_image.header.required_libavb_version_minor),
                required_minor,
            ),
        )
    finally:
        if metadata_temp_path is not None:
            Path(metadata_temp_path).unlink(missing_ok=True)


def _write_preserved_vbmeta_blob(
    avb_module: ModuleType,
    original_image: _ParsedAvbImage,
    original_vbmeta_path: Path,
    output_path: Path,
    vbmeta_blob: bytes,
    padding_size: str,
) -> None:
    if original_image.footer is not None:
        shutil.copy2(original_vbmeta_path, output_path)
        image_handler = avb_module.ImageHandler(str(output_path))
        try:
            avb_module.Avb()._write_resigned_image(
                image_handler,
                original_image.footer,
                vbmeta_blob,
                True,
            )
        finally:
            _close_image_handler(image_handler)
        return

    pad_to = int(padding_size)
    padded_size = len(vbmeta_blob)
    if pad_to > 0:
        padded_size = avb_module.round_to_multiple(padded_size, pad_to)
    padded_size = max(padded_size, int(original_image.image_size))
    padding_needed = padded_size - len(vbmeta_blob)
    output_path.write_bytes(vbmeta_blob + (b"\0" * padding_needed))


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


def _require_info_keys(
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


def extract_image_avb_info(image_path: Path) -> Dict[str, Any]:
    avbtool = utils.AvbToolWrapper()
    info_proc = avbtool.run("info_image", "--image", image_path, capture=True)

    output = info_proc.stdout.strip()
    info: Dict[str, Any] = {}
    props_args: List[str] = []

    partition_size_match = re.search(
        r"^Image size:\s*(\d+)\s*bytes", output, re.MULTILINE
    )
    if partition_size_match:
        info["partition_size"] = partition_size_match.group(1)

    data_size_match = re.search(r"Original image size:\s*(\d+)\s*bytes", output)
    if data_size_match:
        info["data_size"] = data_size_match.group(1)
    else:
        desc_size_match = re.search(
            r"^\s*Image Size:\s*(\d+)\s*bytes", output, re.MULTILINE
        )
        if desc_size_match:
            info["data_size"] = desc_size_match.group(1)

    patterns = {
        "name": r"Partition Name:\s*(\S+)",
        "salt": r"Salt:\s*([0-9a-fA-F]+)",
        "algorithm": r"Algorithm:\s*(\S+)",
        "pubkey_sha1": r"Public key \(sha1\):\s*([0-9a-fA-F]+)",
    }

    header_section = output.split("Descriptors:")[0]
    rollback_match = re.search(r"Rollback Index:\s*(\d+)", header_section)
    if rollback_match:
        info["rollback"] = rollback_match.group(1)

    flags_match = re.search(r"Flags:\s*(\d+)", header_section)
    if flags_match:
        info["flags"] = flags_match.group(1)
        if output:
            utils.ui.info(get_string("img_info_flags").format(flags=info["flags"]))

    for key, pattern in patterns.items():
        if key not in info:
            match = re.search(pattern, output)
            if match:
                info[key] = match.group(1)

    for line in output.split("\n"):
        if line.strip().startswith("Prop:"):
            parts = line.split("->")
            if len(parts) < 2:
                continue
            key = parts[0].split(":")[-1].strip()
            val = parts[1].strip()[1:-1]
            info[key] = val
            props_args.extend(["--prop", f"{key}:{val}"])

    info["props_args"] = props_args
    if props_args and output:
        utils.ui.info(get_string("img_info_props").format(count=len(props_args) // 2))

    return info


def vbmeta_has_chain_partition(vbmeta_path: Path, partition_name: str) -> bool:
    avbtool = utils.AvbToolWrapper()
    info_proc = avbtool.run("info_image", "--image", vbmeta_path, capture=True)
    output = info_proc.stdout

    pattern = re.compile(
        r"Chain Partition descriptor:\s*\n\s*Partition Name:\s*(\S+)",
        re.MULTILINE,
    )
    descriptors = {m.group(1) for m in pattern.finditer(output)}
    return partition_name in descriptors


def _apply_avb_integrity_footer(
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

    avbtool = utils.AvbToolWrapper()
    apply_footer_cmd = [
        "add_hash_footer",
        "--image",
        image_path,
        "--algorithm",
        image_info["algorithm"],
        "--partition_size",
        image_info["partition_size"],
        "--partition_name",
        image_info["name"],
        "--rollback_index",
        str(rollback_index),
        "--salt",
        image_info["salt"],
        *image_info.get("props_args", []),
    ]

    if key_file:
        apply_footer_cmd.extend(["--key", key_file])

    if "flags" in image_info:
        apply_footer_cmd.extend(["--flags", image_info.get("flags", "0")])
        utils.ui.info(
            get_string("img_footer_restore_flags").format(
                flags=image_info.get("flags", "0")
            )
        )

    avbtool.run(*apply_footer_cmd)
    utils.ui.info(get_string("img_footer_success").format(name=image_path.name))


def resign_avb_image(
    image_path: Path,
    key_file: Path,
    algorithm: str,
    rollback_index: Optional[int] = None,
    auto_resize: bool = False,
) -> None:
    avbtool = utils.AvbToolWrapper()
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
    avbtool.run(*cmd)


def _resign_avb_image(
    image_path: Path,
    key_file: Path,
    algorithm: str,
    rollback_index: Optional[int] = None,
) -> None:
    resign_avb_image(
        image_path=image_path,
        key_file=key_file,
        algorithm=algorithm,
        rollback_index=rollback_index,
    )


def _update_vbmeta_partition_descriptor(
    output_path: Path,
    original_vbmeta_path: Path,
    partition_image: Path,
    key_file: Path,
    algorithm: str,
    rollback_index: str,
    flags: str,
) -> None:
    avbtool = utils.AvbToolWrapper()
    avbtool.run(
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

        _require_info_keys(
            info, ["partition_size", "name", "salt", "algorithm"], new_image_path
        )

        key_file = None
        if "pubkey_sha1" in info:
            key_file = const.KEY_MAP.get(str(info["pubkey_sha1"]))
            if not key_file:
                raise KeyError(
                    get_string("img_err_unknown_key").format(
                        key=info["pubkey_sha1"], name=new_image_path.name
                    )
                )

        shutil.copy(new_image_path, patched_image_path)

        if key_file and info["algorithm"] != "NONE":
            _resign_avb_image(
                image_path=patched_image_path,
                key_file=key_file,
                algorithm=info["algorithm"],
                rollback_index=current_rb_index,
            )
        else:
            _apply_avb_integrity_footer(
                image_path=patched_image_path,
                image_info=info,
                key_file=key_file,
                new_rollback_index=str(current_rb_index),
            )

    except (KeyError, subprocess.CalledProcessError, FileNotFoundError) as e:
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

        _require_info_keys(info, ["algorithm", "pubkey_sha1"], new_image_path)

        key_file = const.KEY_MAP.get(info["pubkey_sha1"])
        if not key_file:
            raise KeyError(
                get_string("img_err_unknown_key").format(
                    key=info["pubkey_sha1"], name=new_image_path.name
                )
            )

        shutil.copy(new_image_path, patched_image_path)
        _resign_avb_image(
            image_path=patched_image_path,
            key_file=key_file,
            algorithm=info["algorithm"],
            rollback_index=current_rb_index,
        )
        utils.ui.info(get_string("img_patch_success").format(name=image_name))

    except (KeyError, subprocess.CalledProcessError, FileNotFoundError) as e:
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

    _require_info_keys(
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

    _apply_avb_integrity_footer(
        image_path=image_to_process, image_info=boot_info, key_file=key_file
    )


def rebuild_vbmeta_preserving_descriptors(
    output_path: Path,
    original_vbmeta_path: Path,
    chained_images: List[Path],
    padding_size: str = "8192",
    key_file: Optional[Path] = None,
    algorithm: Optional[str] = None,
) -> None:
    utils.ui.info(get_string("act_remake_vbmeta"))
    if not chained_images:
        shutil.copy2(original_vbmeta_path, output_path)
        return

    vbmeta_info = extract_image_avb_info(original_vbmeta_path)
    resolved_key_file = key_file
    if resolved_key_file is None:
        vbmeta_pubkey = vbmeta_info.get("pubkey_sha1")
        resolved_key_file = const.KEY_MAP.get(str(vbmeta_pubkey))

        utils.ui.info(get_string("act_verify_vbmeta_key"))
        if not resolved_key_file:
            utils.ui.info(
                get_string("act_err_vbmeta_key_mismatch").format(key=vbmeta_pubkey)
            )
            raise KeyError(get_string("act_err_unknown_key").format(key=vbmeta_pubkey))
        utils.ui.info(get_string("img_key_matched").format(name=resolved_key_file.name))

    resolved_algorithm = algorithm or vbmeta_info["algorithm"]
    utils.ui.info(get_string("act_remaking_vbmeta"))

    avb_module = _get_avbtool_module()
    original_image = _parse_avb_image(
        original_vbmeta_path,
        partition_name=original_vbmeta_path.stem,
    )
    parsed_images = [_parse_avb_image(image_path) for image_path in chained_images]
    descriptors = list(original_image.descriptors)
    required_minor = _replace_vbmeta_descriptors(
        avb_module,
        descriptors,
        parsed_images,
    )
    vbmeta_blob = _generate_preserved_vbmeta_blob(
        avb_module,
        original_image,
        descriptors,
        resolved_key_file,
        resolved_algorithm,
        required_minor,
    )
    _write_preserved_vbmeta_blob(
        avb_module,
        original_image,
        original_vbmeta_path,
        output_path,
        vbmeta_blob,
        padding_size,
    )


def rebuild_vbmeta_with_chained_images(
    output_path: Path,
    original_vbmeta_path: Path,
    chained_images: List[Path],
    padding_size: str = "8192",
    key_file: Optional[Path] = None,
    algorithm: Optional[str] = None,
) -> None:
    utils.ui.info(get_string("act_remake_vbmeta"))
    vbmeta_info = extract_image_avb_info(original_vbmeta_path)
    resolved_key_file = key_file
    if resolved_key_file is None:
        vbmeta_pubkey = vbmeta_info.get("pubkey_sha1")
        resolved_key_file = const.KEY_MAP.get(str(vbmeta_pubkey))

        utils.ui.info(get_string("act_verify_vbmeta_key"))
        if not resolved_key_file:
            utils.ui.info(
                get_string("act_err_vbmeta_key_mismatch").format(key=vbmeta_pubkey)
            )
            raise KeyError(get_string("act_err_unknown_key").format(key=vbmeta_pubkey))
        utils.ui.info(get_string("img_key_matched").format(name=resolved_key_file.name))

    resolved_algorithm = algorithm or vbmeta_info["algorithm"]

    utils.ui.info(get_string("act_remaking_vbmeta"))

    if len(chained_images) == 1:
        try:
            _update_vbmeta_partition_descriptor(
                output_path=output_path,
                original_vbmeta_path=original_vbmeta_path,
                partition_image=chained_images[0],
                key_file=resolved_key_file,
                algorithm=resolved_algorithm,
                rollback_index=vbmeta_info.get("rollback", "0"),
                flags=vbmeta_info.get("flags", "0"),
            )
            return
        except subprocess.CalledProcessError:
            pass

    avbtool = utils.AvbToolWrapper()
    cmd = [
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
        vbmeta_info.get("flags", "0"),
        "--rollback_index",
        vbmeta_info.get("rollback", "0"),
        "--include_descriptors_from_image",
        original_vbmeta_path,
    ]

    for img in chained_images:
        cmd.extend(["--include_descriptors_from_image", img])

    avbtool.run(*cmd)
