import sys

from .part.xml_catalog import (
    PartitionGroup,
    PartitionParams,
    PartitionRecord,
    XmlCatalog,
    scan_and_decrypt_xmls,
)
from .part import xml_catalog as _module

__all__ = [
    "PartitionGroup",
    "PartitionParams",
    "PartitionRecord",
    "XmlCatalog",
    "scan_and_decrypt_xmls",
]

sys.modules[__name__] = _module
