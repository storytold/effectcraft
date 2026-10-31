"""Shared helpers of the After Effects -> EffectCraft parity tools.

Imported by measure.py, glow_fit.py, rerender.py and full_scores.py (each entry script puts its own
directory on sys.path first, so the tools also run under `python3 -I`).

These tools call the ffmpeg and ffprobe found on PATH, as separate programs on the user's machine:
ffprobe to read the reference videos, ffmpeg to decode them, to score (SSIM / PSNR), to encode the
render windows and reference copies that the encoded and ceiling scores compare, and, only with
`rerender.py --encoder ffmpeg`, to encode deliverables. EffectCraft never links, bundles or ships
them; its own exporter writes the deliverables by default.
"""
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from fractions import Fraction
from pathlib import Path

HERE = Path(__file__).resolve().parent
JSX = HERE / "jsx"
REPO = HERE.parent.parent

# Every YCbCr <-> RGB step uses BT.709 limited range with swscale's accurate rounding and full chroma
# interpolation. swscale's default fast path biases dark frames by about a level per channel, which is
# the same order as the differences being measured.
DEC = "scale=in_color_matrix=bt709:in_range=tv:flags=bicubic+accurate_rnd+full_chroma_int,format=rgb24"
ENC = "scale=out_color_matrix=bt709:out_range=tv:flags=bicubic+accurate_rnd+full_chroma_inp"
TAGS = ["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv"]
JSON_MARK = "@@JSON "


class ParityError(RuntimeError):
    pass


def die(msg):
    sys.exit(f"error: {msg}")


# ---------------------------------------------------------------- processes

def run(cmd, check=True, **kw):
    cmd = [str(c) for c in cmd]
    r = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", **kw)
    if check and r.returncode != 0:
        tail = (r.stderr or r.stdout).strip()[-1500:]
        raise ParityError(f"command failed ({r.returncode}): {' '.join(cmd[:6])} ...\n{tail}")
    return r


def need_tools(*names):
    for n in names:
        if shutil.which(n) is None:
            die(f"`{n}` is not on PATH")


# ---------------------------------------------------------------- config

def load_config(path):
    """Read the JSON config; relative paths in it are relative to the config file's folder."""
    path = Path(path).resolve()
    try:
        cfg = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        die(f"cannot read config {path}: {e}")
    base = path.parent

    def rel(p):
        return None if p in (None, "") else (base / p).resolve()

    cfg["_base"] = base
    cfg["aep"] = rel(cfg.get("aep"))
    cfg["project"] = rel(cfg.get("project"))
    if not cfg["aep"] and not cfg["project"]:
        die("the config needs `aep` (to import) or `project` (an existing .ecproj)")
    cfg["work_dir"] = rel(cfg.get("work_dir") or "parity-work")
    cfg["scripts"] = [rel(s) for s in cfg.get("scripts", [])]
    cfg["relink"] = cfg.get("relink", [])
    names = set()
    for e in cfg.get("entries", []):
        for k in ("name", "comp", "reference"):
            if not e.get(k):
                die(f"every entry needs `{k}`: {e}")
        if e["name"] in names:
            die(f"duplicate entry name {e['name']}")
        names.add(e["name"])
        e["reference"] = rel(e["reference"])
        e.setdefault("offset", 0)
        e.setdefault("frames", 8)
        e.setdefault("vf", None)
        e["project_scripts"] = [rel(s) for s in e.get("project_scripts", [])]
    return cfg


def entries(cfg, only=""):
    es = cfg.get("entries", [])
    if only:
        want = only.split(",")
        missing = [w for w in want if w not in {e["name"] for e in es}]
        if missing:
            die(f"unknown entries: {', '.join(missing)}")
        es = [e for e in es if e["name"] in want]
    return es


