#!/usr/bin/env python3
"""Re-render the delivery videos of an After Effects project with EffectCraft.

    python3 -I rerender.py CONFIG.json [--out DIR] [--swap OLD=NEW ...] [--only a,b] [--jobs N]
                           [--encoder effectcraft|ffmpeg] [--chunk N] [--frames A-B] [--no-encode]
                           [--reimport]

For every config entry: import the .aep (or take the configured .ecproj), relink footage, apply the
config's scripts (for example the glow overrides written by glow_fit.py) and the entry's own
project_scripts, optionally replace footage files, then write OUT/<entry>.mp4 over the reference
video's frame range.

  --encoder effectcraft  (default) EffectCraft renders and encodes the video itself, through its
                   render queue's H.264 output: the reference's frame rate and average bit rate, and
                   its stored size when the entry's `vf` is a plain `scale=W:H`. One render process
                   per entry, --jobs at a time. Any other `vf` needs --encoder ffmpeg.
  --encoder ffmpeg the comp is rendered to PNG frames in chunks of --chunk frames, --jobs render
                   processes at a time, and the frames are encoded by the ffmpeg found on PATH with
                   the reference's geometry, pixel aspect, x264 settings (see parity_common.enc_opts)
                   and, if the reference has one, a silent audio track of the same shape. Use it to
                   reproduce a reference file's exact x264 settings. ffmpeg is an external tool you
                   run on your own machine: EffectCraft never links, bundles or ships it.
  --swap OLD=NEW   replace a footage file: OLD is matched against the end of the footage path (it must
                   match exactly one item, e.g. "images/photo-12.jpg"), NEW is the new file. Every layer
                   using that footage follows (the scripting form of File > Replace Footage).
                   Repeatable. Changes to the swaps produce a new project; frames are cached per project.
  --frames A-B     render only comp frames A..B to PNG (smoke test, no encode)
  --no-encode      render the PNG frames only (full_scores.py scores them as the plain render)

Re-runs resume: projects and PNG frames already rendered are kept. A lock on the work folder refuses a
second run on the same folder (two runs would wipe each other's chunk folders).
"""
import argparse
import concurrent.futures as cf
import json
import re
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import parity_common as pc  # noqa: E402


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


def lock(work):
    """Exclusive lock on WORK/.lock for the life of the process (POSIX; best effort elsewhere)."""
    f = open(work / ".lock", "w")
    try:
        import fcntl
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except ImportError:
        pass
    except OSError:
        pc.die(f"another rerender.py is running on {work}")
    return f


def swap_script(cfg, swaps):
    if not swaps:
        return []
    pairs = []
    for s in swaps:
        if "=" not in s:
            pc.die(f"--swap wants OLD=NEW, got {s}")
        old, new = s.split("=", 1)
        new = Path(new).resolve()
        if not new.exists():
            pc.die(f"--swap: {new} does not exist")
        pairs.append({"old": old, "new": str(new)})
    js = cfg["work_dir"] / f"_swap-{pc._hash([], json.dumps(pairs))}.jsx"
    return [pc.jsx_with_data("swap_footage.jsx", {"swaps": pairs}, js)]


def plan(cfg, cli, args):
    """Per entry: project, frames folder, comp frame range, reference info."""
    extra = swap_script(cfg, args.swap)
    plans = []
    for e in pc.entries(cfg, args.only):
        info = pc.probe(e["reference"])
        off = int(e["offset"])
        if off > 0:
            pc.die(f"{e['name']}: offset {off} > 0 means the video starts before the comp; nothing to render there")
        project = pc.entry_project(cfg, cli, e, extra=extra)
        a, b = -off, info["frames"] - 1 - off
        if args.frames:
            lo, hi = (int(x) for x in args.frames.split("-"))
            a, b = max(a, lo), min(b, hi)
        frames_dir = cfg["work_dir"] / "frames" / f"{pc.slug(e['comp'])}-{project.stem}"
        plans.append(dict(e=e, info=info, off=off, project=project, a=a, b=b, frames=frames_dir))
    return plans


def render_all(cli, plans, args):
    tasks = {}
    for p in plans:
        p["frames"].mkdir(parents=True, exist_ok=True)
        for a in range(p["a"], p["b"] + 1, args.chunk):
            b = min(p["b"], a + args.chunk - 1)
            # entries sharing a comp and project share the chunk
            tasks[(str(p["project"]), p["e"]["comp"], a, b)] = (p["info"]["fps"], p["frames"])
    log(f"render: {len(tasks)} chunks, {args.jobs} at a time")
    with cf.ThreadPoolExecutor(args.jobs) as ex:
        futs = {ex.submit(pc.render_range, cli, proj, comp, a, b, fps, d): (comp, a, b)
                for (proj, comp, a, b), (fps, d) in tasks.items()}
        for f in cf.as_completed(futs):
            comp, a, b = futs[f]
            if f.result():
                log(f"rendered {comp} {a}-{b}")


