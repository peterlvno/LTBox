from __future__ import annotations

import importlib
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, List

from . import constants as const
from .errors import ToolError


@dataclass(frozen=True)
class PayloadPartitionInfo:
    name: str
    new_size: int


def _ensure_update_engine_scripts() -> None:
    scripts_dir = const.UPDATE_ENGINE_SCRIPTS_DIR
    package_init = scripts_dir / "update_payload" / "__init__.py"
    update_metadata_pb2 = scripts_dir / "update_metadata_pb2.py"

    if not package_init.exists() or not update_metadata_pb2.exists():
        raise ToolError(
            "Required OTA tool missing: "
            f"{scripts_dir}. Re-download or re-extract the LTBox release package."
        )

    # The official generated protobuf bindings still rely on the pure-Python
    # implementation with modern protobuf wheels.
    os.environ["PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION"] = "python"

    scripts_dir_str = str(scripts_dir)
    if scripts_dir_str not in sys.path:
        sys.path.insert(0, scripts_dir_str)
        importlib.invalidate_caches()


def _load_update_payload_module():
    _ensure_update_engine_scripts()
    try:
        return importlib.import_module("update_payload")
    except Exception as e:  # pragma: no cover - exercised via caller paths
        raise ToolError(
            f"Failed to load bundled update_engine payload tools: {e}"
        ) from e


def get_partition_infos(payload_path: Path) -> List[PayloadPartitionInfo]:
    update_payload = _load_update_payload_module()

    try:
        payload = update_payload.Payload(str(payload_path))
    except Exception as e:
        raise ToolError(f"Failed to parse payload metadata: {e}") from e

    return [
        PayloadPartitionInfo(
            name=partition.partition_name,
            new_size=int(partition.new_partition_info.size),
        )
        for partition in payload.manifest.partitions
        if partition.partition_name
    ]


def get_partition_names(payload_path: Path) -> List[str]:
    return [info.name for info in get_partition_infos(payload_path)]


def get_partition_sizes(payload_path: Path) -> dict[str, int]:
    return {info.name: info.new_size for info in get_partition_infos(payload_path)}


def partition_names_from_infos(
    partition_infos: Iterable[PayloadPartitionInfo],
) -> List[str]:
    return [info.name for info in partition_infos]
