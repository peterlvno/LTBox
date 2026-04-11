from pathlib import Path
from typing import Optional

from ..i18n import get_string
from .xml_catalog import PartitionParams, XmlCatalog, scan_and_decrypt_xmls


def get_partition_params(
    target_label: str, xml_paths: list[Path]
) -> Optional[PartitionParams]:
    record = XmlCatalog.from_paths(xml_paths).find_partition(target_label)
    if record is None:
        return None
    return record.to_params()


def require_partition_params(label: str) -> PartitionParams:
    xmls = scan_and_decrypt_xmls()
    if not xmls:
        raise FileNotFoundError(get_string("act_err_no_xml_dump"))

    catalog = XmlCatalog.from_paths(xmls)
    labels = [label]
    if label == "boot":
        labels.extend(["boot_a", "boot_b"])

    params = None
    for candidate in labels:
        record = catalog.find_partition(candidate)
        if record is not None:
            params = record.to_params()
            break

    if params:
        return params

    print(get_string("act_err_part_info_missing").format(label=label))
    raise ValueError(get_string("act_err_part_not_found").format(label=label))