def encode_ffmpeg(p, out):
    """Encode the PNG frames with the user's own ffmpeg, replaying the reference's x264 settings."""
    info, e = p["info"], p["e"]
    n = p["b"] - p["a"] + 1
    missing = [f for f in range(p["a"], p["b"] + 1) if not (p["frames"] / f"{f:05d}.png").exists()]
    if missing:
        pc.die(f"{e['name']}: {len(missing)} frames missing in {p['frames']}")
    opts, desc = pc.enc_opts(e["reference"], info)
    ain, aout = pc.audio_opts(info)
    chain = ",".join(x for x in (e["vf"], pc.ENC, "format=yuv420p", f"setsar={info['sar'].replace(':', '/')}") if x)
    dest = out / f"{e['name']}.mp4"
    log(f"encode {dest.name}: {n} frames, {desc}")
    pc.run(["ffmpeg", "-v", "error", "-y", "-framerate", str(info["rate"]), "-start_number", str(p["a"]),
            "-i", p["frames"] / "%05d.png", *ain, "-frames:v", str(n), "-vf", chain, *opts, *aout,
            "-movflags", "+faststart", dest])
    return dest


def native_resize(e):
    """The render queue's resize for an entry's `vf`: None, or (width, height) for a plain scale=W:H."""
    vf = (e["vf"] or "").strip()
    if not vf:
        return None
    m = re.fullmatch(r"scale=(?:w=)?(\d+):(?:h=)?(\d+)", vf)
    if not m:
        pc.die(f"{e['name']}: vf {vf!r} is an ffmpeg filter EffectCraft's exporter cannot apply; "
               "use --encoder ffmpeg, or a plain scale=W:H")
    return int(m.group(1)), int(m.group(2))


def render_native(cli, p, out):
    """Render the entry's frame range straight to H.264 with EffectCraft's own exporter."""
    info, e = p["info"], p["e"]
    n = p["b"] - p["a"] + 1
    dest = out / f"{e['name']}.mp4"
    fps = info["fps"]
    # As in parity_common.render_range: the queue starts at ceil(start * fps), so ask a quarter frame
    # early, and stop a quarter frame after the last frame's start.
    item = {"comp": e["comp"], "output": str(dest), "format": "h264", "frameRate": fps,
            "start": round((p["a"] - 0.25) / fps, 9), "end": round((p["b"] + 0.25) / fps, 9)}
    if info["bit_rate"]:
        item["bitrate"] = max(50, info["bit_rate"] // 1000)
    size = native_resize(e)
    if size:
        item["resize"] = {"width": size[0], "height": size[1], "lockAspect": False, "quality": "high"}
    log(f"render {dest.name}: {n} frames, EffectCraft H.264 at {item.get('bitrate', 'default')} kb/s")
    dest.unlink(missing_ok=True)
    pc.run([cli, "run", "file.open", json.dumps({"path": str(p["project"])}), "renderQueue.add", json.dumps(item),
            "renderQueue.render", json.dumps({"wait": True})])
    if not dest.exists():
        raise pc.ParityError(f"{e['name']}: the render queue wrote no {dest}")
    got = pc.probe(dest)["frames"]
    if got != n:
        log(f"warning: {dest.name} has {got} frames, the reference range has {n}")
    if info["sar"] != "1:1":
        log(f"warning: {dest.name}: the reference's pixel aspect {info['sar']} is not reproduced (--encoder ffmpeg does)")
    return dest


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("--cli")
    ap.add_argument("--out", type=Path, help="folder of the encoded videos (default WORK/out)")
    ap.add_argument("--swap", action="append", default=[], metavar="OLD=NEW")
    ap.add_argument("--only", default="", help="comma-separated entry names")
    ap.add_argument("--jobs", type=int, default=2, help="parallel render processes")
    ap.add_argument("--encoder", choices=("effectcraft", "ffmpeg"), default="effectcraft",
                    help="effectcraft: EffectCraft's own H.264 export (default); ffmpeg: PNG frames encoded "
                         "by your own ffmpeg with the reference's x264 settings")
    ap.add_argument("--chunk", type=int, default=120, help="frames per render process (PNG frames)")
    ap.add_argument("--frames", default="", metavar="A-B", help="render only comp frames A..B to PNG (no encode)")
    ap.add_argument("--no-encode", action="store_true", help="render the PNG frames only")
    ap.add_argument("--reimport", action="store_true")
    args = ap.parse_args()
    pngs = args.encoder == "ffmpeg" or args.frames or args.no_encode
    pc.need_tools(*(("ffmpeg", "ffprobe") if args.encoder == "ffmpeg" else ("ffprobe",)))
    cfg = pc.load_config(args.config)
    cli = pc.find_cli(cfg, args.cli)
    cfg["work_dir"].mkdir(parents=True, exist_ok=True)
    held = lock(cfg["work_dir"])
    out = (args.out or cfg["work_dir"] / "out").resolve()
    out.mkdir(parents=True, exist_ok=True)
    try:
        pc.base_project(cfg, cli, args.reimport)
        plans = plan(cfg, cli, args)
        if pngs:
            render_all(cli, plans, args)
            if args.frames or args.no_encode:
                return
            for p in plans:
                encode_ffmpeg(p, out)
        else:
            for p in plans:
                native_resize(p["e"])
            with cf.ThreadPoolExecutor(args.jobs) as ex:
                for f in cf.as_completed([ex.submit(render_native, cli, p, out) for p in plans]):
                    f.result()
        log(f"done: {out}")
    except pc.ParityError as e:
        pc.die(str(e))
    finally:
        held.close()


if __name__ == "__main__":
    main()
