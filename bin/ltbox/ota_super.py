from __future__ import annotations

import shutil
import struct
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Optional, Sequence

from .errors import MissingFileError, ToolError
from .xml_catalog import PartitionRecord

LP_METADATA_GEOMETRY_MAGIC = 0x616C4467
LP_METADATA_HEADER_MAGIC = 0x414C5030
LP_HEADER_FLAG_VIRTUAL_AB_DEVICE = 0x1
LP_PARTITION_ATTR_READONLY = 1 << 0
LP_SECTOR_SIZE = 512
LP_TARGET_TYPE_LINEAR = 0
LP_TARGET_TYPE_ZERO = 1

_GEOMETRY_STRUCT = struct.Struct("<II32sIII")
_HEADER_V1_0_STRUCT = struct.Struct("<IHHI32sI32sIIIIIIIIIIII")
_PARTITION_STRUCT = struct.Struct("<36sIIII")
_EXTENT_STRUCT = struct.Struct("<QIQI")
_GROUP_STRUCT = struct.Struct("<36sIQ")
_BLOCK_DEVICE_STRUCT = struct.Struct("<QIIQ36sI")


def _decode_c_string(raw: bytes) -> str:
    return raw.split(b"\x00", 1)[0].decode("ascii", errors="ignore")


@dataclass(frozen=True)
class SuperGeometry:
    metadata_max_size: int
    metadata_slot_count: int
    logical_block_size: int


@dataclass(frozen=True)
class TableDescriptor:
    offset: int
    num_entries: int
    entry_size: int


@dataclass(frozen=True)
class SuperBlockDevice:
    name: str
    size: int


@dataclass(frozen=True)
class SuperGroup:
    name: str
    maximum_size: int


@dataclass(frozen=True)
class SuperExtent:
    num_sectors: int
    target_type: int
    target_data: int
    target_source: int


@dataclass(frozen=True)
class SuperPartition:
    name: str
    attributes: int
    group_name: str
    extents: tuple[SuperExtent, ...]

    @property
    def slot_suffix(self) -> Optional[str]:
        lowered = self.name.lower()
        if lowered.endswith("_a"):
            return "a"
        if lowered.endswith("_b"):
            return "b"
        return None

    @property
    def base_name(self) -> str:
        suffix = self.slot_suffix
        if suffix is None:
            return self.name
        return self.name[:-2]

    @property
    def logical_size(self) -> int:
        return sum(extent.num_sectors * LP_SECTOR_SIZE for extent in self.extents)

    @property
    def attribute_name(self) -> str:
        if self.attributes & LP_PARTITION_ATTR_READONLY:
            return "readonly"
        return "none"


@dataclass(frozen=True)
class SuperChunk:
    filename: str
    path: Path
    start_sector: int
    num_sectors: int
    sector_size_bytes: int
    start_byte: int
    size_bytes: int
    relative_start_byte: int

    @property
    def relative_end_byte(self) -> int:
        return self.relative_start_byte + self.size_bytes


@dataclass(frozen=True)
class SuperLayout:
    geometry: SuperGeometry
    header_flags: int
    block_devices: tuple[SuperBlockDevice, ...]
    groups: tuple[SuperGroup, ...]
    partitions: tuple[SuperPartition, ...]
    chunks: tuple[SuperChunk, ...]

    @property
    def super_name(self) -> str:
        if self.block_devices:
            return self.block_devices[0].name
        return "super"

    @property
    def dynamic_partition_names(self) -> set[str]:
        return {
            partition.base_name
            for partition in self.partitions
            if partition.logical_size > 0 and partition.slot_suffix != "b"
        }

    def find_partition(self, name: str) -> Optional[SuperPartition]:
        normalized = name.lower()
        for partition in self.partitions:
            if partition.name.lower() == normalized:
                return partition
            if (
                partition.base_name.lower() == normalized
                and partition.slot_suffix != "b"
            ):
                return partition
        return None


def _find_geometry_offset(data: bytes) -> int:
    for offset in (0x1000, 0x2000, 0):
        if len(data) >= offset + _GEOMETRY_STRUCT.size:
            magic = struct.unpack_from("<I", data, offset)[0]
            if magic == LP_METADATA_GEOMETRY_MAGIC:
                return offset

    header_offset = _find_header_offset(data)
    geometry_magic = struct.pack("<I", LP_METADATA_GEOMETRY_MAGIC)
    geometry_offset = data.rfind(geometry_magic, 0, header_offset)
    if geometry_offset < 0:
        raise ToolError("LP metadata geometry not found in super chunk")
    return geometry_offset


