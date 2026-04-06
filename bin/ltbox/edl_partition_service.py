from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Optional

from .i18n import get_string
from .partition import PartitionParams
from .utils import ui


@dataclass(frozen=True)
class EdlPartitionService:
    resolve_params: Callable[[str], PartitionParams]

    def get_params(self, label: str) -> PartitionParams:
        return self.resolve_params(label)

    def _announce_partition_details(
        self, params: PartitionParams, *, message_key: str = "act_found_dump_info"
    ) -> None:
        ui.echo(
            get_string(message_key).format(
                xml=params["source_xml"],
                lun=params["lun"],
                start=params["start_sector"],
            )
        )

    def _validate_dump_size(
        self,
        *,
        label: str,
        output_path: Path,
        params: PartitionParams,
    ) -> None:
        size_in_kb = params.get("size_in_kb")
        if not size_in_kb:
            return

        expected_size = int(float(size_in_kb) * 1024)
        actual_size = output_path.stat().st_size
        if expected_size != actual_size:
            raise RuntimeError(
                get_string("act_err_dump_size_mismatch").format(
                    target=label,
                    expected=expected_size,
                    actual=actual_size,
                )
            )

    def flash_partition(
        self,
        dev: Any,
        port: str,
        target_name: str,
        image_path: Path,
    ) -> PartitionParams:
        params = self.resolve_params(target_name)
        ui.echo(get_string("act_flashing_target").format(target=target_name))
        self._announce_partition_details(params)

        ui.echo(
            get_string("device_flashing_part").format(
                filename=image_path.name,
                lun=params["lun"],
                start=params["start_sector"],
            )
        )
        dev.edl.write_partition(
            port=port,
            image_path=image_path,
            lun=params["lun"],
            start_sector=params["start_sector"],
            partition_name=target_name,
        )
        ui.echo(get_string("device_flash_success").format(filename=image_path.name))
        return params

    def dump_partition(
        self,
        dev: Any,
        port: str,
        label: str,
        output_path: Path,
        *,
        params: Optional[PartitionParams] = None,
    ) -> PartitionParams:
        resolved_params = params or self.get_params(label)
        self._announce_partition_details(resolved_params)
        dev.edl.read_partition(
            port=port,
            output_filename=str(output_path),
            lun=resolved_params["lun"],
            start_sector=resolved_params["start_sector"],
            num_sectors=resolved_params["num_sectors"],
            partition_name=label,
        )
        self._validate_dump_size(
            label=label,
            output_path=output_path,
            params=resolved_params,
        )
        return resolved_params
