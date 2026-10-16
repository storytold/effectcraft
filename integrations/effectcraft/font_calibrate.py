"""Score isolated AE/native text masks and record a proposed render-font relink.

Requires Pillow. Inputs should be
same-size transparent text PNGs; opaque white text on dark/color backgrounds also
works. This never edits a project or starts After Effects.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

from PIL import Image, ImageChops


def mask(path: Path, crop: tuple[int, int, int, int] | None = None) -> Image.Image:
    with Image.open(path) as source:
        image = source.crop(crop).convert("RGBA") if crop else source.convert("RGBA")
    alpha = image.getchannel("A")
    if alpha.getextrema() == (255, 255):
        red, green, blue, _ = image.split()
        alpha = ImageChops.darker(red, ImageChops.darker(green, blue))
    return alpha.point(lambda value: 255 if value > 180 else 0)


def score(reference: Image.Image, candidate: Image.Image) -> tuple[float, int]:
    if reference.size != candidate.size:
        raise ValueError(f"mask sizes differ: {reference.size} vs {candidate.size}")
    overlap = ImageChops.darker(reference, candidate).histogram()[255]
    total = ImageChops.lighter(reference, candidate).histogram()[255]
    a, b = reference.getbbox(), candidate.getbbox()
    if not a or not b or not total:
        raise ValueError("both masks need visible text")
    return overlap / total, max(abs(x - y) for x, y in zip(a, b))


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ae", type=Path, required=True)
    parser.add_argument("--candidate", action="append", required=True, metavar="POSTSCRIPT=PNG")
    parser.add_argument("--requested", required=True, help="original PostScript font name")
    parser.add_argument("--version", default="")
    parser.add_argument("--script", required=True, help="Latin, Arabic, etc.")
    parser.add_argument("--style", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--crop", nargs=4, type=int, metavar=("X0", "Y0", "X1", "Y1"), help="isolate one text region")
    args = parser.parse_args()
    crop = tuple(args.crop) if args.crop else None
    reference = mask(args.ae, crop)
    results = []
    for item in args.candidate:
        if "=" not in item:
            parser.error(f"candidate must be POSTSCRIPT=PNG: {item}")
        name, filename = item.split("=", 1)
        path = Path(filename)
        overlap, bounds_error = score(reference, mask(path, crop))
        results.append({"postscript": name, "png": str(path), "sha256": digest(path), "iou": round(overlap, 4), "bbox_error": bounds_error})
    results.sort(key=lambda row: row["iou"], reverse=True)
    best = results[0]
    runner_up = results[1]["iou"] if len(results) > 1 else 0.0
    # ponytail: fixed conservative thresholds; calibrate from more verified pairs if needed.
    selected = best["postscript"] if best["iou"] >= 0.90 and best["bbox_error"] <= 2 and best["iou"] - runner_up >= 0.05 else None
    evidence = {
        "requested_ps": args.requested,
        "version": args.version,
        "script": args.script,
        "style": args.style,
        "crop": crop,
        "ae": {"png": str(args.ae), "sha256": digest(args.ae), "bbox": reference.getbbox()},
        "candidates": results,
        "selected_render_font": selected,
    }
    args.out.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({"selected_render_font": selected, "best_iou": best["iou"], "evidence": str(args.out)}))


def self_check() -> None:
    a = Image.new("L", (8, 8), 0)
    b = Image.new("L", (8, 8), 0)
    a.paste(255, (2, 2, 5, 5))
    b.paste(255, (3, 2, 6, 5))
    assert score(a, a) == (1.0, 0)
    assert score(a, b) == (0.5, 1)


if __name__ == "__main__":
    import sys

    self_check() if sys.argv[1:] == ["--self-check"] else main()
