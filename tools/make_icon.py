"""Draws Cronista's logo: a recording dot, half of it still recording, half of it already written.

The left half of the dot is the record button's red; the right half is lines of text, each cut by
the same circle, so the whole stays one round dot. Writes assets/logo.svg (the README, the video),
assets/logo-1024.png, assets/cronista-256.png and assets/cronista.ico, whose small sizes have
fewer, thicker lines so they still read as lines at 16 pixels.

    .venv\\Scripts\\python.exe tools\\make_icon.py
"""
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

ASSETS = Path(__file__).resolve().parents[1] / "assets"
BACKGROUND = ((26, 27, 33), (10, 11, 14))  # top, bottom
RED = ((255, 106, 85), (242, 59, 76))  # top, bottom
PAPER = (245, 245, 247)
RADIUS, GAP = 300, 10  # the dot, and half the gap between its two halves (on a 1024 canvas)


def lines(count: int) -> list[tuple[float, float]]:
    """Top and bottom of each line across the dot's height, with gaps of about half a line."""
    height = 2 * RADIUS / (count + (count - 1) * 0.55)
    gap = height * 0.55
    return [(512 - RADIUS + index * (height + gap), 512 - RADIUS + index * (height + gap) + height) for index in range(count)]


def svg(count: int = 4) -> str:
    bars = "\n    ".join(f'<rect x="{512 + GAP}" y="{top:.1f}" width="{RADIUS}" height="{bottom - top:.1f}" fill="#F5F5F7"/>'
                         for top, bottom in lines(count))
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <defs>
    <linearGradient id="background" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#1A1B21"/><stop offset="1" stop-color="#0A0B0E"/>
    </linearGradient>
    <linearGradient id="red" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#FF6A55"/><stop offset="1" stop-color="#F23B4C"/>
    </linearGradient>
    <clipPath id="dot"><circle cx="512" cy="512" r="{RADIUS}"/></clipPath>
  </defs>
  <rect x="32" y="32" width="960" height="960" rx="224" fill="url(#background)"/>
  <rect x="32.5" y="32.5" width="959" height="959" rx="223.5" fill="none" stroke="#FFFFFF" stroke-opacity="0.07"/>
  <g clip-path="url(#dot)">
    <rect x="{512 - RADIUS}" y="{512 - RADIUS}" width="{RADIUS - GAP}" height="{2 * RADIUS}" fill="url(#red)"/>
    {bars}
  </g>
</svg>
"""


def gradient(size: int, top: tuple, bottom: tuple) -> Image.Image:
    image = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        image.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))
    return image.resize((size, size))


def draw(size: int, count: int) -> Image.Image:
    """The icon at `size` pixels, drawn four times larger and reduced, so its edges are smooth."""
    big = size * 4
    k = big / 1024
    icon = Image.new("RGBA", (big, big), (0, 0, 0, 0))

    tile = Image.new("L", (big, big), 0)
    ImageDraw.Draw(tile).rounded_rectangle([32 * k, 32 * k, 992 * k, 992 * k], radius=224 * k, fill=255)
    icon.paste(gradient(big, *BACKGROUND), mask=tile)
    rim = ImageDraw.Draw(icon)
    rim.rounded_rectangle([32 * k, 32 * k, 992 * k, 992 * k], radius=224 * k, outline=(255, 255, 255, 18), width=max(1, round(2 * k)))

    dot = Image.new("L", (big, big), 0)
    ImageDraw.Draw(dot).ellipse([(512 - RADIUS) * k, (512 - RADIUS) * k, (512 + RADIUS) * k, (512 + RADIUS) * k], fill=255)
    left = Image.new("L", (big, big), 0)
    ImageDraw.Draw(left).rectangle([0, 0, (512 - GAP) * k, big], fill=255)
    icon.paste(gradient(big, *RED), mask=ImageChops.multiply(dot, left))

    bands = Image.new("L", (big, big), 0)
    paint = ImageDraw.Draw(bands)
    for top, bottom in lines(count):
        paint.rectangle([(512 + GAP) * k, top * k, big, bottom * k], fill=255)
    icon.paste(Image.new("RGB", (big, big), PAPER), mask=ImageChops.multiply(dot, bands))
    return icon.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    ASSETS.mkdir(exist_ok=True)
    (ASSETS / "logo.svg").write_text(svg(), encoding="utf-8")
    master = draw(1024, 4)
    master.save(ASSETS / "logo-1024.png")
    master.resize((256, 256), Image.LANCZOS).save(ASSETS / "cronista-256.png")
    sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [draw(size, 3 if size <= 24 else 4) for size in sizes]
    frames[-1].save(ASSETS / "cronista.ico", sizes=[(size, size) for size in sizes], append_images=frames[:-1])
    print("wrote", ", ".join(str(ASSETS / name) for name in ("logo.svg", "logo-1024.png", "cronista-256.png", "cronista.ico")))
