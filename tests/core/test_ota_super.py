import struct
import xml.etree.ElementTree as ET
from pathlib import Path

from ltbox.ota_super import (
    LP_HEADER_FLAG_VIRTUAL_AB_DEVICE,
    LP_METADATA_GEOMETRY_MAGIC,
    LP_METADATA_HEADER_MAGIC,
    LP_SECTOR_SIZE,
    SuperBlockDevice,
    SuperExtent,
    SuperGeometry,
    SuperGroup,
    SuperLayout,
    SuperPartition,
    build_lpmake_command,
    create_keep_data_ota_xml,
    extract_partition_images,
    parse_super_layout,
    plan_rebuilt_super_chunks,
    rewrite_xml_filenames,
    rewrite_super_xml_entries,
    split_rebuilt_super,
    write_rebuilt_super_chunks,
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


def _build_realistic_super_layout(tmp_path: Path) -> tuple[list[PartitionRecord], Path]:
    flash_sector_size = 4096
    chunk1 = tmp_path / "super_1.img"
    chunk2 = tmp_path / "super_2.img"

    chunk1_bytes = bytearray(32 * flash_sector_size)
    chunk2_bytes = bytearray(32 * flash_sector_size)

    for geometry_offset in (0x1000, 0x2000):
        struct.pack_into(
            "<II32sIII",
            chunk1_bytes,
            geometry_offset,
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
    ]
    extents = [
        struct.pack("<QIQI", 6, 0, 32, 0),
        struct.pack("<QIQI", 10, 0, 250, 0),
    ]
    groups = [struct.pack("<36sIQ", b"dynamic_a", 0, 65536)]
    block_devices = [
        struct.pack("<QIIQ36sI", 6, 4096, 0, 64 * flash_sector_size, b"super", 0),
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

    header_offset = 0x3000
    chunk1_bytes[header_offset : header_offset + header_size] = header
    table_offset = header_offset + header_size
    cursor = table_offset
    for table in (partitions_table, extents_table, groups_table, block_devices_table):
        chunk1_bytes[cursor : cursor + len(table)] = table
        cursor += len(table)

    system_offset = 32 * LP_SECTOR_SIZE
    system_bytes = 6 * LP_SECTOR_SIZE
    vendor_offset = 250 * LP_SECTOR_SIZE
    vendor_bytes = 10 * LP_SECTOR_SIZE
    chunk1_bytes[system_offset : system_offset + system_bytes] = b"S" * system_bytes
    vendor_in_chunk1 = len(chunk1_bytes) - vendor_offset
    chunk1_bytes[vendor_offset:] = b"V" * vendor_in_chunk1
    remaining_vendor = vendor_bytes - vendor_in_chunk1
    chunk2_bytes[0:remaining_vendor] = b"V" * remaining_vendor

    chunk1.write_bytes(chunk1_bytes)
    chunk2.write_bytes(chunk2_bytes)

    records = [
        PartitionRecord(
            label="super",
            filename="super_1.img",
            lun="0",
            start_sector="90504",
            num_sectors=str(len(chunk1_bytes) // flash_sector_size),
            source_xml="rawprogram_unsparse0.xml",
            size_in_kb=None,
            sector_size_bytes=str(flash_sector_size),
        ),
        PartitionRecord(
            label="super",
            filename="super_2.img",
            lun="0",
            start_sector=str(90504 + (len(chunk1_bytes) // flash_sector_size)),
            num_sectors=str(len(chunk2_bytes) // flash_sector_size),
            source_xml="rawprogram_unsparse0.xml",
            size_in_kb=None,
            sector_size_bytes=str(flash_sector_size),
        ),
    ]
    return records, tmp_path


def _build_partition_chunk_super_layout(
    tmp_path: Path,
) -> tuple[list[PartitionRecord], Path, Path]:
    flash_sector_size = 4096
    full_super = tmp_path / "super_full.img"
    chunk1 = tmp_path / "super_1.img"
    chunk2 = tmp_path / "super_2.img"
    chunk3 = tmp_path / "super_3.img"

    full_bytes = bytearray(64 * flash_sector_size)

    for geometry_offset in (0x1000, 0x2000):
        struct.pack_into(
            "<II32sIII",
            full_bytes,
            geometry_offset,
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
    ]
    extents = [
        struct.pack("<QIQI", 48, 0, 256, 0),
        struct.pack("<QIQI", 80, 0, 400, 0),
    ]
    groups = [struct.pack("<36sIQ", b"dynamic_a", 0, 64 * flash_sector_size)]
    block_devices = [
        struct.pack("<QIIQ36sI", 6, 4096, 0, 64 * flash_sector_size, b"super", 0),
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

    header_offset = 0x3000
    full_bytes[header_offset : header_offset + header_size] = header
    table_offset = header_offset + header_size
    cursor = table_offset
    for table in (partitions_table, extents_table, groups_table, block_devices_table):
        full_bytes[cursor : cursor + len(table)] = table
        cursor += len(table)

    full_bytes[32 * flash_sector_size : 38 * flash_sector_size] = b"S" * (
        6 * flash_sector_size
    )
    full_bytes[50 * flash_sector_size : 60 * flash_sector_size] = b"V" * (
        10 * flash_sector_size
    )
    full_super.write_bytes(full_bytes)

    chunk1.write_bytes(full_bytes[: 4 * flash_sector_size])
    chunk2.write_bytes(full_bytes[32 * flash_sector_size : 38 * flash_sector_size])
    chunk3.write_bytes(full_bytes[50 * flash_sector_size : 60 * flash_sector_size])

    records = [
        PartitionRecord(
            label="super",
            filename="super_1.img",
            lun="0",
            start_sector="90504",
            num_sectors="4",
            source_xml="rawprogram_unsparse0.xml",
            size_in_kb=None,
            sector_size_bytes=str(flash_sector_size),
        ),
        PartitionRecord(
            label="super",
            filename="super_2.img",
            lun="0",
            start_sector=str(90504 + 32),
            num_sectors="6",
            source_xml="rawprogram_unsparse0.xml",
            size_in_kb=None,
            sector_size_bytes=str(flash_sector_size),
        ),
        PartitionRecord(
            label="super",
            filename="super_3.img",
            lun="0",
            start_sector=str(90504 + 50),
            num_sectors="10",
            source_xml="rawprogram_unsparse0.xml",
            size_in_kb=None,
            sector_size_bytes=str(flash_sector_size),
        ),
    ]
    return records, tmp_path, full_super


def test_parse_super_layout_extracts_dynamic_images_across_chunk_boundaries(tmp_path):
    records, image_dir = _build_test_super_layout(tmp_path)

    layout = parse_super_layout(records, image_dir)
    extracted = extract_partition_images(
        layout, tmp_path / "dynamic", ["system", "vendor"]
    )

    assert layout.dynamic_partition_names == {"system", "vendor"}
    assert extracted["system"].read_bytes() == b"S" * (2 * LP_SECTOR_SIZE)
    assert extracted["vendor"].read_bytes() == b"V" * (3 * LP_SECTOR_SIZE)


def test_parse_super_layout_handles_realistic_geometry_offsets_and_4k_xml_sectors(
    tmp_path,
):
    records, image_dir = _build_realistic_super_layout(tmp_path)

    layout = parse_super_layout(records, image_dir)
    extracted = extract_partition_images(layout, tmp_path / "dynamic")

    assert layout.dynamic_partition_names == {"system", "vendor"}
    assert extracted["system"].read_bytes() == b"S" * (6 * LP_SECTOR_SIZE)
    assert extracted["vendor"].read_bytes() == b"V" * (10 * LP_SECTOR_SIZE)


def test_parse_super_layout_deduplicates_duplicate_super_records(tmp_path):
    records, image_dir, _ = _build_partition_chunk_super_layout(tmp_path)

    layout = parse_super_layout(records + records, image_dir)

    assert [chunk.filename for chunk in layout.chunks] == [
        "super_1.img",
        "super_2.img",
        "super_3.img",
    ]


def test_write_rebuilt_super_chunks_and_rewrite_xml_follow_rebuilt_extents(tmp_path):
    records, image_dir, full_super = _build_partition_chunk_super_layout(tmp_path)
    layout = parse_super_layout(records, image_dir)

    chunk_plans = plan_rebuilt_super_chunks(layout, layout)
    assert [chunk.filename for chunk in chunk_plans] == [
        "super_1.img",
        "super_2.img",
        "super_3.img",
    ]

    output_dir = tmp_path / "split"
    output_dir.mkdir()
    (output_dir / "super_9.img").write_bytes(b"stale")
    created = write_rebuilt_super_chunks(full_super, chunk_plans, output_dir)

    assert [path.name for path in created] == [
        "super_1.img",
        "super_2.img",
        "super_3.img",
    ]
    assert not (output_dir / "super_9.img").exists()
    assert (output_dir / "super_1.img").read_bytes()[0x3000:0x3004] == struct.pack(
        "<I", LP_METADATA_HEADER_MAGIC
    )
    assert (output_dir / "super_2.img").read_bytes() == b"S" * (6 * 4096)
    assert (output_dir / "super_3.img").read_bytes() == b"V" * (10 * 4096)

    xml_path = tmp_path / "rawprogram_unsparse0.xml"
    xml_path.write_text(
        """<?xml version='1.0'?>
<data>
  <program label='super' filename='old_super_1.img' start_sector='1' num_partition_sectors='1' SECTOR_SIZE_IN_BYTES='4096' />
  <program label='super' filename='old_super_2.img' start_sector='2' num_partition_sectors='2' SECTOR_SIZE_IN_BYTES='4096' />
  <program label='userdata' filename='userdata.img' start_sector='3' num_partition_sectors='3' SECTOR_SIZE_IN_BYTES='4096' />
</data>
""",
        encoding="utf-8",
    )

    updated = rewrite_super_xml_entries([xml_path], chunk_plans)
    assert updated == [xml_path]

    root = ET.parse(xml_path).getroot()
    super_entries = [
        (
            program.get("filename"),
            program.get("start_sector"),
            program.get("num_partition_sectors"),
        )
        for program in root.findall("program")
        if (program.get("label") or "").lower() == "super"
    ]
    assert super_entries == [
        ("super_1.img", "90504", "4"),
        ("super_2.img", "90536", "6"),
        ("super_3.img", "90554", "10"),
    ]


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


def test_build_lpmake_command_skips_implicit_default_group(tmp_path):
    dynamic_dir = tmp_path / "images"
    dynamic_dir.mkdir()
    (dynamic_dir / "system.img").write_bytes(b"S" * (2 * LP_SECTOR_SIZE))

    layout = SuperLayout(
        geometry=SuperGeometry(
            metadata_max_size=4096,
            metadata_slot_count=3,
            logical_block_size=4096,
        ),
        header_flags=0,
        block_devices=(SuperBlockDevice(name="super", size=4096),),
        groups=(
            SuperGroup(name="default", maximum_size=0),
            SuperGroup(name="dynamic_a", maximum_size=2048),
        ),
        partitions=(
            SuperPartition(
                name="system_a",
                attributes=1,
                group_name="default",
                extents=(SuperExtent(2, 0, 0, 0),),
            ),
        ),
        chunks=(),
    )

    command = build_lpmake_command(
        layout,
        dynamic_dir,
        tmp_path / "super.img",
        "lpmake",
    )

    assert "--group=default:0" not in command
    assert "--group=dynamic_a:2048" in command
    assert "--partition=system_a:readonly:1024:default" in command


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
    assert target_xml == source_xml
    root = ET.parse(target_xml).getroot()
    files = {
        program.get("label"): program.get("filename")
        for program in root.findall("program")
    }

    assert files["system"] == "system.img"
    assert files["userdata"] == ""
    assert files["metadata"] == ""


def test_rewrite_xml_filenames_updates_rawprogram_and_patch_entries(tmp_path):
    rawprogram_xml = tmp_path / "rawprogram0.xml"
    patch_xml = tmp_path / "patch0.xml"
    rawprogram_xml.write_text(
        """<?xml version='1.0'?>
<data>
  <program label='xbl_a' filename='xbl.img' />
  <program label='vbmeta_a' filename='vbmeta.img' />
</data>
""",
        encoding="utf-8",
    )
    patch_xml.write_text(
        """<?xml version='1.0'?>
<patches>
  <patch filename="xbl.img" />
  <patch filename="DISK" />
</patches>
""",
        encoding="utf-8",
    )

    updated = rewrite_xml_filenames(
        [rawprogram_xml, patch_xml],
        {"xbl.img": "xbl.elf", "vbmeta.img": "vbmeta.bin"},
    )

    assert updated == [rawprogram_xml, patch_xml]
    assert "filename='xbl.elf'" in rawprogram_xml.read_text(encoding="utf-8")
    assert "filename='vbmeta.bin'" in rawprogram_xml.read_text(encoding="utf-8")
    assert 'filename="xbl.elf"' in patch_xml.read_text(encoding="utf-8")
    assert 'filename="DISK"' in patch_xml.read_text(encoding="utf-8")
