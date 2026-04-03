import functools
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Dict, Iterable, List, Optional, Tuple

from . import constants as const
from . import utils
from .crypto import decrypt_file
from .i18n import get_string

PartitionParams = Dict[str, Optional[str]]
ErrorReporter = Callable[[str], None]
PathSignature = Tuple[str, Optional[int], Optional[int]]
ParseErrorInfo = Tuple[str, str]


def _default_error_reporter(message: str) -> None:
    print(message)


def _build_path_signature(xml_path: Path) -> PathSignature:
    resolved = str(xml_path.resolve())
    try:
        stat_result = xml_path.stat()
    except OSError:
        return resolved, None, None
    return resolved, stat_result.st_mtime_ns, stat_result.st_size


@functools.lru_cache(maxsize=32)
def _parse_xml_records(
    path_signature: PathSignature,
) -> Tuple[Tuple["PartitionRecord", ...], Optional[ParseErrorInfo]]:
    path_str, _, _ = path_signature
    xml_path = Path(path_str)

    try:
        tree = ET.parse(xml_path)
    except (ET.ParseError, OSError) as error:
        return (), (xml_path.name, str(error))

    records: List[PartitionRecord] = []
    root = tree.getroot()
    for program in root.findall("program"):
        label = (program.get("label") or "").strip()
        if not label:
            continue

        records.append(
            PartitionRecord(
                label=label,
                filename=(program.get("filename") or "").strip(),
                lun=program.get("physical_partition_number"),
                start_sector=program.get("start_sector"),
                num_sectors=program.get("num_partition_sectors"),
                source_xml=xml_path.name,
                size_in_kb=program.get("size_in_KB"),
                sector_size_bytes=program.get("SECTOR_SIZE_IN_BYTES"),
            )
        )

    return tuple(records), None


@dataclass(frozen=True)
class PartitionRecord:
    label: str
    filename: str
    lun: Optional[str]
    start_sector: Optional[str]
    num_sectors: Optional[str]
    source_xml: str
    size_in_kb: Optional[str]
    sector_size_bytes: Optional[str] = None

    @property
    def slot_suffix(self) -> Optional[str]:
        lowered = self.label.lower()
        if lowered.endswith("_a"):
            return "a"
        if lowered.endswith("_b"):
            return "b"
        return None

    @property
    def is_ab(self) -> bool:
        return self.slot_suffix is not None

    @property
    def base_label(self) -> str:
        return self.label[:-2] if self.is_ab else self.label

    def to_params(self) -> PartitionParams:
        return {
            "lun": self.lun,
            "start_sector": self.start_sector,
            "num_sectors": self.num_sectors,
            "filename": self.filename,
            "source_xml": self.source_xml,
            "size_in_kb": self.size_in_kb,
            "sector_size_bytes": self.sector_size_bytes,
        }


@dataclass
class PartitionGroup:
    base_label: str
    a: List[PartitionRecord] = field(default_factory=list)
    b: List[PartitionRecord] = field(default_factory=list)
    none: List[PartitionRecord] = field(default_factory=list)

    @property
    def is_ab(self) -> bool:
        return bool(self.a or self.b)

    @property
    def has_files(self) -> bool:
        return any(record.filename.strip() for record in self.records)

    @property
    def records(self) -> List[PartitionRecord]:
        return [*self.a, *self.b, *self.none]

    def add(self, record: PartitionRecord) -> None:
        slot = record.slot_suffix
        if slot == "a":
            self.a.append(record)
        elif slot == "b":
            self.b.append(record)
        else:
            self.none.append(record)

    def slot_records(self, slot: str) -> List[PartitionRecord]:
        if slot == "a":
            return self.a
        if slot == "b":
            return self.b
        return self.none


class XmlCatalog:
    def __init__(self, records: Iterable[PartitionRecord]):
        self._records = list(records)

    @property
    def records(self) -> List[PartitionRecord]:
        return list(self._records)

    @classmethod
    def from_paths(
        cls,
        xml_paths: Iterable[Path],
        *,
        on_error: Optional[ErrorReporter] = None,
    ) -> "XmlCatalog":
        reporter = on_error or _default_error_reporter
        records: List[PartitionRecord] = []

        for xml_path in xml_paths:
            parsed_records, error_info = _parse_xml_records(
                _build_path_signature(xml_path)
            )
            if error_info is not None:
                name, error_message = error_info
                reporter(
                    get_string("act_xml_parse_err").format(
                        name=name,
                        e=error_message,
                    )
                )
                continue

            records.extend(parsed_records)

        return cls(records)

    @classmethod
    def from_environment(cls) -> "XmlCatalog":
        return cls.from_paths(scan_and_decrypt_xmls())

    def find_partition(self, target_label: str) -> Optional[PartitionRecord]:
        normalized = target_label.lower()
        for record in self._records:
            if record.label.lower() == normalized:
                return record
        return None

    def require_partition(
        self,
        label: str,
        *,
        fallback_labels: Optional[Iterable[str]] = None,
    ) -> PartitionRecord:
        labels = [label]
        if fallback_labels:
            labels.extend(fallback_labels)

        for candidate in labels:
            record = self.find_partition(candidate)
            if record is not None:
                return record

        print(get_string("act_err_part_info_missing").format(label=label))
        raise ValueError(get_string("act_err_part_not_found").format(label=label))

    def group_by_base_label(
        self, *, with_files_only: bool = False
    ) -> Dict[str, PartitionGroup]:
        groups: Dict[str, PartitionGroup] = {}
        for record in self._records:
            base_label = record.base_label
            group = groups.setdefault(base_label, PartitionGroup(base_label=base_label))
            group.add(record)

        if with_files_only:
            return {label: group for label, group in groups.items() if group.has_files}
        return groups


def scan_and_decrypt_xmls() -> List[Path]:
    const.OUTPUT_XML_DIR.mkdir(exist_ok=True)

    xmls = list(const.OUTPUT_XML_DIR.glob("rawprogram*.xml"))
    if not xmls:
        xmls = list(const.IMAGE_DIR.glob("rawprogram*.xml"))

    if not xmls:
        print(get_string("act_xml_scan_x"))
        x_files = list(const.IMAGE_DIR.glob("*.x"))

        if x_files:
            print(get_string("act_xml_found_x_count").format(len=len(x_files)))
            utils.check_dependencies()
            for x_file in x_files:
                xml_name = x_file.stem + ".xml"
                out_path = const.OUTPUT_XML_DIR / xml_name
                if out_path.exists():
                    xmls.append(out_path)
                    continue

                print(get_string("act_xml_decrypting").format(name=x_file.name))
                if decrypt_file(str(x_file), str(out_path)):
                    xmls.append(out_path)
                else:
                    print(get_string("act_xml_decrypt_fail").format(name=x_file.name))
        else:
            print(get_string("img_xml_no_files").format(dir=const.IMAGE_DIR.name))
            print(get_string("act_xml_dump_req"))
            print(get_string("act_xml_place_prompt"))
            return []

    return xmls
