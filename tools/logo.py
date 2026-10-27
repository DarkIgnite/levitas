"""Draws the Levitas mark, a flat floating block, as docs/assets/logo.svg: the mark on
a dark rounded square, for avatars and icons. Run `python tools/logo.py` to rewrite it.

The README's logo (the mark next to the blocky wordmark) is made by
tools/logo_lockup.py.
"""

from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "docs" / "assets"

# Three tones of one orange, lightest on top, like light falling from above.
TOP, LEFT, RIGHT = "#FF9A3C", "#F2600C", "#B8420A"

def mark(x, y, size):
    """The floating block: an isometric cube in three flat tones with thin gaps between
    the faces, and a short shadow line under it."""
    s = size / 100.0

    def p(px, py):
        return f"{x + px * s:.2f} {y + py * s:.2f}"

    g = 2.2  # half the gap between faces
    top = f"M{p(50, 4)} L{p(91, 27.5 - g)} L{p(50, 51 - 2 * g)} L{p(9, 27.5 - g)} Z"
    left = f"M{p(9 - 0, 27.5 + g)} L{p(50 - g, 51)} L{p(50 - g, 92)} L{p(9, 68.5)} Z"
    right = f"M{p(91, 27.5 + g)} L{p(50 + g, 51)} L{p(50 + g, 92)} L{p(91, 68.5)} Z"
    shadow = f"M{p(30, 103)} H{x + 70 * s:.2f}"
    return (
        f'<path d="{top}" fill="{TOP}"/>'
        f'<path d="{left}" fill="{LEFT}"/>'
        f'<path d="{right}" fill="{RIGHT}"/>'
        f'<path d="{shadow}" stroke="{LEFT}" stroke-width="{4.5 * s:.2f}" '
        f'stroke-linecap="round" opacity="0.55"/>'
    )


def square():
    size = 256
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" '
        f'viewBox="0 0 {size} {size}" role="img" aria-label="Levitas">'
        f"<title>Levitas</title>"
        f'<rect width="{size}" height="{size}" rx="56" fill="#16181D"/>'
        + mark(48, 40, 160)
        + "</svg>\n"
    )


def main():
    files = {"logo.svg": square()}
    for name, svg in files.items():
        (OUT / name).write_text(svg, encoding="utf-8", newline="\n")
        print(f"wrote {OUT / name}")


if __name__ == "__main__":
    main()