def find_cli(cfg=None, arg=None):
    """--cli, then $EFFECTCRAFT_CLI, then the config's `cli`, then this checkout's release build."""
    for c in (arg, os.environ.get("EFFECTCRAFT_CLI"), (cfg or {}).get("cli")):
        if c:
            p = Path(c)
            if not p.is_absolute() and cfg and (cfg["_base"] / p).exists():
                p = cfg["_base"] / p
            return p
    p = REPO / "target" / "release" / ("effectcraft-cli.exe" if os.name == "nt" else "effectcraft-cli")
    if not p.exists():
        die(f"no effectcraft-cli at {p}: build it with `cargo build --release -p effectcraft-cli` "
            "or pass --cli / set EFFECTCRAFT_CLI")
    return p


# ---------------------------------------------------------------- JSX helpers

def jsx_with_data(template, data, out):
    """Write `out`: `var CFG = <data>;`, the shared selection helper, then the template body."""
    body = (JSX / template).read_text(encoding="utf-8")
    head = (JSX / "_select.jsx").read_text(encoding="utf-8")
    Path(out).write_text(f"var CFG = {json.dumps(data, separators=(',', ':'))};\n" + head + "\n" + body, encoding="utf-8")
    return Path(out)


def run_script(cli, script, project, save=True):
    cmd = [cli, "script", script, project] + (["--save"] if save else [])
    return run(cmd).stdout


def script_json(cli, script, project):
    """Run a read-only JSX that prints one `@@JSON {...}` line; return the parsed object."""
    out = run_script(cli, script, project, save=False)
    for line in out.splitlines():
        if line.startswith(JSON_MARK):
            return json.loads(line[len(JSON_MARK):])
    raise ParityError(f"{script} printed no {JSON_MARK.strip()} line:\n{out[-800:]}")


def comp_info(cli, project, comps, work):
    js = jsx_with_data("comp_info.jsx", {"comps": sorted(set(comps))}, Path(work) / "_comp_info.jsx")
    info = script_json(cli, js, project)
    missing = [c for c in comps if c not in info]
    if missing:
        die(f"comps not found in {project}: {missing}")
    return info


# ---------------------------------------------------------------- projects

def _hash(paths, extra=""):
    h = hashlib.sha256(extra.encode())
    for p in paths:
        h.update(str(p).encode())
        h.update(Path(p).read_bytes())
    return h.hexdigest()[:12]


def base_project(cfg, cli, reimport=False):
    """work/base.ecproj: the .aep imported (or the configured .ecproj copied) and relinked."""
    work = cfg["work_dir"]
    work.mkdir(parents=True, exist_ok=True)
    dest = work / "base.ecproj"
    if dest.exists() and not reimport:
        return dest
    tmp = work / "_base.ecproj"
    tmp.unlink(missing_ok=True)
    if cfg["project"]:
        shutil.copyfile(cfg["project"], tmp)
    else:
        print(f"importing {cfg['aep'].name} ...", flush=True)
        run([cli, "run", "file.importAep", json.dumps({"path": str(cfg["aep"]), "allowLosses": True}),
             "file.saveAs", json.dumps({"path": str(tmp)}), "--empty"])
    if cfg["relink"]:
        js = jsx_with_data("relink.jsx", {"pairs": cfg["relink"]}, work / "_relink.jsx")
        print((run_script(cli, js, tmp).strip().splitlines() or [""])[-1], flush=True)
    os.replace(tmp, dest)
    # Every derived project and every frame rendered from one depends on the base: drop them.
    for p in work.glob("project-*.ecproj"):
        p.unlink()
    shutil.rmtree(work / "frames", ignore_errors=True)
    return dest


def derived_project(cfg, cli, scripts, tag="project", reimport=False):
    """base + the listed JSX scripts, applied in order, cached by the scripts' content."""
    base = base_project(cfg, cli, reimport)
    scripts = [Path(s) for s in scripts]
    for s in scripts:
        if not s.exists():
            die(f"script not found: {s}")
    if not scripts:
        return base
    dest = cfg["work_dir"] / f"project-{tag}-{_hash(scripts, base.name)}.ecproj"
    if dest.exists():
        return dest
    tmp = dest.with_name("_" + dest.name)
    shutil.copyfile(base, tmp)
    for s in scripts:
        out = run_script(cli, s, tmp).strip().splitlines()
        print(f"  {s.name}: {out[-2] if len(out) > 1 else (out[0] if out else '')}", flush=True)
    os.replace(tmp, dest)
    return dest


