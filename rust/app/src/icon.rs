//! The tray icon (section 9 of `docs/design/ui-handoff.md`): a monitor glyph drawn on a 16 x 16
//! grid, with the aggregate PC state as the screen fill and the aggregate tunnel state as the
//! corner square (the owner's decision of 13.11.2026, which swaps section 9 of the handoff). [`render`] is pure: it returns the pixels, [`to_image`] turns them into a Slint image.
//!
//! Coordinates follow the handoff. A rectangle edge at `N.5` is a 1 px stroke on pixel `N`, and
//! a fill starts at its integer coordinate. The stand is drawn in column 7 (the handoff gives
//! x = 8, which is not a whole pixel) so that it stays centred on the glyph. The corner square
//! is a 4 x 4 fill inside a 1 px ring of the taskbar background, on columns 10 to 15 and rows 9
//! to 14. Scaling to 24 and 32 px is nearest-neighbour, so every stroke stays hard-edged.

use homelink_core::snapshot::{DisplayState, TunnelCorner};

/// The side of the grid the glyph is drawn on.
const GRID: usize = 16;

/// One colour per taskbar theme. The state colours are those of the matching application theme
/// (section 2 of the handoff, and its dark addendum).
struct Palette {
    outline: [u8; 3],
    ring: [u8; 3],
    ok: [u8; 3],
    busy: [u8; 3],
    off: [u8; 3],
    fail: [u8; 3],
}

/// A light taskbar. The background of the taskbar is not in the handoff; this is an assumption.
const LIGHT: Palette = Palette {
    outline: [0x1A, 0x1A, 0x1A],
    ring: [0xF3, 0xF3, 0xF3],
    ok: [0x15, 0x80, 0x3D],
    busy: [0xE8, 0xA3, 0x17],
    off: [0x1F, 0x4F, 0xD8],
    fail: [0xC4, 0x28, 0x1C],
};

/// A dark taskbar. The background is an assumption, as for [`LIGHT`].
const DARK: Palette = Palette {
    outline: [0xF2, 0xF2, 0xF2],
    ring: [0x1F, 0x1F, 0x1F],
    ok: [0x22, 0xC5, 0x5E],
    busy: [0xFA, 0xCC, 0x15],
    off: [0x4A, 0x78, 0xFF],
    fail: [0xF0, 0x55, 0x4A],
};

/// An icon as RGBA pixels, row by row, `size` x `size`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bitmap {
    size: u32,
    pixels: Vec<[u8; 4]>,
}

/// One grid cell: `None` is transparent.
type Grid = Vec<Option<[u8; 4]>>;

fn opaque(colour: [u8; 3]) -> Option<[u8; 4]> {
    Some([colour[0], colour[1], colour[2], 0xFF])
}

/// Sets one cell; a coordinate outside the grid is ignored.
fn set(grid: &mut Grid, x: usize, y: usize, colour: Option<[u8; 4]>) {
    if x < GRID
        && y < GRID
        && let Some(cell) = grid.get_mut(y * GRID + x)
    {
        *cell = colour;
    }
}

/// The colour of a screen fill, `None` for "unknown" (outline only).
fn screen_colour(state: DisplayState, palette: &Palette) -> Option<[u8; 3]> {
    match state {
        DisplayState::Ok => Some(palette.ok),
        DisplayState::Busy => Some(palette.busy),
        DisplayState::Off => Some(palette.off),
        DisplayState::Fail => Some(palette.fail),
        DisplayState::Unknown => None,
    }
}

/// Draws the glyph on the 16 x 16 grid.
fn draw_grid(screen: DisplayState, corner: TunnelCorner, palette: &Palette) -> Grid {
    let mut grid: Grid = vec![None; GRID * GRID];
    let outline = opaque(palette.outline);

    // Outline: x 1.5 to 14.5 and y 2.5 to 11.5, 1 px stroke.
    for x in 1..=14 {
        set(&mut grid, x, 2, outline);
        set(&mut grid, x, 11, outline);
    }
    for y in 2..=11 {
        set(&mut grid, 1, y, outline);
        set(&mut grid, 14, y, outline);
    }

    // Screen fill: x 3, y 4, 10 x 6.
    if let Some(colour) = screen_colour(screen, palette) {
        for y in 4..10 {
            for x in 3..13 {
                set(&mut grid, x, y, opaque(colour));
            }
        }
    }

    // Stand from y 12 to 14, base at y 14 from x 5 to 11.
    set(&mut grid, 7, 12, outline);
    set(&mut grid, 7, 13, outline);
    for x in 5..11 {
        set(&mut grid, x, 14, outline);
    }

    // Corner square: a 1 px ring of the taskbar background around a 4 x 4 fill.
    let fill = match corner {
        TunnelCorner::None => None,
        TunnelCorner::Ok => Some(palette.ok),
        TunnelCorner::Busy => Some(palette.busy),
        TunnelCorner::Fail => Some(palette.fail),
    };
    if let Some(fill) = fill {
        let ring = opaque(palette.ring);
        for x in 10..=15 {
            set(&mut grid, x, 9, ring);
            set(&mut grid, x, 14, ring);
        }
        for y in 9..=14 {
            set(&mut grid, 10, y, ring);
            set(&mut grid, 15, y, ring);
        }
        for y in 10..14 {
            for x in 11..15 {
                set(&mut grid, x, y, opaque(fill));
            }
        }
    }
    grid
}

