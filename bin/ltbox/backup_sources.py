import sys

from .part.backups import (
    find_backup_critical_dirs,
    find_dp_source_folders,
    format_dp_folder_label,
)
from .part import backups as _module

__all__ = [
    "find_backup_critical_dirs",
    "find_dp_source_folders",
    "format_dp_folder_label",
]

sys.modules[__name__] = _module
