#!/usr/bin/env python3
"""Full-length scores of the videos written by rerender.py, over every frame.

    python3 -I full_scores.py CONFIG.json [--out DIR] [--only a,b] [--swap OLD=NEW ...]

Per entry (pass the same --out / --swap as the rerender.py run, so the same frames are found):
  plain    SSIM / PSNR of the rendered PNG frames (mapped with the entry's `vf`) vs the reference;
           only when rerender.py wrote PNG frames (--encoder ffmpeg or --no-encode)
  encoded  SSIM / PSNR of OUT/<entry>.mp4 vs the reference
  ceiling  SSIM / PSNR of the reference re-encoded with the same x264 settings vs the reference
gap = ceiling - encoded; the worst frames are listed. Writes WORK/full-scores.json.

The ceiling is re-encoded with x264 through ffmpeg, so the gap compares like with like only for a
video written by `rerender.py --encoder ffmpeg`. A video from EffectCraft's own H.264 export also
carries the difference between the two encoders.
"""
import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import parity_common as pc  # noqa: E402
import rerender  # noqa: E402


def compare(a_in, a_vf, b_in, b_vf, stats, rate):
    g = (f"[0:v]settb=1/1000,setpts=N,{a_vf},format=yuv444p,split[a0][a1];"
         f"[1:v]settb=1/1000,setpts=N,{b_vf},format=yuv444p,split[b0][b1];"
         f"[a0][b0]ssim=stats_file={stats}.ssim;[a1][b1]psnr=stats_file={stats}.psnr")
    r = pc.run(["ffmpeg", "-hide_banner", "-nostats", *a_in, *b_in, "-lavfi", g, "-f", "null", "-"], check=False)
    s = re.search(r"SSIM Y:([0-9.]+).*All:([0-9.]+)", r.stderr)
    p = re.search(r"PSNR .*average:([0-9.inf]+)", r.stderr)
    if not s:
        raise pc.ParityError(r.stderr[-800:])
    per = [float(re.search(r"All:([0-9.]+)", line).group(1)) for line in open(f"{stats}.ssim")]
    return {"ssim": round(float(s.group(2)), 4), "psnr": round(float(p.group(1)), 2) if p else None,
            "min_ssim": round(min(per), 4), "worst_frames": sorted(range(len(per)), key=lambda i: per[i])[:5], "n": len(per)}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("--cli")
    ap.add_argument("--out", type=Path)
    ap.add_argument("--only", default="")
    ap.add_argument("--swap", action="append", default=[])
    args = ap.parse_args()
    pc.need_tools("ffmpeg", "ffprobe")
    cfg = pc.load_config(args.config)
    cli = pc.find_cli(cfg, args.cli)
    out = (args.out or cfg["work_dir"] / "out").resolve()
    stats = cfg["work_dir"] / "stats"
    stats.mkdir(parents=True, exist_ok=True)
    args.frames = ""
    res = {}
    try:
        for p in rerender.plan(cfg, cli, args):
            e, info = p["e"], p["info"]
            name, ref = e["name"], ["-i", str(e["reference"])]
            mp4 = out / f"{name}.mp4"
            if not mp4.exists():
                pc.die(f"{mp4} does not exist: run rerender.py first")
            ceil = stats / f"{name}-ceiling.mp4"
            if not ceil.exists():
                opts, _ = pc.enc_opts(e["reference"], info)
                pc.run(["ffmpeg", "-v", "error", "-y", "-i", e["reference"], *opts, "-an", ceil])
            frames = ["-framerate", str(info["rate"]), "-start_number", str(p["a"]), "-i", str(p["frames"] / "%05d.png")]
            have_pngs = all((p["frames"] / f"{f:05d}.png").exists() for f in range(p["a"], p["b"] + 1))
            rate = info["rate"]
            r = {"plain": compare(ref, pc.DEC, frames, e["vf"] or "null", stats / f"{name}-plain", rate) if have_pngs else None,
                 "encoded": compare(ref, pc.DEC, ["-i", str(mp4)], pc.DEC, stats / f"{name}-enc", rate),
                 "ceiling": compare(ref, pc.DEC, ["-i", str(ceil)], pc.DEC, stats / f"{name}-ceil", rate)}
            r["gap"] = round(r["ceiling"]["ssim"] - r["encoded"]["ssim"], 4)
            res[name] = r
            pl = r["plain"]
            plain = (f"plain {pl['ssim']:.4f} (min {pl['min_ssim']:.4f}, worst {pl['worst_frames']})" if pl
                     else "plain - (no PNG frames)")
            print(f"{name:28} {plain}  encoded {r['encoded']['ssim']:.4f}  ceiling {r['ceiling']['ssim']:.4f}"
                  f"  gap {r['gap']:.4f}  n={r['encoded']['n']}", flush=True)
    except pc.ParityError as err:
        pc.die(str(err))
    f = cfg["work_dir"] / "full-scores.json"
    old = json.loads(f.read_text()) if f.exists() else {}
    old.update(res)
    f.write_text(json.dumps(old, indent=1))


if __name__ == "__main__":
    main()
