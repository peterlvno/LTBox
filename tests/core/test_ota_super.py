import struct
import xml.etree.ElementTree as ET
from pathlib import Path

from ltbox.ota_super import (
    LP_HEADER_FLAG_VIRTUAL_AB_DEVICE,
    LP_METADATA_GEOMETRY_MAGIC,
    LP_METADATA_HEADER_MAGIC,
    LP_SECTOR_SIZE,
    build_lpmake_command,
    create_keep_data_ota_xml,
    extract_partition_images,
    parse_super_layout,
    split_rebuilt_super,
)
from ltbox.xml_catalog import PartitionRecord


def _build_test_super_layout(tmp_path: Path) -> tuple[list[PartitionRecord], Path]:
    chunk1 = tmp_path / "super_1.img"
    chunk2 = tmp_path / "super_2.img"

    chunk1_bytes = bytearray(10 * LP_SECTOR_SIZE)
    chunk2_bytes = bytearray(10 * LP_SECTOR_SIZE)

    struct.pack_into(
        "<II32sIII",
        chunk1_bytes,
        0,
        LP_METADATA_GEOMETRY_MAGIC,
        52,
        b"\x00" * 32,
        4096,
        3,
        4096,
    )

    partitions = [
        struct.pack("<36sIIII", b"system_a", 1, 0, 1, 0),
        struct.pack("<36sIIII", b"vendor_a", 1, 1, 1, 0),
        struct.pack("<36sIIII", b"system_b", 1, 2, 1, 1),
    ]
    extents = [
        struct.pack("<QIQI", 2, 0, 6, 0),
        struct.pack("<QIQI", 3, 0, 9, 0),
        struct.pack("<QIQI", 0, 1, 0, 0),
    ]
    groups = [
        struct.pack("<36sIQ", b"dynamic_a", 0, 8192),
        struct.pack("<36sIQ", b"dynamic_b", 0, 8192),
    ]
    block_devices = [
        struct.pack("<QIIQ36sI", 6, 4096, 0, 20 * LP_SECTOR_SIZE, b"super", 0),
    ]

    header_size = 256
    partitions_table = b"".join(partitions)
    extents_table = b"".join(extents)
    groups_table = b"".join(groups)
    block_devices_table = b"".join(block_devices)

    partitions_offset = 0
    extents_offset = partitions_offset + len(partitions_table)
    groups_offset = extents_offset + len(extents_table)
    block_devices_offset = groups_offset + len(groups_table)
    tables_size = (
        len(partitions_table)
        + len(extents_table)
        + len(groups_table)
        + len(block_devices_table)
    )

    header_prefix = struct.pack(
        "<IHHI32sI32sIIIIIIIIIIII",
        LP_METADATA_HEADER_MAGIC,
        10,
        2,
        header_size,
        b"\x00" * 32,
        tables_size,
        b"\x00" * 32,
        partitions_offset,
        len(partitions),
        52,
        extents_offset,
        len(extents),
        24,
        groups_offset,
        len(groups),
        48,
        block_devices_offset,
        len(block_devices),
        64,
    )
    header = bytearray(header_size)
    header[: len(header_prefix)] = header_prefix
    struct.pack_into("<I", header, 128, LP_HEADER_FLAG_VIRTUAL_AB_DEVICE)

    header_offset = 1024
    chunk1_bytes[header_offset : header_offset + header_size] = header
    table_offset = header_offset + header_size
    cursor = table_offset
    for table in (partitions_table, extents_table, groups_table, block_devices_table):
        chunk1_bytes[cursor : cursor + len(table)] = table
        cursor += len(table)

    chunk1_bytes[6 * LP_SECTOR_SIZE : 8 * LP_SECTOR_SIZE] = b"S" * (2 * LP_SECTOR_SIZE)
    chunk1_bytes[9 * LP_SECTOR_SIZE : 10 * LP_SECTOR_SIZE] = b"V" * LP_SECTOR_SIZE
    chunk2_bytes[0 : 2 * LP_SECTOR_SIZE] = b"V" * (2 * LP_SECTOR_SIZE)

    chunk1.write_bytes(chunk1_bytes)
    chunk2.write_bytes(chunk2_bytes)

    records = [
        PartitionRecord(
            label="super",
            filename="super_1.img",
            lun="0",
            start_sector="100",
            num_sectors="10",
            source_xml="rawprogram0.xml",
            size_in_kb=None,
        ),
        PartitionRecord(
            label="super",
            filename="super_2.img",
            lun="0",
            start_sector="110",
            num_sectors="10",
            source_xml="rawprogram0.xml",
            size_in_kb=None,
        ),
    ]
    return records, tmp_path


