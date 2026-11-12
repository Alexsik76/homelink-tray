"""Tray icons: one PNG file per combination of PC state and tunnel state.

File name pattern: ``<pc state>_<tunnel state>.png``, for example ``online_up.png``.
The built-in sets live in one folder per taskbar theme (``light`` and ``dark``) and
can be replaced through the ``icons_dir`` setting.
"""

from pathlib import Path

from PIL import Image

from homelink.state import PcState, Status, TunnelState

ICONS_ROOT = Path(__file__).resolve().parent / "assets" / "icons"
THEMES = ("light", "dark")


def builtin_dir(theme: str) -> Path:
    """Folder of the built-in set for a taskbar theme."""
    if theme not in THEMES:
        raise ValueError(f"unknown icon theme '{theme}' (available: {', '.join(THEMES)})")
    return ICONS_ROOT / theme


def icon_name(status: Status) -> str:
    return f"{status.pc}_{status.tunnel}.png"


class IconSet:
    def __init__(self, directory: Path):
        self._images: dict[Status, Image.Image] = {}
        for pc in PcState:
            for tunnel in TunnelState:
                status = Status(tunnel=tunnel, pc=pc)
                path = directory / icon_name(status)
                with Image.open(path) as image:
                    self._images[status] = image.convert("RGBA")

    def get(self, status: Status) -> Image.Image:
        return self._images[status]