def entry_project(cfg, cli, entry, exclude=(), extra=(), tag="project"):
    """The project an entry renders from: base + config scripts (minus `exclude`) + the entry's own
    project_scripts + `extra`."""
    ex = {Path(x).resolve() for x in exclude}
    scripts = [s for s in cfg["scripts"] if Path(s).resolve() not in ex] + entry["project_scripts"] + list(extra)
    return derived_project(cfg, cli, scripts, tag)


# ---------------------------------------------------------------- video

def probe(video):
    """Geometry, timing, encoder hints and audio of a reference video."""
    out = run(["ffprobe", "-v", "error", "-count_packets", "-show_entries",
               "stream=codec_type,codec_name,width,height,r_frame_rate,nb_read_packets,sample_aspect_ratio,"
               "profile,level,bit_rate,sample_rate,channels,channel_layout:format=bit_rate",
               "-of", "json", str(video)]).stdout
    js = json.loads(out)
    v = next((s for s in js.get("streams", []) if s.get("codec_type") == "video"), None)
    if v is None:
        raise ParityError(f"{video}: no video stream")
    a = next((s for s in js.get("streams", []) if s.get("codec_type") == "audio"), None)
    rate = Fraction(v["r_frame_rate"])
    sar = v.get("sample_aspect_ratio") or "1:1"
    return {"width": int(v["width"]), "height": int(v["height"]), "fps": float(rate), "rate": rate,
            "frames": int(v["nb_read_packets"]), "sar": "1:1" if sar in ("0:1", "N/A") else sar,
            "profile": v.get("profile"), "level": v.get("level"), "codec": v.get("codec_name"),
            "bit_rate": int(v.get("bit_rate") or js.get("format", {}).get("bit_rate") or 0),
            "audio": None if a is None else {"rate": int(a.get("sample_rate") or 48000),
                                             "layout": a.get("channel_layout") or "stereo",
                                             "bit_rate": int(a.get("bit_rate") or 192000)}}


def gop_shape(video, n=400):
    """(keyint, consecutive B-frames) from the first `n` frames' picture types."""
    out = run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-read_intervals", f"%+#{n}",
               "-show_entries", "frame=pict_type", "-of", "csv=p=0", str(video)]).stdout
    types = [t.strip() for t in out.split() if t.strip()]
    ki = [i for i, t in enumerate(types) if t == "I"]
    keyint = min((b - a for a, b in zip(ki, ki[1:])), default=250)
    runs = [len(m) for m in re.findall(r"B+", "".join(types))]
    return keyint, max(runs, default=0)


_PROFILES = {"Main": "main", "High": "high", "Baseline": "baseline", "Constrained Baseline": "baseline"}
_B_PYRAMID = {"0": "none", "1": "strict", "2": "normal"}
_DIRECT = {"0": "none", "1": "spatial", "2": "temporal", "3": "auto"}