def _parse_geometry(primary_chunk: Path) -> SuperGeometry:
    data = primary_chunk.read_bytes()
    if len(data) < _GEOMETRY_STRUCT.size:
        raise ToolError(
            f"super chunk too small to contain geometry: {primary_chunk.name}"
        )

    geometry_offset = _find_geometry_offset(data)

    (
        magic,
        _struct_size,
        _checksum,
        metadata_max_size,
        slot_count,
        logical_block_size,
    ) = _GEOMETRY_STRUCT.unpack_from(data, geometry_offset)
    if magic != LP_METADATA_GEOMETRY_MAGIC:
        raise ToolError(f"invalid super geometry in {primary_chunk.name}")

    return SuperGeometry(
        metadata_max_size=metadata_max_size,
        metadata_slot_count=slot_count,
        logical_block_size=logical_block_size,
    )


def _find_header_offset(data: bytes) -> int:
    magic_bytes = struct.pack("<I", LP_METADATA_HEADER_MAGIC)
    header_offset = data.find(magic_bytes)
    if header_offset < 0:
        raise ToolError("LP metadata header not found in super chunk")
    return header_offset


def _parse_table_descriptor(
    values: tuple[int, ...], start_index: int
) -> TableDescriptor:
    return TableDescriptor(
        offset=values[start_index],
        num_entries=values[start_index + 1],
        entry_size=values[start_index + 2],
    )


def _iter_table_entries(
    data: bytes,
    *,
    header_offset: int,
    header_size: int,
    descriptor: TableDescriptor,
    struct_size: int,
) -> Iterable[bytes]:
    table_offset = header_offset + header_size + descriptor.offset
    for index in range(descriptor.num_entries):
        record_offset = table_offset + (index * descriptor.entry_size)
        yield data[record_offset : record_offset + struct_size]


def _parse_metadata(
    primary_chunk: Path,
) -> tuple[
    int,
    tuple[SuperBlockDevice, ...],
    tuple[SuperGroup, ...],
    tuple[SuperPartition, ...],
]:
    data = primary_chunk.read_bytes()
    header_offset = _find_header_offset(data)

    header_values = _HEADER_V1_0_STRUCT.unpack_from(data, header_offset)
    magic = header_values[0]
    if magic != LP_METADATA_HEADER_MAGIC:
        raise ToolError(f"invalid LP metadata header in {primary_chunk.name}")

    header_size = header_values[3]
    partitions_desc = _parse_table_descriptor(header_values, 7)
    extents_desc = _parse_table_descriptor(header_values, 10)
    groups_desc = _parse_table_descriptor(header_values, 13)
    block_devices_desc = _parse_table_descriptor(header_values, 16)

    header_flags = 0
    if header_size >= 132:
        header_flags = struct.unpack_from("<I", data, header_offset + 128)[0]

    extents: list[SuperExtent] = []
    for raw_extent in _iter_table_entries(
        data,
        header_offset=header_offset,
        header_size=header_size,
        descriptor=extents_desc,
        struct_size=_EXTENT_STRUCT.size,
    ):
        num_sectors, target_type, target_data, target_source = _EXTENT_STRUCT.unpack(
            raw_extent
        )
        extents.append(
            SuperExtent(
                num_sectors=num_sectors,
                target_type=target_type,
                target_data=target_data,
                target_source=target_source,
            )
        )

    groups: list[SuperGroup] = []
    for raw_group in _iter_table_entries(
        data,
        header_offset=header_offset,
        header_size=header_size,
        descriptor=groups_desc,
        struct_size=_GROUP_STRUCT.size,
    ):
        name, _flags, maximum_size = _GROUP_STRUCT.unpack(raw_group)
        groups.append(
            SuperGroup(
                name=_decode_c_string(name),
                maximum_size=maximum_size,
            )
        )

    block_devices: list[SuperBlockDevice] = []
    for raw_device in _iter_table_entries(
        data,
        header_offset=header_offset,
        header_size=header_size,
        descriptor=block_devices_desc,
        struct_size=_BLOCK_DEVICE_STRUCT.size,
    ):
        (
            _first_logical_sector,
            _alignment,
            _alignment_offset,
            size,
            partition_name,
            _flags,
        ) = _BLOCK_DEVICE_STRUCT.unpack(raw_device)
        block_devices.append(
            SuperBlockDevice(
                name=_decode_c_string(partition_name),
                size=size,
            )
        )

    partitions: list[SuperPartition] = []
    for raw_partition in _iter_table_entries(
        data,
        header_offset=header_offset,
        header_size=header_size,
        descriptor=partitions_desc,
        struct_size=_PARTITION_STRUCT.size,
    ):
        name, attributes, first_extent_index, num_extents, group_index = (
            _PARTITION_STRUCT.unpack(raw_partition)
        )
        partition_name = _decode_c_string(name)
        group_name = (
            groups[group_index].name if group_index < len(groups) else "default"
        )
        partition_extents = tuple(
            extents[first_extent_index + index] for index in range(num_extents)
        )
        partitions.append(
            SuperPartition(
                name=partition_name,
                attributes=attributes,
                group_name=group_name,
                extents=partition_extents,
            )
        )

    return header_flags, tuple(block_devices), tuple(groups), tuple(partitions)


