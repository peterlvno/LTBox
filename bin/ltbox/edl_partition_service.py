import sys

from .part.service import EdlPartitionService
from .part import service as _module

__all__ = ["EdlPartitionService"]

sys.modules[__name__] = _module
