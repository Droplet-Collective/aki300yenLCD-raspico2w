"""Create a 400x96, uncompressed 24-bit BMP for the LCD sample."""

import argparse
import struct
from pathlib import Path

from PIL import Image, ImageOps


LCD_SIZE = (400, 96)


def convert(source_path: Path, output_path: Path) -> None:
    with Image.open(source_path) as source:
        image = ImageOps.exif_transpose(source).convert("RGB")
        # Fill the LCD without bars. Bias the vertical crop upward so the bear
        # and the avatar's face both stay inside the 1200x400 source viewport.
        frame = ImageOps.fit(
            image,
            LCD_SIZE,
            method=Image.Resampling.LANCZOS,
            centering=(0.5, 0.25),
        )
        frame.save(output_path, format="BMP")

    # The firmware accepts only uncompressed 24-bit BMP.
    with output_path.open("rb") as bmp:
        header = bmp.read(54)
    if (
        len(header) < 54
        or header[:2] != b"BM"
        or struct.unpack_from("<ii", header, 18) != LCD_SIZE
        or struct.unpack_from("<H", header, 28)[0] != 24
        or struct.unpack_from("<I", header, 30)[0] != 0
    ):
        raise ValueError("Output is not an uncompressed 400x96 24-bit BMP")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="Source PNG or other Pillow image")
    parser.add_argument(
        "output", type=Path, nargs="?", default=Path("IMAGE.BMP"), help="Output BMP path"
    )
    args = parser.parse_args()
    convert(args.source, args.output)
    print(f"Wrote {args.output} ({LCD_SIZE[0]}x{LCD_SIZE[1]}, 24-bit BMP)")


if __name__ == "__main__":
    main()
