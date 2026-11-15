"""Launcher for running from source without installation and for PyInstaller."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "src"))

from homelink.__main__ import main  # noqa: E402

sys.exit(main())