def parse_super_layout(
    super_records: Sequence[PartitionRecord],
    image_dir: Path,
) -> SuperLayout:
    if not super_records:
        raise MissingFileError("super partition records not found in rawprogram XML")

    ordered_records = sorted(
        super_records,
        key=lambda record: int(record.start_sector or "0"),
    )
    usable_records = [record for record in ordered_records if record.filename]
    if not usable_records:
        raise MissingFileError("no usable super chunks were found")

    first_sector_size_bytes = int(usable_records[0].sector_size_bytes or LP_SECTOR_SIZE)
    first_start_byte = (
        int(usable_records[0].start_sector or "0") * first_sector_size_bytes
    )

    chunks: list[SuperChunk] = []
    for record in usable_records:
        chunk_path = image_dir / record.filename
        if not chunk_path.exists():
            raise MissingFileError(f"missing super chunk: {chunk_path.name}")
        start_sector = int(record.start_sector or "0")
        num_sectors = int(record.num_sectors or "0")
        sector_size_bytes = int(record.sector_size_bytes or LP_SECTOR_SIZE)
        start_byte = start_sector * sector_size_bytes
        size_bytes = num_sectors * sector_size_bytes
        chunks.append(
            SuperChunk(
                filename=record.filename,
                path=chunk_path,
                start_sector=start_sector,
                num_sectors=num_sectors,
                sector_size_bytes=sector_size_bytes,
                start_byte=start_byte,
                size_bytes=size_bytes,
                relative_start_byte=start_byte - first_start_byte,
            )
        )

    geometry = _parse_geometry(chunks[0].path)
    header_flags, block_devices, groups, partitions = _parse_metadata(chunks[0].path)

    return SuperLayout(
        geometry=geometry,
        header_flags=header_flags,
        block_devices=block_devices,
        groups=groups,
        partitions=partitions,
        chunks=tuple(chunks),
    )


def _find_chunk(layout: SuperLayout, logical_byte: int) -> SuperChunk:
    for chunk in layout.chunks:
        if chunk.relative_start_byte <= logical_byte < chunk.relative_end_byte:
            return chunk
    raise ToolError(f"no super chunk covers logical byte offset {logical_byte}")


