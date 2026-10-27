"""Draws the Levitas logo: a flat floating block and a geometric wordmark.

Everything is drawn as paths (no fonts), so it looks the same everywhere, including on
GitHub, which shows SVGs as images. Run `python tools/logo.py` to rewrite the files in
docs/assets:

- logo.svg: the mark alone, on a dark rounded square (avatars, icons);
- logo-light.svg / logo-dark.svg: mark and wordmark side by side, for light and dark
  backgrounds.
"""

from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "docs" / "assets"

# Three tones of one orange, lightest on top, like light falling from above.
TOP, LEFT, RIGHT = "#FF9A3C", "#F2600C", "#B8420A"

# Letters drawn with one heavy stroke on a 100-unit cap height.
STROKE = 21
LETTERS = {
    # name: (advance width, path)
    "L": (56, "M10.5 0 V89.5 H56"),
    "E": (56, "M56 10.5 H10.5 V89.5 H56 M10.5 50 H48"),
    "V": (74, "M0 0 L37 100 L74 0"),
    "I": (21, "M10.5 0 V100"),
    "T": (66, "M0 10.5 H66 M33 10.5 V100"),
    "A": (74, "M0 100 L37 0 L74 100 M16 66 H58"),
    "S": (
        60,
        "M60 10.5 H23 A20 20 0 0 0 23 50.5 H37 A19.5 19.5 0 0 1 37 89.5 H0",
    ),
}
GAP = 16


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


def wordmark(x, y, height, color):
    scale = height / 100.0
    parts = []
    cx = 0.0
    for ch in "LEVITAS":
        width, path = LETTERS[ch]
        parts.append(f'<path transform="translate({cx:g} 0)" d="{path}"/>')
        cx += width + GAP
    total = (cx - GAP) * scale
    # Pointed joins (the V's tip, the A's apex) are cut flat at the cap height and the
    # baseline, like a geometric typeface.
    group = (
        f'<g transform="translate({x:g} {y:g}) scale({scale:g})">'
        f'<clipPath id="caps"><rect x="-50" y="0" width="{cx + 100:g}" height="100"/></clipPath>'
        f'<g clip-path="url(#caps)" fill="none" stroke="{color}" stroke-width="{STROKE}" '
        f'stroke-linejoin="miter" stroke-miterlimit="10" stroke-linecap="butt">'
        + "".join(parts)
        + "</g></g>"
    )
    return group, total


def horizontal(text_color):
    mark_size = 120
    word_height = 74
    word, word_width = wordmark(0, 0, word_height, text_color)
    pad = 20
    gap = 34
    width = pad + mark_size + gap + word_width + pad
    height = 160
    word_y = (height - word_height) / 2
    word, _ = wordmark(pad + mark_size + gap, word_y, word_height, text_color)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0f}" height="{height}" '
        f'viewBox="0 0 {width:.0f} {height}" role="img" aria-label="Levitas">'
        f"<title>Levitas</title>"
        + mark(pad, (height - mark_size) / 2 - 6, mark_size)
        + word
        + "</svg>\n"
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
    files = {
        "logo.svg": square(),
        "logo-light.svg": horizontal("#16181D"),
        "logo-dark.svg": horizontal("#F3F4F6"),
    }
    for name, svg in files.items():
        (OUT / name).write_text(svg, encoding="utf-8", newline="\n")
        print(f"wrote {OUT / name}")


if __name__ == "__main__":
    main()
