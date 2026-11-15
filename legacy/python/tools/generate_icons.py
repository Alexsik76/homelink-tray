"""Generate the built-in tray icons, app.ico and a contact sheet for review.

A development tool: the program itself only loads the resulting PNG files.
Usage: python tools/generate_icons.py

The artwork is drawn on a 64x64 grid (SVG conventions, y grows downwards) at
SS times the final size and downscaled with LANCZOS.
"""

import math
import shutil
from collections.abc import Sequence
from itertools import pairwise
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "src" / "homelink" / "assets"
PREVIEW = ROOT / "build" / "icon-preview.png"
ICON_SIZE = 64
ICO_SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
FONT_PATH = Path(r"C:\Windows\Fonts\segoeuib.ttf")

SS = 8  # supersampling factor

Point = tuple[float, float]

WHITE = "#ffffff"
DARK_GLYPH = "#1e293b"

# colour of the screen by PC state
SCREEN_COLORS = {
    "online": "#22c55e",
    "waking": "#f59e0b",
    "offline": "#a3a3a3",
    "unknown": "#52525b",
}
UNKNOWN_GLYPH = "#f1f5f9"
# (badge, arrow) colours by tunnel state
TUNNEL_COLORS = {
    "up": ("#2563eb", WHITE),
    "starting": ("#facc15", DARK_GLYPH),
    "down": ("#dc2626", WHITE),
}
# (body, ring) colours by theme: "light" is for a light taskbar, "dark" for a dark one
THEMES = {
    "light": ("#1e293b", "#ffffff"),
    "dark": ("#e5e7eb", "#1c1c1c"),
}
TASKBAR_BACKGROUNDS = {"light": "#f3f3f3", "dark": "#1c1c1c"}

APP_BLUE = "#2563eb"

# two opposing arrows in a local frame centred on the badge (or on the app icon):
# (stroke width, lines, triangles)
BADGE_ARROWS = (
    3.0,
    [((-9, -4), (3, -4)), ((9, 4), (-3, 4))],
    [[(2, -9), (10, -4), (2, 1)], [(-2, -1), (-10, 4), (-2, 9)]],
)
APP_ARROWS = (
    5.5,
    [((-17, -7), (6, -7)), ((17, 7), (-6, 7))],
    [[(4, -16), (19, -7), (4, 2)], [(-4, -2), (-19, 7), (-4, 16)]],
)


# ---------- drawing helpers (coordinates in 64-unit space, u = pixels per unit) ----------
def _stroke(d: ImageDraw.ImageDraw, points: Sequence[Point], width: float, color: str, u: float):
    """A polyline with round caps and joins."""
    r = width * u / 2
    scaled = [(x * u, y * u) for x, y in points]
    for (x0, y0), (x1, y1) in pairwise(scaled):
        steps = max(int(math.hypot(x1 - x0, y1 - y0)), 1)
        for i in range(steps + 1):
            t = i / steps
            x, y = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t
            d.ellipse((x - r, y - r, x + r, y + r), fill=color)


def _flat_line(d: ImageDraw.ImageDraw, p0: Point, p1: Point, width: float, color: str, u: float):
    """A straight line with butt caps."""
    (x0, y0), (x1, y1) = p0, p1
    length = math.hypot(x1 - x0, y1 - y0)
    nx, ny = -(y1 - y0) / length * width / 2, (x1 - x0) / length * width / 2
    corners = [(x0 + nx, y0 + ny), (x1 + nx, y1 + ny), (x1 - nx, y1 - ny), (x0 - nx, y0 - ny)]
    d.polygon([(x * u, y * u) for x, y in corners], fill=color)


def _circle(d: ImageDraw.ImageDraw, center: Point, radius: float, color: str, u: float):
    (cx, cy), r = center, radius
    d.ellipse(((cx - r) * u, (cy - r) * u, (cx + r) * u, (cy + r) * u), fill=color)


def _rect(d: ImageDraw.ImageDraw, x: float, y: float, w: float, h: float, r: float, color, u):
    d.rounded_rectangle((x * u, y * u, (x + w) * u, (y + h) * u), radius=r * u, fill=color)


def _rotated(points: Sequence[Point], center: Point, degrees: float = -45.0) -> list[Point]:
    """Rotate local-frame points about the frame origin placed at ``center`` (SVG rotate())."""
    a = math.radians(degrees)
    cos, sin = math.cos(a), math.sin(a)
    cx, cy = center
    return [(cx + x * cos - y * sin, cy + x * sin + y * cos) for x, y in points]


def _arrows(d: ImageDraw.ImageDraw, u: float, center: Point, spec, color: str) -> None:
    width, lines, triangles = spec
    for line in lines:
        _stroke(d, _rotated(line, center), width, color, u)
    for triangle in triangles:
        d.polygon([(x * u, y * u) for x, y in _rotated(triangle, center)], fill=color)


def _unknown_glyph(d: ImageDraw.ImageDraw, u: float) -> None:
    size = round(20 * u)
    try:
        font = ImageFont.truetype(str(FONT_PATH), size)
    except OSError:
        font = ImageFont.load_default(size=size)
    d.text((22 * u, 30 * u), "?", font=font, fill=UNKNOWN_GLYPH, anchor="ms")