def enc_opts(video, info=None):
    """ffmpeg output options that reproduce the reference's H.264 encode as closely as libx264 can.

    When the reference was written by x264, its options SEI is parsed and replayed option by option.
    Otherwise (another encoder) the GOP length, B-frame run, profile, level and average bit rate are
    measured with ffprobe and an ABR encode with that shape is used. Returns (options, description)."""
    info = info or probe(video)
    common = ["-c:v", "libx264", "-pix_fmt", "yuv420p", *TAGS]
    prof = _PROFILES.get(info.get("profile") or "", "high")
    lvl = info.get("level")
    level = ["-level:v", f"{lvl / 10:.1f}"] if isinstance(lvl, int) and lvl > 0 else []
    data = Path(video).read_bytes()
    i = data.find(b"x264 - core")
    if i >= 0:
        sei = data[i:data.find(b"\0", i)].decode("ascii", "replace")
        o = dict(kv.split("=", 1) for kv in sei.split("options:", 1)[-1].split() if "=" in kv)
        p = []

        def put(src, dst=None, conv=None):
            if src in o:
                p.append(f"{dst or src}={conv(o[src]) if conv else o[src]}")

        for k in ("ref", "bframes", "subme", "me", "weightp", "weightb", "trellis", "8x8dct", "cabac",
                  "keyint", "scenecut", "qcomp", "vbv_maxrate", "vbv_bufsize"):
            put(k, k.replace("_", "-"))
        put("b_adapt", "b-adapt")
        put("b_pyramid", "b-pyramid", lambda v: _B_PYRAMID.get(v, v))
        put("direct", "direct", lambda v: _DIRECT.get(v, v))
        put("me_range", "merange")
        put("rc_lookahead", "rc-lookahead")
        put("mixed_ref", "mixed-refs")
        put("keyint_min", "min-keyint")
        put("ip_ratio", "ipratio")
        if "deblock" in o:
            d = o["deblock"].split(":")
            p.append("no-deblock=1" if d[0] == "0" else f"deblock={d[1]},{d[2]}")
        if "aq" in o:
            aq = o["aq"].split(":")
            p.append(f"aq-mode={aq[0]}")
            if len(aq) > 1:
                p.append(f"aq-strength={aq[1]}")
        if "psy_rd" in o and o.get("psy", "1") == "1":
            p.append("psy-rd=" + o["psy_rd"].replace(":", ","))
        # chroma_qp_offset is not replayed: x264 derives it from psy-rd itself.
        rc = o.get("rc", "crf")
        if rc == "crf":
            rate = ["-crf", o.get("crf", "23")]
        else:
            rate = ["-b:v", f"{o.get('bitrate', info['bit_rate'] // 1000)}k"]
        return (common + ["-profile:v", prof, *level, "-preset", "medium", *rate, "-x264-params", ":".join(p)],
                f"x264 options from the reference's SEI ({rc})")
    keyint, bf = gop_shape(video)
    br = info["bit_rate"] or 8_000_000
    x = f"bframes={bf}:b-adapt=0:keyint={keyint}:min-keyint={keyint}:scenecut=0"
    return (common + ["-profile:v", prof, *level, "-preset", "medium", "-b:v", str(br), "-maxrate", str(int(br * 1.5)),
                      "-bufsize", str(br * 2), "-x264-params", x],
            f"x264 ABR {br // 1000} kb/s, keyint {keyint}, {bf} B (no SEI: measured GOP shape)")


def audio_opts(info):
    """A silent track shaped like the reference's audio, or no audio."""
    a = info.get("audio")
    if not a:
        return [], ["-an"]
    return (["-f", "lavfi", "-i", f"anullsrc=channel_layout={a['layout']}:sample_rate={a['rate']}"],
            ["-map", "0:v", "-map", "1:a", "-c:a", "aac", "-b:a", str(a["bit_rate"]), "-shortest"])


def extract_frames(video, frames, outdir, name="ref-{:05d}.png", vf=None, key=None):
    """Decode the listed frame indices (0-based, presentation order) to outdir/name.format(key(k))."""
    outdir = Path(outdir)
    outdir.mkdir(parents=True, exist_ok=True)
    frames = sorted(set(frames))
    if not frames:
        return
    tmp = outdir / "_x"
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir()
    # select with a long eq() chain is slow; between() on chunks of contiguous runs keeps it short.
    runs = []
    for k in frames:
        if runs and runs[-1][1] == k - 1:
            runs[-1][1] = k
        else:
            runs.append([k, k])
    sel = "+".join(f"between(n\\,{a}\\,{b})" for a, b in runs)
    chain = f"select='{sel}',{DEC}" + (f",{vf}" if vf else "")
    run(["ffmpeg", "-v", "error", "-y", "-i", video, "-vf", chain, "-fps_mode", "passthrough", tmp / "%06d.png"])
    got = sorted(tmp.glob("*.png"))
    if len(got) != len(frames):
        raise ParityError(f"{video}: decoded {len(got)} of {len(frames)} frames (past the end?)")
    for k, f in zip(frames, got):
        os.replace(f, outdir / name.format(key(k) if key else k))
    shutil.rmtree(tmp, ignore_errors=True)


