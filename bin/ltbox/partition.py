import sys

from .part.partition import (
    PartitionParams,
    XmlCatalog,
    get_partition_params,
    require_partition_params,
    scan_and_decrypt_xmls,
)
from .part import partition as _module

__all__ = [
    "PartitionParams",
    "XmlCatalog",
    "get_partition_params",
    "require_partition_params",
    "scan_and_decrypt_xmls",
]

sys.modules[__name__] = _module