def _pc_glyph(d: ImageDraw.ImageDraw, pc_state: str, u: float) -> None:
    if pc_state == "online":
        _stroke(d, [(14, 22), (20, 28), (31, 16)], 4, WHITE, u)
    elif pc_state == "waking":
        for x in (14, 22, 30):
            _circle(d, (x, 23), 2.6, DARK_GLYPH, u)
    elif pc_state == "offline":
        centre, radius = (22, 23), 7
        start = math.atan2(17.5 - 23, 17.5 - 22)  # upper left, the long way round to upper right
        end = math.atan2(17.5 - 23, 26.5 - 22) - 2 * math.pi
        steps = 90
        arc = [
            (
                centre[0] + radius * math.cos(start + (end - start) * i / steps),
                centre[1] + radius * math.sin(start + (end - start) * i / steps),
            )
            for i in range(steps + 1)
        ]
        _stroke(d, arc, 3.2, DARK_GLYPH, u)
        _stroke(d, [(22, 13), (22, 21)], 3.2, DARK_GLYPH, u)
    else:
        _unknown_glyph(d, u)


def render(pc_state: str, tunnel_state: str, theme: str, size: int = ICON_SIZE) -> Image.Image:
    body, ring = THEMES[theme]
    badge, arrow = TUNNEL_COLORS[tunnel_state]
    s = size * SS
    u = s / 64.0
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    _rect(d, 3, 5, 46, 35, 5, body, u)  # monitor body
    _rect(d, 7, 9, 38, 27, 2, SCREEN_COLORS[pc_state], u)  # screen
    d.rectangle((21 * u, 40 * u, 31 * u, 46 * u), fill=body)  # neck
    _rect(d, 13, 45, 26, 5, 2.5, body, u)  # base
    _pc_glyph(d, pc_state, u)

    # tunnel badge: the outline is centred on the circle's edge, as an SVG stroke is
    centre = (46, 46)
    _circle(d, centre, 16 + 1.5, ring, u)
    _circle(d, centre, 16 - 1.5, badge, u)
    _arrows(d, u, centre, BADGE_ARROWS, arrow)
    if tunnel_state == "down":
        _flat_line(d, *_rotated([(0, -13), (0, 13)], centre), 6.5, badge, u)
        _stroke(d, _rotated([(0, -12), (0, 12)], centre), 2.6, WHITE, u)

    return img.resize((size, size), Image.LANCZOS)


def render_app_icon(size: int) -> Image.Image:
    s = size * SS
    u = s / 64.0
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    _rect(d, 2, 2, 60, 60, 14, APP_BLUE, u)
    _arrows(d, u, (32, 32), APP_ARROWS, WHITE)
    return img.resize((size, size), Image.LANCZOS)


# ---------- contact sheet ----------
SHEET_ICONS = [
    ("unknown", "down"),
    ("unknown", "starting"),
    ("unknown", "up"),
    ("offline", "up"),
    ("waking", "up"),
    ("online", "up"),
]


def contact_sheet(app_icon: Image.Image) -> Image.Image:
    margin, gap, inner, row_height = 24, 24, 12, 112
    group_width = 64 + inner + 32 + inner + 16
    columns = len(SHEET_ICONS) + 1
    width = 2 * margin + columns * group_width + (columns - 1) * gap
    sheet = Image.new("RGBA", (width, row_height * len(THEMES)), (0, 0, 0, 0))

    for row, theme in enumerate(THEMES):
        top = row * row_height
        sheet.paste(Image.new("RGBA", (width, row_height), TASKBAR_BACKGROUNDS[theme]), (0, top))
        images = [
            Image.open(ASSETS / "icons" / theme / f"{pc}_{tunnel}.png").convert("RGBA")
            for pc, tunnel in SHEET_ICONS
        ]
        if row == 0:
            images.append(app_icon)
        for column, image in enumerate(images):
            x = margin + column * (group_width + gap)
            for shown, size in ((64, 64), (32, 32), (16, 16)):
                small = image.resize((size, size), Image.LANCZOS)
                y = top + (row_height - shown) // 2
                sheet.alpha_composite(small, (x, y))
                x += size + inner
    return sheet


def main() -> None:
    for theme in THEMES:
        folder = ASSETS / "icons" / theme
        folder.mkdir(parents=True, exist_ok=True)
        for tunnel in TUNNEL_COLORS:
            # the PC state is only shown while the tunnel is up
            pcs = SCREEN_COLORS if tunnel == "up" else ["unknown"]
            for pc in pcs:
                render(pc, tunnel, theme).save(folder / f"{pc}_{tunnel}.png")
            if tunnel != "up":
                reference = folder / f"unknown_{tunnel}.png"
                for pc in SCREEN_COLORS:
                    if pc != "unknown":
                        shutil.copyfile(reference, folder / f"{pc}_{tunnel}.png")

    render_app_icon(256).save(ASSETS / "app.ico", sizes=ICO_SIZES)

    PREVIEW.parent.mkdir(parents=True, exist_ok=True)
    contact_sheet(render_app_icon(64)).save(PREVIEW)


if __name__ == "__main__":
    main()