def extract_partition_images(
    layout: SuperLayout,
    output_dir: Path,
    partition_names: Optional[Iterable[str]] = None,
) -> dict[str, Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    requested = None
    if partition_names is not None:
        requested = {name.lower() for name in partition_names}

    extracted: dict[str, Path] = {}
    for partition in layout.partitions:
        if partition.slot_suffix == "b" or partition.logical_size <= 0:
            continue
        if requested is not None and partition.base_name.lower() not in requested:
            continue

        output_path = output_dir / f"{partition.base_name}.img"
        with open(output_path, "wb") as target:
            for extent in partition.extents:
                extent_size_bytes = extent.num_sectors * LP_SECTOR_SIZE
                if extent.target_type == LP_TARGET_TYPE_ZERO:
                    target.write(b"\x00" * extent_size_bytes)
                    continue
                if extent.target_type != LP_TARGET_TYPE_LINEAR:
                    raise ToolError(
                        f"unsupported super extent type {extent.target_type} for {partition.name}"
                    )

                remaining_bytes = extent_size_bytes
                current_byte = extent.target_data * LP_SECTOR_SIZE
                while remaining_bytes > 0:
                    chunk = _find_chunk(layout, current_byte)
                    byte_offset = current_byte - chunk.relative_start_byte
                    readable_bytes = min(
                        remaining_bytes,
                        chunk.size_bytes - byte_offset,
                    )
                    read_offset = byte_offset
                    read_size = readable_bytes
                    with open(chunk.path, "rb") as source:
                        source.seek(read_offset)
                        chunk_bytes = source.read(read_size)
                    if len(chunk_bytes) != read_size:
                        raise ToolError(
                            f"unexpected EOF while reading {chunk.filename} for {partition.name}"
                        )
                    target.write(chunk_bytes)
                    remaining_bytes -= readable_bytes
                    current_byte += readable_bytes

        extracted[partition.base_name] = output_path

    return extracted


def build_lpmake_command(
    layout: SuperLayout,
    image_dir: Path,
    output_path: Path,
    lpmake_executable: Optional[str] = None,
    path_resolver: Callable[[Path], str] = str,
) -> list[str]:
    if not layout.block_devices:
        raise ToolError("super metadata does not define any block devices")

    command = [
        f"--metadata-size={layout.geometry.metadata_max_size}",
        f"--metadata-slots={layout.geometry.metadata_slot_count}",
    ]
    if lpmake_executable is not None:
        command.insert(0, lpmake_executable)

    for group in layout.groups:
        # lpmake provides the implicit "default" group on its own.
        if group.name.strip().lower() == "default":
            continue
        command.append(f"--group={group.name}:{group.maximum_size}")

    for device in layout.block_devices:
        command.append(f"--device={device.name}:{device.size}")

    command.append(f"--super-name={layout.super_name}")
    if layout.header_flags & LP_HEADER_FLAG_VIRTUAL_AB_DEVICE:
        command.append("--virtual-ab")

    for partition in layout.partitions:
        image_path: Optional[Path] = None
        image_size = 0
        if partition.slot_suffix != "b":
            image_path = image_dir / f"{partition.base_name}.img"
            if not image_path.exists():
                raise MissingFileError(
                    f"missing dynamic partition image for rebuild: {image_path.name}"
                )
            image_size = image_path.stat().st_size

        command.append(
            f"--partition={partition.name}:{partition.attribute_name}:{image_size}:{partition.group_name}"
        )
        if image_path is not None:
            command.append(f"--image={partition.name}={path_resolver(image_path)}")

    command.append(f"--output={path_resolver(output_path)}")
    return command


def split_rebuilt_super(
    layout: SuperLayout, super_image: Path, output_dir: Path
) -> list[Path]:
    if not super_image.exists():
        raise MissingFileError(f"rebuilt super image not found: {super_image.name}")

    output_dir.mkdir(parents=True, exist_ok=True)
    created_files: list[Path] = []
    with open(super_image, "rb") as source:
        for chunk in layout.chunks:
            chunk_path = output_dir / chunk.filename
            with open(chunk_path, "wb") as target:
                target.write(source.read(chunk.size_bytes))
            created_files.append(chunk_path)

    return created_files


def copy_flash_xmls(
    rawprogram_paths: Iterable[Path],
    patch_paths: Iterable[Path],
    output_dir: Path,
) -> list[Path]:
    copied: list[Path] = []
    output_dir.mkdir(parents=True, exist_ok=True)

    for source_path in [*rawprogram_paths, *patch_paths]:
        destination = output_dir / source_path.name
        shutil.copy2(source_path, destination)
        copied.append(destination)

    return copied


def create_keep_data_ota_xml(output_dir: Path) -> Path:
    source_candidates = [
        output_dir / "rawprogram_save_persist_unsparse0.xml",
        output_dir / "rawprogram_unsparse0.xml",
        output_dir / "rawprogram0.xml",
    ]
    source_xml = next((path for path in source_candidates if path.exists()), None)
    if source_xml is None:
        raise MissingFileError("rawprogram XML not found for OTA keep-data generation")

    target_xml = output_dir / "rawprogram_save_persist_ota_unsparse0.xml"
    tree = ET.parse(source_xml)
    root = tree.getroot()

    for program in root.findall("program"):
        label = (program.get("label") or "").lower()
        if label.startswith("metadata") or label.startswith("userdata"):
            program.set("filename", "")

    tree.write(target_xml, encoding="utf-8", xml_declaration=True)
    return target_xml
