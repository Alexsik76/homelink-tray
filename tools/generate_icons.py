"""Generate the built-in tray icons and app.ico.

A development tool: the program itself only loads the resulting PNG files.
Usage: python tools/generate_icons.py
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ASSETS = Path(__file__).resolve().parent.parent / "src" / "homelink" / "assets"
ICON_SIZE = 64
ICO_SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]

SS = 4  # supersampling factor

PC_COLORS = {
    "online": ((52, 211, 153), (16, 150, 100)),
    "waking": ((251, 191, 36), (217, 119, 6)),
    "offline": ((100, 116, 139), (51, 65, 85)),
    "unknown": ((71, 85, 105), (30, 41, 59)),
}
TUNNEL_COLORS = {
    "up": (56, 189, 248),
    "starting": (251, 191, 36),
    "down": (239, 68, 68),
}


def _vgrad(size, top, bottom):
    w, h = size
    grad = Image.new("RGBA", (1, h))
    for y in range(h):
        t = y / max(h - 1, 1)
        grad.putpixel((0, y), (*(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3)), 255))
    return grad.resize((w, h))


def render(pc_state: str, tunnel_state: str, size: int = 64) -> Image.Image:
    S = size * SS
    u = S / 64.0
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # monitor body
    body = (4 * u, 8 * u, 60 * u, 46 * u)
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle(
        (body[0], body[1] + 2 * u, body[2], body[3] + 2 * u), radius=7 * u, fill=(0, 0, 0, 110)
    )
    img = Image.alpha_composite(img, shadow.filter(ImageFilter.GaussianBlur(2 * u)))

    d = ImageDraw.Draw(img)
    d.rounded_rectangle(
        body, radius=7 * u, fill=(30, 35, 45, 255), outline=(120, 132, 150, 255), width=int(1.6 * u)
    )

    # screen with gradient
    top, bottom = PC_COLORS.get(pc_state, PC_COLORS["unknown"])
    scr = (9 * u, 13 * u, 55 * u, 41 * u)
    sw, sh = int(scr[2] - scr[0]), int(scr[3] - scr[1])
    grad = _vgrad((sw, sh), top, bottom)
    mask = Image.new("L", (sw, sh), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, sw - 1, sh - 1), radius=4 * u, fill=255)
    img.paste(grad, (int(scr[0]), int(scr[1])), mask)

    # glossy highlight
    gloss = Image.new("RGBA", (sw, sh), (0, 0, 0, 0))
    ImageDraw.Draw(gloss).polygon(
        [(0, 0), (sw * 0.65, 0), (sw * 0.25, sh), (0, sh)], fill=(255, 255, 255, 38)
    )
    gl_mask = Image.new("L", (sw, sh), 0)
    ImageDraw.Draw(gl_mask).rounded_rectangle((0, 0, sw - 1, sh - 1), radius=4 * u, fill=255)
    layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    layer.paste(gloss, (int(scr[0]), int(scr[1])), gl_mask)
    img = Image.alpha_composite(img, layer)
    d = ImageDraw.Draw(img)

    # power glyph on the screen
    cx, cy, r = 32 * u, 27 * u, 7.5 * u
    glyph = (255, 255, 255, 235) if pc_state != "unknown" else (255, 255, 255, 120)
    d.arc((cx - r, cy - r, cx + r, cy + r), start=-60, end=240, fill=glyph, width=int(2.6 * u))
    d.line((cx, cy - r - 1.5 * u, cx, cy - 1 * u), fill=glyph, width=int(2.6 * u))

    # stand
    d.polygon(
        [(27 * u, 46 * u), (37 * u, 46 * u), (39 * u, 53 * u), (25 * u, 53 * u)],
        fill=(30, 35, 45, 255),
    )
    d.rounded_rectangle((18 * u, 52 * u, 46 * u, 57 * u), radius=2.5 * u, fill=(30, 35, 45, 255))

    # tunnel badge
    bc = TUNNEL_COLORS.get(tunnel_state, TUNNEL_COLORS["down"])
    bx, by, br = 50 * u, 50 * u, 12 * u
    d.ellipse(
        (bx - br - 2 * u, by - br - 2 * u, bx + br + 2 * u, by + br + 2 * u), fill=(15, 18, 24, 255)
    )
    d.ellipse((bx - br, by - br, bx + br, by + br), fill=(*bc, 255))
    # badge glyph: two arrows (up), three dots (starting), cross (down)
    w = int(2.4 * u)
    if tunnel_state == "up":
        d.line((bx - 6 * u, by - 2.5 * u, bx + 6 * u, by - 2.5 * u), fill="white", width=w)
        d.line((bx + 2.5 * u, by - 6 * u, bx + 6.2 * u, by - 2.5 * u), fill="white", width=w)
        d.line((bx - 6 * u, by + 3 * u, bx + 6 * u, by + 3 * u), fill="white", width=w)
        d.line((bx - 2.5 * u, by + 6.5 * u, bx - 6.2 * u, by + 3 * u), fill="white", width=w)
    elif tunnel_state == "starting":
        for dx in (-5, 0, 5):
            d.ellipse(
                (bx + dx * u - 1.8 * u, by - 1.8 * u, bx + dx * u + 1.8 * u, by + 1.8 * u),
                fill="white",
            )
    else:
        d.line((bx - 5 * u, by - 5 * u, bx + 5 * u, by + 5 * u), fill="white", width=w)
        d.line((bx - 5 * u, by + 5 * u, bx + 5 * u, by - 5 * u), fill="white", width=w)

    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    icons = ASSETS / "icons"
    icons.mkdir(parents=True, exist_ok=True)
    for pc in PC_COLORS:
        for tunnel in TUNNEL_COLORS:
            render(pc, tunnel, ICON_SIZE).save(icons / f"{pc}_{tunnel}.png")
    render("online", "up", 256).save(ASSETS / "app.ico", sizes=ICO_SIZES)


if __name__ == "__main__":
    main()
