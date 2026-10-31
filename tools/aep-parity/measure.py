#!/usr/bin/env python3
"""Parity harness: EffectCraft renders of an imported After Effects project vs After Effects' renders.

    python3 -I measure.py CONFIG.json --run NAME [--frames N] [--only a,b] [--jobs N] [--reimport]
                          [--frame-list ENTRY=a,b,c ...] [--suffix S] [--no-encoded] [--reuse] [--notes TEXT]
    python3 -I measure.py CONFIG.json --probe-offset ENTRY [--window W]

For each config entry it picks N evenly spaced frames of the reference video (or the frames listed
with --frame-list), renders the comp at the matching comp frames (video frame = comp frame + offset),
maps the render to the video's stored geometry with the entry's `vf` (e.g. scale=1280:720 for a
720p encode), decodes the same frames from the reference and writes, under WORK/measure/NAME/:

    ENTRY/ref-K.png  ren-K.png  raw-K.png      reference, mapped render, render at comp size
    ENTRY/enc-K.png  ceil-K.png               encoded render and re-encoded reference (decoded)
    ENTRY-sheet.png                           contact sheet, one row per frame:
                                              reference | render | encoded render | heat map
    WORK/scores.jsonl                         one line per run and entry

Three numbers per frame (SSIM All on yuv444p, with PSNR alongside):
  plain    the clean render vs the reference. A delivered video carries codec loss, so even a
           perfect render does not reach 1.
  encoded  the render encoded like the reference (x264 options replayed from its SEI, or the measured
           GOP shape and bit rate when there is none) over a contiguous window of --enc-window frames
           around the sample, so GOP, B-frames and rate control behave as in the delivered file.
  ceiling  the reference re-encoded with the same settings vs itself, on the same frames: roughly what
           a perfect render scores. `gap = ceiling - encoded` is the number to drive towards 0.
  self     SSIM(encoded render, render): a first-generation estimate of the codec loss (the ceiling
           re-encodes already-encoded content and runs a little high).

--probe-offset ENTRY reports, for a few frames, the video frame offset whose SSIM against the render is
highest: run it first to check the comp-to-video time mapping (`offset` in the entry).
"""
import argparse
import concurrent.futures as cf
import json
import shutil
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import parity_common as pc  # noqa: E402


def frame_indices(lo, hi, n):
    """n evenly spaced frames in [lo, hi)."""
    span = hi - lo
    return sorted({lo + min(span - 1, int(round((i + 0.5) * span / n))) for i in range(n)})