/// The icon of a tray state, `size` pixels square, for a light or a dark taskbar.
pub(crate) fn render(
    screen: DisplayState,
    corner: TunnelCorner,
    taskbar_dark: bool,
    size: u32,
) -> Bitmap {
    let palette = if taskbar_dark { &DARK } else { &LIGHT };
    let grid = draw_grid(screen, corner, palette);
    let side = usize::try_from(size.max(1)).unwrap_or(GRID);
    let mut pixels = Vec::with_capacity(side * side);
    for y in 0..side {
        for x in 0..side {
            // Nearest neighbour: the grid cell that covers this pixel.
            let gx = x * GRID / side;
            let gy = y * GRID / side;
            let cell = grid.get(gy * GRID + gx).copied().flatten();
            pixels.push(cell.unwrap_or([0, 0, 0, 0]));
        }
    }
    Bitmap {
        size: u32::try_from(side).unwrap_or(size),
        pixels,
    }
}

/// The image Slint shows as the tray icon.
pub(crate) fn to_image(bitmap: &Bitmap) -> slint::Image {
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(bitmap.size, bitmap.size);
    for (target, [r, g, b, a]) in buffer.make_mut_slice().iter_mut().zip(&bitmap.pixels) {
        *target = slint::Rgba8Pixel {
            r: *r,
            g: *g,
            b: *b,
            a: *a,
        };
    }
    slint::Image::from_rgba8(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: [u8; 4] = [0x15, 0x80, 0x3D, 0xFF];
    const FAIL_LIGHT: [u8; 4] = [0xC4, 0x28, 0x1C, 0xFF];
    const INK_LIGHT: [u8; 4] = [0x1A, 0x1A, 0x1A, 0xFF];
    const RING_LIGHT: [u8; 4] = [0xF3, 0xF3, 0xF3, 0xFF];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    /// The pixel at (`x`, `y`); a marker value outside the bitmap, so that a test fails.
    fn at(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let index = usize::try_from(y * b.size + x).unwrap_or(usize::MAX);
        b.pixels.get(index).copied().unwrap_or([9, 9, 9, 9])
    }

    #[test]
    fn the_outline_and_the_screen_fill_are_where_the_handoff_puts_them() {
        let b = render(DisplayState::Ok, TunnelCorner::None, false, 16);
        assert_eq!(at(&b, 1, 2), INK_LIGHT, "outline corner");
        assert_eq!(at(&b, 14, 11), INK_LIGHT, "outline corner");
        assert_eq!(at(&b, 2, 4), CLEAR, "gap inside the outline");
        assert_eq!(at(&b, 3, 4), OK, "screen fill, top left");
        assert_eq!(at(&b, 12, 9), OK, "screen fill, bottom right");
        assert_eq!(at(&b, 13, 9), CLEAR, "past the screen fill");
        assert_eq!(at(&b, 7, 12), INK_LIGHT, "stand");
        assert_eq!(at(&b, 5, 14), INK_LIGHT, "base left");
        assert_eq!(at(&b, 10, 14), INK_LIGHT, "base right");
    }

    #[test]
    fn unknown_has_an_outline_and_no_fill() {
        let b = render(DisplayState::Unknown, TunnelCorner::None, false, 16);
        assert_eq!(at(&b, 3, 4), CLEAR);
        assert_eq!(at(&b, 1, 2), INK_LIGHT);
    }

    #[test]
    fn a_stopped_tunnel_and_an_unknown_pc_leave_only_the_outline() {
        let b = render(DisplayState::Unknown, TunnelCorner::None, false, 16);
        assert_eq!(at(&b, 3, 4), CLEAR, "no screen fill");
        assert_eq!(at(&b, 12, 12), CLEAR, "no corner square");
        assert_eq!(at(&b, 10, 9), CLEAR, "no ring");
        assert_eq!(at(&b, 1, 2), INK_LIGHT);
    }

    #[test]
    fn an_up_tunnel_and_an_online_pc_make_a_green_screen_with_a_green_square() {
        let b = render(DisplayState::Ok, TunnelCorner::Ok, false, 16);
        assert_eq!(at(&b, 5, 6), OK, "screen");
        assert_eq!(at(&b, 12, 12), OK, "corner square");
        assert_eq!(at(&b, 10, 9), RING_LIGHT, "ring");
    }

    #[test]
    fn the_corner_ring_and_fill_overlap_the_glyph_in_order() {
        let b = render(DisplayState::Ok, TunnelCorner::Fail, false, 16);
        // Ring over the screen fill (row 9, columns 10 to 12).
        assert_eq!(at(&b, 10, 9), RING_LIGHT);
        assert_eq!(at(&b, 12, 9), RING_LIGHT);
        // Ring over the base line (row 14, column 10).
        assert_eq!(at(&b, 10, 14), RING_LIGHT);
        // Fill over the outline (column 14, rows 10 and 11).
        assert_eq!(at(&b, 14, 10), FAIL_LIGHT);
        assert_eq!(at(&b, 14, 11), FAIL_LIGHT);
        // Inside the fill, and the ring beyond it.
        assert_eq!(at(&b, 11, 10), FAIL_LIGHT);
        assert_eq!(at(&b, 15, 12), RING_LIGHT);
        assert_eq!(at(&b, 9, 15), CLEAR, "outside the ring");
    }

    #[test]
    fn every_state_has_its_colour_on_both_taskbars() {
        // The screen fill at pixel (5, 6) of the 16 px icon, and the corner fill at (12, 12).
        let screens = [
            (DisplayState::Ok, [0x15, 0x80, 0x3D], [0x22, 0xC5, 0x5E]),
            (DisplayState::Busy, [0xE8, 0xA3, 0x17], [0xFA, 0xCC, 0x15]),
            (DisplayState::Off, [0x1F, 0x4F, 0xD8], [0x4A, 0x78, 0xFF]),
            (DisplayState::Fail, [0xC4, 0x28, 0x1C], [0xF0, 0x55, 0x4A]),
        ];
        for (state, light, dark) in screens {
            let l = render(state, TunnelCorner::None, false, 16);
            assert_eq!(at(&l, 5, 6), with_alpha(light), "{state:?} on light");
            let d = render(state, TunnelCorner::None, true, 16);
            assert_eq!(at(&d, 5, 6), with_alpha(dark), "{state:?} on dark");
        }
        let unknown = render(DisplayState::Unknown, TunnelCorner::None, true, 16);
        assert_eq!(at(&unknown, 5, 6), CLEAR, "unknown has no fill");
        let corners = [
            (TunnelCorner::Ok, [0x15, 0x80, 0x3D], [0x22, 0xC5, 0x5E]),
            (TunnelCorner::Busy, [0xE8, 0xA3, 0x17], [0xFA, 0xCC, 0x15]),
            (TunnelCorner::Fail, [0xC4, 0x28, 0x1C], [0xF0, 0x55, 0x4A]),
        ];
        for (corner, light, dark) in corners {
            let l = render(DisplayState::Off, corner, false, 16);
            assert_eq!(at(&l, 12, 12), with_alpha(light), "{corner:?} on light");
            let d = render(DisplayState::Off, corner, true, 16);
            assert_eq!(at(&d, 12, 12), with_alpha(dark), "{corner:?} on dark");
        }
    }

    fn with_alpha([r, g, b]: [u8; 3]) -> [u8; 4] {
        [r, g, b, 0xFF]
    }

    #[test]
    fn a_busy_tunnel_uses_the_busy_colour() {
        let b = render(DisplayState::Off, TunnelCorner::Busy, false, 16);
        assert_eq!(at(&b, 12, 13), [0xE8, 0xA3, 0x17, 0xFF]);
    }

    #[test]
    fn a_dark_taskbar_has_light_outline_and_dark_state_colours() {
        let b = render(DisplayState::Fail, TunnelCorner::None, true, 16);
        assert_eq!(at(&b, 1, 2), [0xF2, 0xF2, 0xF2, 0xFF]);
        assert_eq!(at(&b, 3, 4), [0xF0, 0x55, 0x4A, 0xFF]);
    }

    #[test]
    fn scaling_keeps_each_grid_cell_whole() {
        let b32 = render(DisplayState::Ok, TunnelCorner::None, false, 32);
        // Grid cell (3, 4) is pixels 6..8 by 8..10 at 32 px.
        assert_eq!(at(&b32, 6, 8), OK);
        assert_eq!(at(&b32, 7, 9), OK);
        assert_eq!(at(&b32, 5, 8), CLEAR);
        let b24 = render(DisplayState::Ok, TunnelCorner::None, false, 24);
        // At 24 px, pixel 5 maps to grid column 3 and pixel 6 to grid row 4.
        assert_eq!(at(&b24, 5, 6), OK);
        assert_eq!(at(&b24, 4, 6), CLEAR);
        assert_eq!(b24.size, 24);
        assert_eq!(b32.size, 32);
        assert_eq!(b24.pixels.len(), 24 * 24);
    }

    #[test]
    fn the_sizes_are_the_three_renditions() {
        for size in [16, 24, 32] {
            let b = render(DisplayState::Busy, TunnelCorner::Busy, true, size);
            assert_eq!(b.size, size);
            assert_eq!(b.pixels.len(), usize::try_from(size * size).unwrap_or(0));
        }
    }
}
