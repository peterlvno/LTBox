from pathlib import Path
from typing import List

from ..patch.region import detect_country_codes


def format_dp_folder_label(folder: Path) -> str:
    codes = detect_country_codes(source_dir=folder)
    parts = []
    for filename in ["devinfo.img", "persist.img"]:
        code = codes.get(filename)
        label = Path(filename).stem
        parts.append(f"{label}: {code.upper() if code else '?'}")
    return f"{folder.name} [{', '.join(parts)}]"


def find_backup_critical_dirs(base_dir: Path) -> List[Path]:
    return sorted(
        [
            directory
            for directory in base_dir.iterdir()
            if directory.is_dir()
            and directory.name.startswith("backup_critical")
            and any(directory.glob("*.img"))
        ],
        key=lambda directory: directory.name,
    )


def find_dp_source_folders(base_dir: Path, output_dp_dir: Path) -> List[Path]:
    folders = list(find_backup_critical_dirs(base_dir))
    if output_dp_dir.exists() and any(output_dp_dir.glob("*.img")):
        folders.append(output_dp_dir)
    return folders
