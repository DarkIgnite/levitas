"""Puts the floating block next to the blocky LEVITAS wordmark, for light and dark
backgrounds.

Input: docs/assets/wordmark.png (white letters, black outline, grey sides; transparent
background). Output: docs/assets/logo-light.png and docs/assets/logo-dark.png. On dark
backgrounds the wordmark's black outline would disappear, so the dark version sets the
letters on an orange rim. Needs Pillow (`pip install pillow`).
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ASSETS = Path(__file__).resolve().parent.parent / "docs" / "assets"
TOP, LEFT, RIGHT = (255, 154, 60), (242, 96, 12), (184, 66, 10)
RIM = (247, 76, 0)
SUPERSAMPLE = 4


def block(size):
    """The floating block: three flat faces with thin gaps and a shadow line under it,
    drawn large and scaled down for smooth edges."""
    s = size * SUPERSAMPLE / 100.0
    canvas = Image.new("RGBA", (size * SUPERSAMPLE, int(size * 1.12) * SUPERSAMPLE), (0, 0, 0, 0))
    d = ImageDraw.Draw(canvas)
    g = 2.2

    def p(x, y):
        return (x * s, y * s)

    d.polygon([p(50, 4), p(91, 27.5 - g), p(50, 51 - 2 * g), p(9, 27.5 - g)], fill=TOP)
    d.polygon([p(9, 27.5 + g), p(50 - g, 51), p(50 - g, 92), p(9, 68.5)], fill=LEFT)
    d.polygon([p(91, 27.5 + g), p(50 + g, 51), p(50 + g, 92), p(91, 68.5)], fill=RIGHT)
    d.rounded_rectangle(
        [p(30, 100.75), p(70, 105.25)], radius=2.25 * s, fill=LEFT + (140,)
    )
    return canvas.resize((size, int(size * 1.12)), Image.LANCZOS)


def rimmed(word, width):
    """The wordmark on an orange outline `width` pixels wide."""
    alpha = word.getchannel("A").point(lambda a: 255 if a > 0 else 0)
    grown = alpha.filter(ImageFilter.MaxFilter(width * 2 + 1))
    rim = Image.new("RGBA", word.size, RIM + (0,))
    rim.putalpha(grown)
    rim.alpha_composite(word)
    return rim


def lockup(word):
    height = word.height
    mark = block(int(height * 0.95))
    pad, gap = 24, int(height * 0.18)
    width = pad + mark.width + gap + word.width + pad
    canvas = Image.new("RGBA", (width, height + 2 * pad), (0, 0, 0, 0))
    canvas.alpha_composite(mark, (pad, pad + (height - mark.height) // 2))
    canvas.alpha_composite(word, (pad + mark.width + gap, pad))
    return canvas


def main():
    word = Image.open(ASSETS / "wordmark.png").convert("RGBA")
    rim = 6
    padded = Image.new("RGBA", (word.width + 2 * rim, word.height + 2 * rim), (0, 0, 0, 0))
    padded.alpha_composite(word, (rim, rim))
    lockup(padded).save(ASSETS / "logo-light.png", optimize=True)
    lockup(rimmed(padded, rim)).save(ASSETS / "logo-dark.png", optimize=True)
    print("wrote logo-light.png and logo-dark.png")


if __name__ == "__main__":
    main()