def test_parse_super_layout_extracts_dynamic_images_across_chunk_boundaries(tmp_path):
    records, image_dir = _build_test_super_layout(tmp_path)

    layout = parse_super_layout(records, image_dir)
    extracted = extract_partition_images(
        layout, tmp_path / "dynamic", ["system", "vendor"]
    )

    assert layout.dynamic_partition_names == {"system", "vendor"}
    assert extracted["system"].read_bytes() == b"S" * (2 * LP_SECTOR_SIZE)
    assert extracted["vendor"].read_bytes() == b"V" * (3 * LP_SECTOR_SIZE)


def test_build_lpmake_command_uses_layout_metadata(tmp_path):
    records, image_dir = _build_test_super_layout(tmp_path)
    layout = parse_super_layout(records, image_dir)

    dynamic_dir = tmp_path / "images"
    dynamic_dir.mkdir()
    (dynamic_dir / "system.img").write_bytes(b"S" * (2 * LP_SECTOR_SIZE))
    (dynamic_dir / "vendor.img").write_bytes(b"V" * (3 * LP_SECTOR_SIZE))

    command = build_lpmake_command(
        layout,
        dynamic_dir,
        tmp_path / "super.img",
        "lpmake",
    )

    assert command[0] == "lpmake"
    assert "--metadata-size=4096" in command
    assert "--metadata-slots=3" in command
    assert "--group=dynamic_a:8192" in command
    assert "--group=dynamic_b:8192" in command
    assert "--device=super:10240" in command
    assert "--super-name=super" in command
    assert "--virtual-ab" in command
    assert any(
        part.startswith("--partition=system_a:readonly:1024:dynamic_a")
        for part in command
    )
    assert any(
        part.startswith("--partition=vendor_a:readonly:1536:dynamic_a")
        for part in command
    )
    assert "--partition=system_b:readonly:0:dynamic_b" in command
    assert not any(item.startswith("--image=system_b=") for item in command)


def test_build_lpmake_command_uses_custom_path_resolver(tmp_path):
    records, image_dir = _build_test_super_layout(tmp_path)
    layout = parse_super_layout(records, image_dir)

    dynamic_dir = tmp_path / "images"
    dynamic_dir.mkdir()
    (dynamic_dir / "system.img").write_bytes(b"S" * (2 * LP_SECTOR_SIZE))
    (dynamic_dir / "vendor.img").write_bytes(b"V" * (3 * LP_SECTOR_SIZE))

    command = build_lpmake_command(
        layout,
        dynamic_dir,
        tmp_path / "super.img",
        "lpmake",
        lambda path: f"/mnt/test/{path.name}",
    )

    assert "--image=system_a=/mnt/test/system.img" in command
    assert "--image=vendor_a=/mnt/test/vendor.img" in command
    assert "--output=/mnt/test/super.img" in command


def test_split_rebuilt_super_uses_original_chunk_layout(tmp_path):
    records, image_dir = _build_test_super_layout(tmp_path)
    layout = parse_super_layout(records, image_dir)

    rebuilt_super = tmp_path / "super_rebuilt.img"
    rebuilt_super.write_bytes(
        b"A" * (10 * LP_SECTOR_SIZE) + b"B" * (10 * LP_SECTOR_SIZE)
    )

    output_dir = tmp_path / "split"
    split_rebuilt_super(layout, rebuilt_super, output_dir)

    assert (output_dir / "super_1.img").read_bytes() == b"A" * (10 * LP_SECTOR_SIZE)
    assert (output_dir / "super_2.img").read_bytes() == b"B" * (10 * LP_SECTOR_SIZE)


def test_create_keep_data_ota_xml_blanks_userdata_and_metadata(tmp_path):
    output_dir = tmp_path / "image_new"
    output_dir.mkdir()
    source_xml = output_dir / "rawprogram_save_persist_unsparse0.xml"
    source_xml.write_text(
        """<?xml version='1.0'?>
<data>
  <program label='system' filename='system.img' />
  <program label='userdata' filename='userdata.img' />
  <program label='metadata' filename='metadata.img' />
</data>
""",
        encoding="utf-8",
    )

    target_xml = create_keep_data_ota_xml(output_dir)
    root = ET.parse(target_xml).getroot()
    files = {
        program.get("label"): program.get("filename")
        for program in root.findall("program")
    }

    assert files["system"] == "system.img"
    assert files["userdata"] == ""
    assert files["metadata"] == ""