def window(k, lo, hi, n):
    """A contiguous window of n video frames around k, inside [lo, hi)."""
    a = max(lo, k - n // 2)
    b = min(hi - 1, a + n - 1)
    return max(lo, b - n + 1), b


def encode_window(rdir, a, b, k, off, vf, fps, opts, out_png):
    """Encode render frames (comp a-off..b-off, files named by comp frame) and decode video frame k."""
    mp4 = out_png.with_suffix(".mp4")
    chain = ",".join(x for x in (vf, pc.ENC, "format=yuv420p") if x)
    pc.run(["ffmpeg", "-v", "error", "-y", "-framerate", str(fps), "-start_number", str(a - off), "-i", rdir / "%05d.png",
            "-frames:v", str(b - a + 1), "-vf", chain, *opts, "-an", mp4])
    pc.run(["ffmpeg", "-v", "error", "-y", "-i", mp4, "-vf", f"select='eq(n\\,{k - a})',{pc.DEC}",
            "-fps_mode", "passthrough", "-frames:v", "1", out_png])
    mp4.unlink()


def ceiling_window(video, a, b, k, opts, out_png):
    mp4 = out_png.with_suffix(".mp4")
    pc.run(["ffmpeg", "-v", "error", "-y", "-i", video, "-vf", f"select='between(n\\,{a}\\,{b})'",
            "-fps_mode", "passthrough", *opts, "-an", mp4])
    pc.run(["ffmpeg", "-v", "error", "-y", "-i", mp4, "-vf", f"select='eq(n\\,{k - a})',{pc.DEC}",
            "-fps_mode", "passthrough", "-frames:v", "1", out_png])
    mp4.unlink()


def sheet(rows, out, tile_h=360, gain=8):
    """Contact sheet with ffmpeg: per row reference | render | encoded render | heat(|enc - ref| luma x gain)."""
    inputs, parts, labels = [], [], []
    for i, (k, ref, ren, enc) in enumerate(rows):
        base = len(inputs)
        cols = [ref, ren] + ([enc] if enc else [])
        inputs += cols
        names = []
        for j in range(len(cols)):
            parts.append(f"[{base + j}:v]scale=-2:{tile_h}:flags=area,format=rgb24,split=2[t{i}_{j}][u{i}_{j}]")
            names.append(f"[t{i}_{j}]")
        cmp = len(cols) - 1
        parts.append(f"[u{i}_0]format=gray[g{i}a];[u{i}_{cmp}]format=gray[g{i}b];"
                     f"[g{i}b][g{i}a]blend=all_mode=difference,lut=y='min(255,val*{gain})',format=yuv444p,"
                     f"pseudocolor=p=turbo,format=rgb24[h{i}]")
        for j in range(1, len(cols)):
            if j != cmp:
                parts.append(f"[u{i}_{j}]nullsink")
        parts.append("".join(names) + f"[h{i}]hstack=inputs={len(cols) + 1}[r{i}]")
        labels.append(f"[r{i}]")
    graph = ";".join(parts) + ";" + "".join(labels) + (f"vstack=inputs={len(rows)}[o]" if len(rows) > 1 else "null[o]")
    args = []
    for p in inputs:
        args += ["-i", str(p)]
    pc.run(["ffmpeg", "-v", "error", "-y", *args, "-filter_complex", graph, "-map", "[o]", "-frames:v", "1", out])


def plan_entry(e, args, fl):
    info = pc.probe(e["reference"])
    off = int(e["offset"])
    lo, hi = max(0, off), info["frames"]      # comp frame = video frame - offset must be >= 0
    if e["name"] in fl:
        frames = [k for k in fl[e["name"]] if lo <= k < hi]
    elif isinstance(e["frames"], list):
        frames = [k for k in e["frames"] if lo <= k < hi]
    else:
        frames = frame_indices(lo, hi, args.frames or int(e["frames"]))
    if not frames:
        pc.die(f"{e['name']}: no frame to score")
    return info, off, lo, hi, frames


def measure(args, cfg, cli):
    rnd = cfg["work_dir"] / "measure" / args.run
    rnd.mkdir(parents=True, exist_ok=True)
    fl = {}
    for spec in args.frame_list:
        k, v = spec.split("=", 1)
        fl[k] = sorted({int(x) for x in v.split(",") if x})
    es = pc.entries(cfg, args.only or ",".join(fl))
    plans = {}
    for e in es:
        info, off, lo, hi, frames = plan_entry(e, args, fl)
        d = rnd / (e["name"] + args.suffix)
        d.mkdir(exist_ok=True)
        pc.extract_frames(e["reference"], frames, d)
        project = pc.entry_project(cfg, cli, e)
        plans[e["name"]] = dict(e=e, info=info, off=off, lo=lo, hi=hi, frames=frames, d=d, project=project)
    n = 1 if args.no_encoded else args.enc_window
    # One render per (project, comp, window): entries sharing a comp and project reuse it.
    wins = {}
    for p in plans.values():
        p["wins"] = {}
        for k in p["frames"]:
            a, b = window(k, p["lo"], p["hi"], n)
            ca, cb = a - p["off"], b - p["off"]
            rdir = rnd / "_raw" / f"{pc.slug(p['e']['comp'])}-{p['project'].stem}" / f"w-{ca:05d}-{cb:05d}"
            wins[(str(p["project"]), p["e"]["comp"], ca, cb)] = (p["info"]["fps"], rdir)
            p["wins"][k] = (a, b, rdir)
    wtime = {}
    with cf.ThreadPoolExecutor(args.jobs) as ex:
        futs = {}
        for (proj, comp, ca, cb), (fps, rdir) in wins.items():
            rdir.mkdir(parents=True, exist_ok=True)
            futs[ex.submit(timed_render, cli, proj, comp, ca, cb, fps, rdir)] = (comp, ca, cb, rdir)
        for f in cf.as_completed(futs):
            comp, ca, cb, rdir = futs[f]
            wtime[rdir] = f.result() / (cb - ca + 1)
            print(f"rendered {comp} {ca}-{cb}", flush=True)
    for p in plans.values():
        for k in p["frames"]:
            a, b, rdir = p["wins"][k]
            raw, ren = p["d"] / f"raw-{k:05d}.png", p["d"] / f"ren-{k:05d}.png"
            shutil.copyfile(rdir / f"{k - p['off']:05d}.png", raw)
            if p["e"]["vf"]:
                pc.run(["ffmpeg", "-v", "error", "-y", "-i", raw, "-vf", p["e"]["vf"], ren])
            else:
                shutil.copyfile(raw, ren)
    enc = not args.no_encoded
    encinfo = {}
    if enc:
        with cf.ThreadPoolExecutor(max(2, args.jobs)) as ex:
            futs = []
            for name, p in plans.items():
                opts, desc = pc.enc_opts(p["e"]["reference"], p["info"])
                encinfo[name] = desc
                for k in p["frames"]:
                    a, b, rdir = p["wins"][k]
                    e_png, c_png = p["d"] / f"enc-{k:05d}.png", p["d"] / f"ceil-{k:05d}.png"
                    if not (args.reuse and e_png.exists()):
                        futs.append(ex.submit(encode_window, rdir, a, b, k, p["off"], p["e"]["vf"], p["info"]["rate"], opts, e_png))
                    if not (args.reuse and c_png.exists()):
                        futs.append(ex.submit(ceiling_window, p["e"]["reference"], a, b, k, opts, c_png))
            for f in cf.as_completed(futs):
                f.result()
    results = {}
    for name, p in plans.items():
        d, rows, scores = p["d"], [], []
        for k in p["frames"]:
            ref, ren = d / f"ref-{k:05d}.png", d / f"ren-{k:05d}.png"
            s, y, ps = pc.ssim(ref, ren, psnr=True)
            a, b, rdir = p["wins"][k]
            rec = {"frame": k, "comp_frame": k - p["off"], "ssim": round(s, 4), "ssim_y": round(y, 4), "psnr": ps,
                   "render_s": round(wtime.get(rdir, 0.0), 2)}
            if enc:
                es_, ey, ep = pc.ssim(ref, d / f"enc-{k:05d}.png", psnr=True)
                cs, cy, cp = pc.ssim(ref, d / f"ceil-{k:05d}.png", psnr=True)
                fs = pc.ssim(ren, d / f"enc-{k:05d}.png")[0]
                rec.update({"enc": round(es_, 4), "enc_psnr": ep, "ceil": round(cs, 4), "ceil_psnr": cp,
                            "gap": round(cs - es_, 4), "self": round(fs, 4), "gap_self": round(fs - es_, 4)})
            scores.append(rec)
            rows.append((k, ref, ren, d / f"enc-{k:05d}.png" if enc else None))
        try:
            sheet(rows, rnd / f"{name}{args.suffix}-sheet.png")
        except pc.ParityError as err:
            print(f"warning: contact sheet of {name} failed: {err}", file=sys.stderr)
        m = len(scores)
        rec = {"run": args.run, "entry": name + args.suffix, "comp": p["e"]["comp"], "video": p["e"]["reference"].name,
               "mean_ssim": round(sum(x["ssim"] for x in scores) / m, 4), "min_ssim": min(x["ssim"] for x in scores),
               "mean_psnr": round(sum(x["psnr"] or 0 for x in scores) / m, 2)}
        if enc:
            me = sum(x["enc"] for x in scores) / m
            mc = sum(x["ceil"] for x in scores) / m
            rec.update({"mean_enc": round(me, 4), "mean_ceil": round(mc, 4), "gap": round(mc - me, 4),
                        "max_frame_gap": max(x["gap"] for x in scores),
                        "mean_self": round(sum(x["self"] for x in scores) / m, 4),
                        "gap_self": round(sum(x["gap_self"] for x in scores) / m, 4),
                        "encoder": encinfo[name], "enc_window": args.enc_window})
        rec.update({"frames": scores, "project": p["project"].name, "notes": args.notes,
                    "at": time.strftime("%Y-%m-%dT%H:%M:%S")})
        results[name] = rec
        with open(cfg["work_dir"] / "scores.jsonl", "a") as f:
            f.write(json.dumps(rec) + "\n")
    print(f"{'entry':28} {'plain':>7} {'min':>7} {'psnr':>6}" + ("" if not enc else f" {'enc':>7} {'ceil':>7} {'gap':>7} {'maxgap':>7} {'self':>7}"))
    for r in results.values():
        line = f"{r['entry']:28} {r['mean_ssim']:7.4f} {r['min_ssim']:7.4f} {r['mean_psnr']:6.2f}"
        if enc:
            line += f" {r['mean_enc']:7.4f} {r['mean_ceil']:7.4f} {r['gap']:7.4f} {r['max_frame_gap']:7.4f} {r['mean_self']:7.4f}"
        print(line)


def timed_render(cli, project, comp, a, b, fps, rdir):
    t0 = time.time()
    pc.render_range(cli, project, comp, a, b, fps, rdir)
    return time.time() - t0


def probe_offset(args, cfg, cli):
    e = pc.entries(cfg, args.probe_offset)[0]
    info = pc.probe(e["reference"])
    d = cfg["work_dir"] / "probe-offset" / e["name"]
    d.mkdir(parents=True, exist_ok=True)
    project = pc.entry_project(cfg, cli, e)
    off, win = int(e["offset"]), args.window
    for k in frame_indices(max(0, off), info["frames"], 4):
        ck = k - off
        pc.render_range(cli, project, e["comp"], ck, ck, info["fps"], d)
        ren = d / f"ren-{ck:05d}.png"
        if e["vf"]:
            pc.run(["ffmpeg", "-v", "error", "-y", "-i", d / f"{ck:05d}.png", "-vf", e["vf"], ren])
        else:
            shutil.copyfile(d / f"{ck:05d}.png", ren)
        cand = [j for j in range(k - win, k + win + 1) if 0 <= j < info["frames"]]
        pc.extract_frames(e["reference"], cand, d)
        best = sorted(((pc.ssim(d / f"ref-{j:05d}.png", ren)[0], j - ck) for j in cand), reverse=True)
        print(f"{e['name']} comp frame {ck}: best offsets " + ", ".join(f"{o:+d}:{s:.4f}" for s, o in best[:4]))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("--run", default="run-0", help="name of this measurement (folder under WORK/measure)")
    ap.add_argument("--cli", help="effectcraft-cli binary (default: $EFFECTCRAFT_CLI, the config's `cli`, this checkout's target/release)")
    ap.add_argument("--frames", type=int, default=0, help="evenly spaced frames per entry (default: the entry's `frames`)")
    ap.add_argument("--jobs", type=int, default=2, help="parallel render processes")
    ap.add_argument("--only", default="", help="comma-separated entry names")
    ap.add_argument("--reimport", action="store_true", help="re-import the .aep (or re-copy the .ecproj) first")
    ap.add_argument("--frame-list", action="append", default=[], help="ENTRY=a,b,c: score these video frames")
    ap.add_argument("--suffix", default="", help="suffix of the entry names in this run's results")
    ap.add_argument("--no-encoded", action="store_true", help="plain SSIM only")
    ap.add_argument("--enc-window", type=int, default=12, help="frames rendered and encoded around each sample")
    ap.add_argument("--reuse", action="store_true", help="keep existing encoded frames of this run")
    ap.add_argument("--notes", default="")
    ap.add_argument("--probe-offset", default="", metavar="ENTRY")
    ap.add_argument("--window", type=int, default=6, help="--probe-offset: offsets tried on each side")
    args = ap.parse_args()
    pc.need_tools("ffmpeg", "ffprobe")
    cfg = pc.load_config(args.config)
    cli = pc.find_cli(cfg, args.cli)
    try:
        pc.base_project(cfg, cli, args.reimport)
        if args.probe_offset:
            probe_offset(args, cfg, cli)
        else:
            measure(args, cfg, cli)
    except pc.ParityError as e:
        pc.die(str(e))


if __name__ == "__main__":
    main()
