"""Remote Desktop connection file.

The file is written only when it does not exist: a signed .rdp file must not be modified.
"""

import subprocess
from collections.abc import Iterable
from pathlib import Path


def ensure_rdp_file(path: Path, host: str, user: str, options: Iterable[str]) -> Path:
    if not path.exists():
        lines = [f"full address:s:{host}", f"username:s:{user}", *options]
        path.write_text("\r\n".join(lines) + "\r\n", encoding="utf-16", newline="")
    return path


def open_rdp(path: Path) -> None:
    subprocess.Popen(["mstsc", str(path)])