def ssim(a, b, psnr=False, a_vf="null", b_vf="null"):
    """ffmpeg SSIM (All, Y) [and PSNR average] of two images or videos, compared on yuv444p."""
    g = (f"[0:v]{a_vf},format=yuv444p,split[a0][a1];[1:v]{b_vf},format=yuv444p,split[b0][b1];"
         "[a0][b0]ssim;[a1][b1]psnr")
    r = run(["ffmpeg", "-hide_banner", "-nostats", "-i", a, "-i", b, "-lavfi", g, "-f", "null", "-"], check=False)
    m = re.search(r"SSIM Y:([0-9.]+).*All:([0-9.]+)", r.stderr)
    if not m:
        raise ParityError(f"ssim failed: {r.stderr[-600:]}")
    q = re.search(r"PSNR .*average:([0-9.inf]+)", r.stderr)
    out = (float(m.group(2)), float(m.group(1)))
    return out + ((float(q.group(1)) if q else None),) if psnr else out


def slug(name):
    return re.sub(r"[^A-Za-z0-9]+", "_", name).strip("_") or "comp"


# ---------------------------------------------------------------- rendering

def render_range(cli, project, comp, a, b, fps, outdir, name="{:05d}.png"):
    """Render comp frames a..b (inclusive) as outdir/name.format(frame) with one CLI process.

    The render queue starts at ceil(start * fps), so the range is asked a quarter frame early and the
    files, which the CLI names after their comp frame, are mapped by name, not by order."""
    outdir = Path(outdir)
    want = {f: outdir / name.format(f) for f in range(a, b + 1)}
    if all(p.exists() and p.stat().st_size for p in want.values()):
        return False
    tmp = outdir / f"_tmp-{a}"
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir(parents=True)
    item = {"comp": comp, "output": str(tmp / "f_[#####].png"), "format": "png", "frameRate": fps,
            "start": round((a - 0.25) / fps, 9), "end": round((b + 0.75) / fps, 9)}
    r = run([cli, "run", "file.open", json.dumps({"path": str(project)}), "renderQueue.add", json.dumps(item),
             "renderQueue.render", json.dumps({"wait": True})], check=False)
    got = {}
    for x in tmp.glob("*.png"):
        m = re.search(r"(\d+)\.png$", x.name)
        if m:
            got[int(m.group(1))] = x
    if r.returncode != 0 or any(f not in got for f in want):
        raise ParityError(f"render {comp} {a}-{b} failed (got {len(got)} frames): {r.stderr.strip()[-600:]}")
    for f, dst in want.items():
        os.replace(got[f], dst)
    shutil.rmtree(tmp, ignore_errors=True)
    return True


def render_samples(cli, project, comp, frames, fps, step, scale, outdir, name="{:05d}.png"):
    """Render the comp frames listed (all multiples of `step`) at 1/scale resolution.

    One sequence render at fps/step when that is a whole number (the CLI names files by their index at
    that rate); otherwise one render-frame call per frame."""
    outdir = Path(outdir)
    outdir.mkdir(parents=True, exist_ok=True)
    frames = sorted(frames)
    rate = fps / step
    if abs(rate - round(rate)) < 1e-6 and all(k % step == 0 for k in frames):
        rate = int(round(rate))
        tmp = outdir / "_seq"
        shutil.rmtree(tmp, ignore_errors=True)
        tmp.mkdir()
        res = {1: "full", 2: "half", 3: "third", 4: "quarter"}.get(scale, str(scale))
        run([cli, "render", "--project", project, "--comp", comp, "--out", tmp / "f.png", "--format", "png",
             "--start", f"{frames[0] / fps - 0.25 / fps:.6f}", "--end", f"{frames[-1] / fps + 0.25 / fps:.6f}",
             "--fps", str(rate), "--resolution", res])
        for k in frames:
            src = tmp / f"f_{k // step:05d}.png"
            if not src.exists():
                raise ParityError(f"render {comp}: frame {k} missing ({src.name})")
            os.replace(src, outdir / name.format(k))
        shutil.rmtree(tmp, ignore_errors=True)
        return
    for k in frames:
        run([cli, "render-frame", "--project", project, "--comp", comp, "--frame", str(k),
             "--scale", f"{1 / scale:.6f}", "--out", outdir / name.format(k)])
