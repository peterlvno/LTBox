import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from ltbox import constants as const
from ltbox.actions import edl
from ltbox.actions import region
from ltbox.actions import xml as xml_action
from ltbox.menus import data as menu_data
from ltbox.patch import avb as avb_patch
from ltbox.actions.root.strategies import GkiRootStrategy
from ltbox.patch.avb import (
    patch_chained_image_rollback,
    patch_vbmeta_image_rollback,
    vbmeta_has_chain_partition,
)


def create_xmls(img_dir, names):
    for n in names:
        (img_dir / n).touch()


def test_xml_select(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    files = [
        "rawprogram0.xml",
        "rawprogram1.xml",
        "rawprogram_unsparse0.xml",
        "rawprogram_save_persist_unsparse0.xml",
        "rawprogram_WIPE_PARTITIONS.xml",
        "patch0.xml",
    ]
    create_xmls(img_dir, files)

    with patch("ltbox.actions.edl.utils.ui"):
        raw, patch_files = edl._select_flash_xmls(skip_dp=False)

    r_names = [p.name for p in raw]
    p_names = [p.name for p in patch_files]

    assert "rawprogram_WIPE_PARTITIONS.xml" not in r_names
    assert "rawprogram0.xml" not in r_names
    assert "rawprogram1.xml" in r_names
    assert "rawprogram_save_persist_unsparse0.xml" in r_names
    assert "rawprogram_unsparse0.xml" not in r_names
    assert "patch0.xml" in p_names


def test_xml_select_prefers_ota_keep_data_xml(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    files = [
        "rawprogram1.xml",
        "rawprogram_save_persist_unsparse0.xml",
        "rawprogram_save_persist_ota_unsparse0.xml",
        "patch0.xml",
    ]
    create_xmls(img_dir, files)

    with patch("ltbox.actions.edl.utils.ui"):
        raw, _patch_files = edl._select_flash_xmls(skip_dp=False)

    r_names = [p.name for p in raw]

    assert "rawprogram_save_persist_ota_unsparse0.xml" in r_names
    assert "rawprogram_save_persist_unsparse0.xml" not in r_names


def test_replace_vbmeta_descriptors_keeps_root_chain_descriptors_intact():
    class FakeChainDescriptor:
        def __init__(
            self,
            partition_name="",
            rollback_index_location=0,
            public_key=b"",
            flags=0,
        ):
            self.partition_name = partition_name
            self.rollback_index_location = rollback_index_location
            self.public_key = public_key
            self.flags = flags

    class FakeHashDescriptor:
        def __init__(self, partition_name=""):
            self.partition_name = partition_name

    class FakeHashtreeDescriptor:
        def __init__(self, partition_name=""):
            self.partition_name = partition_name

    class FakeModule:
        AvbChainPartitionDescriptor = FakeChainDescriptor
        AvbHashDescriptor = FakeHashDescriptor
        AvbHashtreeDescriptor = FakeHashtreeDescriptor

    original_descriptors = [
        FakeChainDescriptor("boot", 3, b"boot-old", 0),
        FakeChainDescriptor("recovery", 1, b"recovery-old", 0),
        FakeChainDescriptor("vbmeta_system", 2, b"vbmeta-system-old", 0),
        FakeHashDescriptor("dtbo"),
        FakeHashtreeDescriptor("vendor"),
    ]

    replacement_images = [
        avb_patch._ParsedAvbImage(
            path=Path("boot.img"),
            partition_name="boot",
            footer=None,
            header=MagicMock(required_libavb_version_minor=0),
            descriptors=[FakeHashDescriptor("boot")],
            image_size=0,
            public_key=b"boot-new",
            public_key_metadata=b"",
        ),
        avb_patch._ParsedAvbImage(
            path=Path("vbmeta_system.img"),
            partition_name="vbmeta_system",
            footer=None,
            header=MagicMock(required_libavb_version_minor=0),
            descriptors=[
                FakeHashDescriptor("pvmfw"),
                FakeHashtreeDescriptor("product"),
                FakeHashtreeDescriptor("system"),
                FakeHashtreeDescriptor("system_ext"),
            ],
            image_size=0,
            public_key=b"vbmeta-system-new",
            public_key_metadata=b"",
        ),
    ]

    required_minor = avb_patch._replace_vbmeta_descriptors(
        FakeModule,
        original_descriptors,
        replacement_images,
    )

    assert required_minor == 0
    assert len(original_descriptors) == 5
    assert isinstance(original_descriptors[0], FakeChainDescriptor)
    assert original_descriptors[0].public_key == b"boot-new"
    assert isinstance(original_descriptors[2], FakeChainDescriptor)
    assert original_descriptors[2].public_key == b"vbmeta-system-new"
    assert not any(
        getattr(descriptor, "partition_name", None)
        in {"pvmfw", "product", "system", "system_ext"}
        for descriptor in original_descriptors
    )


def test_resolve_avbtool_openssl_binary_prefers_local_tool(tmp_path):
    tool_dir = tmp_path / "tools"
    tool_dir.mkdir()
    source_path = tool_dir / "avbtool.py"
    source_path.write_text("# stub", encoding="utf-8")
    openssl_path = tool_dir / "openssl.exe"
    openssl_path.write_text("stub", encoding="utf-8")

    assert avb_patch._resolve_avbtool_openssl_binary(source_path) == str(openssl_path)


def test_flash_args(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    files = ["rawprogram1.xml", "rawprogram_unsparse0.xml", "patch0.xml"]
    create_xmls(img_dir, files)

    mock_dev = MagicMock()

    with (
        patch("ltbox.actions.edl.utils.ui") as mock_ui,
        patch("ltbox.actions.edl.ensure_loader_file"),
        patch("ltbox.actions.edl._prepare_flash_files"),
        patch("builtins.input", return_value="y"),
    ):
        mock_ui.get_term_width.return_value = 80
        mock_ui.prompt.return_value = "y"

        edl.flash_full_firmware(
            mock_dev,
            skip_reset=True,
            skip_reset_edl=False,
            wipe=False,
        )

        args, kwargs = mock_dev.edl.flash_rawprogram.call_args
        passed = [p.name for p in args[3]]

        assert "rawprogram_unsparse0.xml" in passed
        assert len(passed) == 2
        assert kwargs["pre_erase"] is False
        assert kwargs["reset_after"] is False


def test_flash_full_firmware_wipe_requests_pre_erase_and_inline_reset(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    files = ["rawprogram1.xml", "rawprogram_unsparse0.xml", "patch0.xml"]
    create_xmls(img_dir, files)

    mock_dev = MagicMock()

    with (
        patch("ltbox.actions.edl.utils.ui"),
        patch("ltbox.actions.edl.ensure_loader_file"),
        patch("ltbox.actions.edl._prepare_flash_files"),
    ):
        edl.flash_full_firmware(
            mock_dev,
            skip_reset=False,
            skip_reset_edl=True,
            wipe=True,
        )

        _, kwargs = mock_dev.edl.flash_rawprogram.call_args
        assert kwargs["pre_erase"] is True
        assert kwargs["reset_after"] is True


def test_flash_full_firmware_prompts_for_manual_mode_when_unspecified(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    files = ["rawprogram1.xml", "rawprogram_unsparse0.xml", "patch0.xml"]
    create_xmls(img_dir, files)

    mock_dev = MagicMock()

    with (
        patch("ltbox.actions.edl.utils.ui") as mock_ui,
        patch("ltbox.actions.edl.ensure_loader_file"),
        patch("ltbox.actions.edl._prepare_flash_files"),
    ):
        mock_ui.get_term_width.return_value = 80
        mock_ui.prompt.return_value = "2"

        edl.flash_full_firmware(mock_dev, skip_reset=True, skip_reset_edl=True)

        _, kwargs = mock_dev.edl.flash_rawprogram.call_args
        assert kwargs["pre_erase"] is False
        assert kwargs["reset_after"] is False


def test_dump_partitions_does_not_abort_when_devinfo_persist_are_not_targets(tmp_path):
    mock_dev = MagicMock()
    mock_dev.edl_session.return_value.__enter__.return_value = "COM1"

    with (
        patch("ltbox.actions.edl.utils.ui"),
        patch("ltbox.actions.edl.ensure_edl_requirements"),
        patch(
            "ltbox.actions.edl.require_partition_params",
            side_effect=ValueError("missing"),
        ),
        patch("ltbox.actions.edl.time.sleep"),
        patch("ltbox.actions.edl.const.BACKUP_DIR", tmp_path),
    ):
        edl.dump_partitions(
            mock_dev,
            default_targets=False,
            additional_targets=["boot"],
            skip_reset=True,
        )


def test_dump_partitions_aborts_when_devinfo_dump_fails(tmp_path):
    mock_dev = MagicMock()
    mock_dev.edl_session.return_value.__enter__.return_value = "COM1"

    with (
        patch("ltbox.actions.edl.utils.ui"),
        patch("ltbox.actions.edl.ensure_edl_requirements"),
        patch(
            "ltbox.actions.edl.require_partition_params",
            side_effect=[
                ValueError("devinfo missing"),
                {
                    "source_xml": "rawprogram0.xml",
                    "lun": "0",
                    "start_sector": "1",
                    "num_sectors": "1",
                },
            ],
        ),
        patch("ltbox.actions.edl.time.sleep"),
        patch("ltbox.actions.edl.const.BACKUP_DIR", tmp_path),
    ):
        with pytest.raises(RuntimeError, match="devinfo"):
            edl.dump_partitions(mock_dev, default_targets=True, skip_reset=True)


def test_xml_fallback(mock_env):
    out_dir = mock_env["OUTPUT_XML_DIR"]
    target = out_dir / "rawprogram_save_persist_unsparse0.xml"

    cases = [
        (
            ["rawprogram_unsparse0.xml", "rawprogram0.xml"],
            "rawprogram_unsparse0.xml",
            "A",
        ),
        (["rawprogram0.xml"], "rawprogram0.xml", "B"),
    ]
    tmpl = """<xml version=\"1.0\" ><data><program label=\"{m}\" filename=\"\"/></data></xml>"""

    for fnames, expected, marker in cases:
        if target.exists():
            target.unlink()
        for f in out_dir.glob("*.xml"):
            f.unlink()

        for fn in fnames:
            m = marker if fn == expected else "X"
            (out_dir / fn).write_text(tmpl.format(m=m))

        with patch("ltbox.actions.xml.utils.ui"):
            xml_action._ensure_rawprogram_save_persist(out_dir)

        assert target.exists()
        root = ET.parse(target).getroot()
        assert root.find(".//program").get("label") == marker


def test_gki_strategy_requires_vbmeta_file():
    strategy = GkiRootStrategy()

    assert const.FN_BOOT in strategy.required_files
    assert const.FN_VBMETA in strategy.required_files


def test_vbmeta_has_chain_partition_parses_descriptor(tmp_path):
    vbmeta_img = tmp_path / "vbmeta.img"
    vbmeta_img.write_bytes(b"dummy")

    mock_chain_boot = MagicMock()
    mock_chain_boot.partition_name = "boot"
    mock_chain_recovery = MagicMock()
    mock_chain_recovery.partition_name = "recovery"

    mock_parsed = MagicMock()
    mock_parsed.descriptors = [mock_chain_boot, mock_chain_recovery]

    mock_avb_module = MagicMock()
    mock_avb_module.AvbChainPartitionDescriptor = type(mock_chain_boot)

    with (
        patch("ltbox.patch.avb._parse_avb_image", return_value=mock_parsed),
        patch("ltbox.patch.avb._get_avbtool_module", return_value=mock_avb_module),
    ):
        assert vbmeta_has_chain_partition(vbmeta_img, "boot") is True
        assert vbmeta_has_chain_partition(vbmeta_img, "init_boot") is False


def test_patch_vbmeta_image_rollback_resigns_copied_image(tmp_path):
    source = tmp_path / "vbmeta_system.img"
    source.write_bytes(b"vbmeta-data")
    patched = tmp_path / "vbmeta_system_patched.img"
    key_file = tmp_path / "testkey.pem"
    key_file.write_text("key", encoding="utf-8")

    info = {
        "algorithm": "SHA256_RSA2048",
        "pubkey_sha1": "known-key",
        "rollback": "10",
    }

    with (
        patch("ltbox.patch.avb.extract_image_avb_info", return_value=info),
        patch("ltbox.patch.avb.const.KEY_MAP", {"known-key": key_file}),
        patch("ltbox.patch.avb._run_avbtool") as mock_run,
    ):
        patch_vbmeta_image_rollback("vbmeta_system.img", 11, source, patched)

    assert patched.exists()
    assert patched.read_bytes() == source.read_bytes()
    mock_run.assert_called_once_with(
        "resign_image",
        "--image",
        patched,
        "--key",
        key_file,
        "--algorithm",
        "SHA256_RSA2048",
        "--rollback_index",
        11,
    )


def test_patch_chained_image_rollback_resigns_signed_image(tmp_path):
    source = tmp_path / "boot.img"
    source.write_bytes(b"boot-data")
    patched = tmp_path / "boot_patched.img"
    key_file = tmp_path / "bootkey.pem"
    key_file.write_text("key", encoding="utf-8")

    info = {
        "algorithm": "SHA256_RSA4096",
        "pubkey_sha1": "boot-key",
        "rollback": "20",
        "partition_size": "4096",
        "name": "boot",
        "salt": "abcd",
    }

    with (
        patch("ltbox.patch.avb.extract_image_avb_info", return_value=info),
        patch("ltbox.patch.avb.const.KEY_MAP", {"boot-key": key_file}),
        patch("ltbox.patch.avb._run_avbtool") as mock_run,
    ):
        patch_chained_image_rollback("boot.img", 21, source, patched)

    assert patched.exists()
    assert patched.read_bytes() == source.read_bytes()
    mock_run.assert_called_once_with(
        "resign_image",
        "--image",
        patched,
        "--key",
        key_file,
        "--algorithm",
        "SHA256_RSA4096",
        "--rollback_index",
        21,
    )


def test_process_boot_image_avb_erases_footer_before_adding(tmp_path):
    backup_dir = tmp_path / "backup"
    backup_dir.mkdir()
    (backup_dir / "boot.bak.img").write_bytes(b"boot-bak")
    target = tmp_path / "boot.img"
    target.write_bytes(b"boot-target")
    key_file = tmp_path / "bootkey.pem"
    key_file.write_text("key", encoding="utf-8")

    boot_info = {
        "partition_size": "4096",
        "name": "boot",
        "rollback": "20",
        "salt": "abcd",
        "algorithm": "SHA256_RSA4096",
        "pubkey_sha1": "boot-key",
    }

    with (
        patch("ltbox.patch.avb.extract_image_avb_info", return_value=boot_info),
        patch("ltbox.patch.avb.const.KEY_MAP", {"boot-key": key_file}),
        patch("ltbox.patch.avb.apply_avb_integrity_footer") as apply_footer,
        patch("ltbox.patch.avb._run_avbtool") as mock_run,
    ):
        from ltbox.patch.avb import process_boot_image_avb

        process_boot_image_avb(target, gki=True, backup_dir=backup_dir)

    mock_run.assert_called_once_with("erase_footer", "--image", target)
    apply_footer.assert_called_once_with(
        image_path=target, image_info=boot_info, key_file=key_file
    )


def test_rebuild_vbmeta_with_single_image_uses_descriptor_update(tmp_path):
    output_path = tmp_path / "vbmeta.out.img"
    original_vbmeta = tmp_path / "vbmeta.img"
    original_vbmeta.write_bytes(b"vbmeta")
    partition_image = tmp_path / "vendor_boot.img"
    partition_image.write_bytes(b"vendor_boot")
    key_file = tmp_path / "vbmeta.pem"
    key_file.write_text("key", encoding="utf-8")

    # sha1 of b"fake-public-key"
    import hashlib

    fake_pubkey = b"fake-public-key"
    pubkey_sha1 = hashlib.sha1(fake_pubkey).hexdigest()

    mock_header = MagicMock()
    mock_header.algorithm_type = 5
    mock_header.rollback_index = 0
    mock_header.flags = 0
    mock_parsed = MagicMock()
    mock_parsed.public_key = fake_pubkey
    mock_parsed.header = mock_header

    mock_avb_module = MagicMock()
    mock_avb_module.lookup_algorithm_by_type.return_value = ("SHA256_RSA4096", None)

    with (
        patch("ltbox.patch.avb._parse_avb_image", return_value=mock_parsed),
        patch("ltbox.patch.avb._get_avbtool_module", return_value=mock_avb_module),
        patch("ltbox.patch.avb.const.KEY_MAP", {pubkey_sha1: key_file}),
        patch("ltbox.patch.avb._run_avbtool") as mock_run,
    ):
        from ltbox.patch.avb import rebuild_vbmeta_with_chained_images

        rebuild_vbmeta_with_chained_images(
            output_path, original_vbmeta, [partition_image]
        )

    mock_run.assert_called_once_with(
        "update_partition_descriptor",
        "--image",
        original_vbmeta,
        "--partition_image",
        partition_image,
        "--output",
        output_path,
        "--key",
        key_file,
        "--algorithm",
        "SHA256_RSA4096",
        "--rollback_index",
        "0",
        "--flags",
        "0",
    )


def test_rebuild_vbmeta_with_multiple_images_falls_back_to_make_vbmeta(tmp_path):
    output_path = tmp_path / "vbmeta.out.img"
    original_vbmeta = tmp_path / "vbmeta.img"
    original_vbmeta.write_bytes(b"vbmeta")
    img1 = tmp_path / "vendor_boot.img"
    img2 = tmp_path / "dtbo.img"
    img1.write_bytes(b"vendor_boot")
    img2.write_bytes(b"dtbo")
    key_file = tmp_path / "vbmeta.pem"
    key_file.write_text("key", encoding="utf-8")

    import hashlib

    fake_pubkey = b"fake-public-key"
    pubkey_sha1 = hashlib.sha1(fake_pubkey).hexdigest()

    mock_header = MagicMock()
    mock_header.algorithm_type = 5
    mock_header.rollback_index = 0
    mock_header.flags = 0
    mock_parsed = MagicMock()
    mock_parsed.public_key = fake_pubkey
    mock_parsed.header = mock_header

    mock_avb_module = MagicMock()
    mock_avb_module.lookup_algorithm_by_type.return_value = ("SHA256_RSA4096", None)

    with (
        patch("ltbox.patch.avb._parse_avb_image", return_value=mock_parsed),
        patch("ltbox.patch.avb._get_avbtool_module", return_value=mock_avb_module),
        patch("ltbox.patch.avb.const.KEY_MAP", {pubkey_sha1: key_file}),
        patch("ltbox.patch.avb._run_avbtool") as mock_run,
    ):
        from ltbox.patch.avb import rebuild_vbmeta_with_chained_images

        rebuild_vbmeta_with_chained_images(output_path, original_vbmeta, [img1, img2])

    mock_run.assert_called_once_with(
        "make_vbmeta_image",
        "--output",
        output_path,
        "--key",
        key_file,
        "--algorithm",
        "SHA256_RSA4096",
        "--padding_size",
        "8192",
        "--flags",
        "0",
        "--rollback_index",
        "0",
        "--include_descriptors_from_image",
        original_vbmeta,
        "--include_descriptors_from_image",
        img1,
        "--include_descriptors_from_image",
        img2,
    )


def test_rebuild_vbmeta_with_override_key_skips_pubkey_validation(tmp_path):
    output_path = tmp_path / "vbmeta.out.img"
    original_vbmeta = tmp_path / "vbmeta.img"
    original_vbmeta.write_bytes(b"vbmeta")
    partition_image = tmp_path / "boot.img"
    partition_image.write_bytes(b"boot")
    key_file = tmp_path / "testkey_rsa4096.pem"
    key_file.write_text("key", encoding="utf-8")

    mock_header = MagicMock()
    mock_header.algorithm_type = 3
    mock_header.rollback_index = 7
    mock_header.flags = 0
    mock_parsed = MagicMock()
    mock_parsed.public_key = b"unknown-key"
    mock_parsed.header = mock_header

    mock_avb_module = MagicMock()
    mock_avb_module.lookup_algorithm_by_type.return_value = ("SHA256_RSA2048", None)

    with (
        patch("ltbox.patch.avb._parse_avb_image", return_value=mock_parsed),
        patch("ltbox.patch.avb._get_avbtool_module", return_value=mock_avb_module),
        patch("ltbox.patch.avb._run_avbtool") as mock_run,
    ):
        from ltbox.patch.avb import rebuild_vbmeta_with_chained_images

        rebuild_vbmeta_with_chained_images(
            output_path,
            original_vbmeta,
            [partition_image],
            key_file=key_file,
            algorithm="SHA256_RSA4096",
        )

    mock_run.assert_called_once_with(
        "update_partition_descriptor",
        "--image",
        original_vbmeta,
        "--partition_image",
        partition_image,
        "--output",
        output_path,
        "--key",
        key_file,
        "--algorithm",
        "SHA256_RSA4096",
        "--rollback_index",
        "7",
        "--flags",
        "0",
    )


def test_gki_finalize_patch_rebuilds_vbmeta_when_boot_chain_missing(tmp_path):
    strategy = GkiRootStrategy()
    patched_boot = tmp_path / "boot_patched.img"
    patched_boot.write_bytes(b"patched")
    output_dir = tmp_path / "output"
    output_dir.mkdir()

    backup_dir = tmp_path / "backup"
    backup_dir.mkdir()
    (backup_dir / const.FN_VBMETA_BAK).write_bytes(b"vbmeta")

    with (
        patch("ltbox.actions.root.strategies.process_boot_image_avb") as process_avb,
        patch(
            "ltbox.actions.root.strategies.vbmeta_has_chain_partition",
            return_value=False,
        ),
        patch(
            "ltbox.actions.root.strategies.rebuild_vbmeta_with_chained_images"
        ) as rebuild_vbmeta,
        patch("ltbox.actions.root.strategies.const.BASE_DIR", tmp_path),
    ):
        (tmp_path / const.FN_VBMETA_ROOT).write_bytes(b"new-vbmeta")

        final_boot = strategy.finalize_patch(patched_boot, output_dir, backup_dir)

    assert final_boot == output_dir / const.FN_BOOT
    assert final_boot.exists()
    process_avb.assert_called_once()
    rebuild_vbmeta.assert_called_once()
    assert (output_dir / const.FN_VBMETA).exists()


def test_gki_finalize_patch_skips_vbmeta_rebuild_when_boot_chain_exists(tmp_path):
    strategy = GkiRootStrategy()
    patched_boot = tmp_path / "boot_patched.img"
    patched_boot.write_bytes(b"patched")
    output_dir = tmp_path / "output"
    output_dir.mkdir()

    backup_dir = tmp_path / "backup"
    backup_dir.mkdir()
    (backup_dir / const.FN_VBMETA_BAK).write_bytes(b"vbmeta")

    with (
        patch("ltbox.actions.root.strategies.process_boot_image_avb") as process_avb,
        patch(
            "ltbox.actions.root.strategies.vbmeta_has_chain_partition",
            return_value=True,
        ),
        patch(
            "ltbox.actions.root.strategies.rebuild_vbmeta_with_chained_images"
        ) as rebuild_vbmeta,
    ):
        final_boot = strategy.finalize_patch(patched_boot, output_dir, backup_dir)

    assert final_boot == output_dir / const.FN_BOOT
    assert final_boot.exists()
    process_avb.assert_called_once()
    rebuild_vbmeta.assert_not_called()
    assert not (output_dir / const.FN_VBMETA).exists()


def test_advanced_menu_numbers_flash_options_after_xml_tools():
    menu_items = menu_data.get_advanced_menu_data("ROW")
    options = {item.key: item for item in menu_items if item.item_type == "option"}
    separators = [
        index for index, item in enumerate(menu_items) if item.item_type == "separator"
    ]

    assert options["11"].action == "flash_full_firmware"
    assert options["12"].action == "flash_selected_partitions"
    assert options["13"].action == "rebuild_vbmeta"
    assert options["14"].action == "sign_and_flash_recovery"

    option_10_index = next(
        index
        for index, item in enumerate(menu_items)
        if item.item_type == "option" and item.key == "10"
    )
    option_11_index = next(
        index
        for index, item in enumerate(menu_items)
        if item.item_type == "option" and item.key == "11"
    )
    assert any(
        option_10_index < separator_index < option_11_index
        for separator_index in separators
    )


def test_main_menu_no_longer_lists_incremental_ota():
    menu_items = menu_data.get_main_menu_data("ROW")
    options = {item.key: item for item in menu_items if item.item_type == "option"}

    assert {item.action for item in options.values()} >= {
        "disable_ota",
        "reenable_ota",
        "rescue_ota",
    }
    assert "apply_incremental_ota" not in {item.action for item in options.values()}
    assert options["3"].action == "disable_ota"
    assert options["4"].action == "reenable_ota"
    assert options["5"].action == "rescue_ota"


def test_rebuild_vbmeta_requires_vbmeta_and_one_image(tmp_path):
    image_dir = tmp_path / "image"
    output_dir = tmp_path / "output"
    image_dir.mkdir()

    with patch.multiple(
        "ltbox.actions.region.const",
        IMAGE_DIR=image_dir,
        OUTPUT_DIR=output_dir,
        FN_VBMETA="vbmeta.img",
        FN_INIT_BOOT="init_boot.img",
        FN_VENDOR_BOOT="vendor_boot.img",
    ):
        with pytest.raises(FileNotFoundError):
            region.rebuild_vbmeta(MagicMock())


def test_convert_region_images_skips_validation_and_avb_when_region_modify_disabled(
    mock_env, tmp_path
):
    image_dir = mock_env["IMAGE_DIR"]
    output_dir = mock_env["OUTPUT_DIR"]
    backup_dir = tmp_path / "backup"
    backup_dir.mkdir()

    (image_dir / const.FN_VENDOR_BOOT).write_bytes(b"vendor_boot")
    (image_dir / const.FN_VBMETA).write_bytes(b"vbmeta")

    mock_dev = MagicMock()
    mock_dev.skip_adb = False

    with (
        patch("ltbox.actions.region.const.BACKUP_DIR", backup_dir),
        patch("ltbox.actions.region.edit_vendor_boot") as edit_vendor_boot,
        patch("ltbox.actions.region.extract_image_avb_info") as extract_info,
        patch("ltbox.actions.region.apply_avb_integrity_footer") as apply_footer,
        patch(
            "ltbox.actions.region.rebuild_vbmeta_with_chained_images"
        ) as rebuild_vbmeta,
    ):
        region.convert_region_images(
            dev=mock_dev,
            device_model="TB322FC",
            target_region="PRC",
            modify_region_code=False,
            on_log=lambda _: None,
        )

    edit_vendor_boot.assert_not_called()
    extract_info.assert_not_called()
    apply_footer.assert_not_called()
    rebuild_vbmeta.assert_not_called()

    assert (output_dir / const.FN_VENDOR_BOOT).exists()
    assert (output_dir / const.FN_VBMETA).exists()


def test_edit_devinfo_prompt_is_highlighted_with_separator_and_color(tmp_path):
    backup_dir = tmp_path / "backup"
    image_dir = tmp_path / "image"
    output_dp_dir = tmp_path / "output_dp"
    base_dir = tmp_path / "base"
    backup_dir.mkdir()
    image_dir.mkdir()
    output_dp_dir.mkdir()
    base_dir.mkdir()

    (backup_dir / "devinfo.img").write_bytes(b"devinfo")
    (backup_dir / "persist.img").write_bytes(b"persist")

    logged_messages = []
    captured = {}

    def _on_log(msg):
        logged_messages.append(msg)

    def _on_confirm(msg):
        captured["prompt"] = msg
        return False

    with (
        patch.multiple(
            "ltbox.actions.region.const",
            BACKUP_DIR=backup_dir,
            IMAGE_DIR=image_dir,
            OUTPUT_DP_DIR=output_dp_dir,
            BASE_DIR=base_dir,
            FN_DEVINFO="devinfo.img",
            FN_PERSIST="persist.img",
        ),
        patch(
            "ltbox.actions.region.detect_country_codes",
            return_value={"devinfo.img": "ROW", "persist.img": "ROW"},
        ),
    ):
        region.edit_devinfo_persist(on_log=_on_log, on_confirm=_on_confirm)

    assert any("=" in message for message in logged_messages)
    assert "\033[93m" not in captured.get("prompt", "")
    assert captured.get("prompt") is not None


def test_edit_devinfo_persist_saves_serialno_txt(tmp_path):
    backup_dir = tmp_path / "backup"
    image_dir = tmp_path / "image"
    output_dp_dir = tmp_path / "output_dp"
    base_dir = tmp_path / "base"
    backup_dir.mkdir()
    image_dir.mkdir()
    output_dp_dir.mkdir()
    base_dir.mkdir()

    (backup_dir / "devinfo.img").write_bytes(b"devinfo")
    (backup_dir / "persist.img").write_bytes(b"persist")

    with (
        patch.multiple(
            "ltbox.actions.region.const",
            BACKUP_DIR=backup_dir,
            IMAGE_DIR=image_dir,
            OUTPUT_DP_DIR=output_dp_dir,
            BASE_DIR=base_dir,
            FN_DEVINFO="devinfo.img",
            FN_PERSIST="persist.img",
        ),
        patch(
            "ltbox.actions.region.detect_country_codes",
            return_value={"devinfo.img": "ROW", "persist.img": "ROW"},
        ),
    ):
        dir_name = region.edit_devinfo_persist(
            on_log=lambda _: None,
            on_confirm=lambda _: False,
            serialno="MX726W4T",
        )

    assert dir_name is not None
    backup_critical = base_dir / dir_name
    serialno_file = backup_critical / "serialno.txt"
    assert serialno_file.exists()
    assert serialno_file.read_text(encoding="utf-8") == "MX726W4T"


def test_edit_devinfo_persist_no_serialno_no_file(tmp_path):
    backup_dir = tmp_path / "backup"
    image_dir = tmp_path / "image"
    output_dp_dir = tmp_path / "output_dp"
    base_dir = tmp_path / "base"
    backup_dir.mkdir()
    image_dir.mkdir()
    output_dp_dir.mkdir()
    base_dir.mkdir()

    (backup_dir / "devinfo.img").write_bytes(b"devinfo")

    with (
        patch.multiple(
            "ltbox.actions.region.const",
            BACKUP_DIR=backup_dir,
            IMAGE_DIR=image_dir,
            OUTPUT_DP_DIR=output_dp_dir,
            BASE_DIR=base_dir,
            FN_DEVINFO="devinfo.img",
            FN_PERSIST="persist.img",
        ),
        patch(
            "ltbox.actions.region.detect_country_codes",
            return_value={"devinfo.img": "ROW"},
        ),
    ):
        dir_name = region.edit_devinfo_persist(
            on_log=lambda _: None,
            on_confirm=lambda _: False,
        )

    assert dir_name is not None
    backup_critical = base_dir / dir_name
    assert not (backup_critical / "serialno.txt").exists()
