#!/usr/bin/env python3
"""Fit a replacement for a procedural glow layer whose pattern cannot be matched.

    python3 -I glow_fit.py CONFIG.json STEP [--only ENTRY,...] [options]

Some looks are driven by a procedural generator whose exact pattern is the host application's own
(typically a Fractal Noise solid, colourised, glowed and blurred into soft "pools" of light, in Add
mode). EffectCraft reproduces the effect chain, but not the other implementation's noise lattice, so
the pools land elsewhere and every frame differs. When the look must match a delivered video, this tool
replaces the content of those layers with a field that is fitted to the reference video:

  * The field lives in the selected layers' own layer space: a grid of radial Gradient Ramp "cones"
    (Add mode, radius R), each with an RGB amplitude keyed every `step` frames.
  * The layers keep their masks, opacity keys, blending mode, parenting and timing, so the field
    goes through the same chain as the noise did. Nothing else in the project changes.

Model. With F the field (0..1) and N the render with the selected layers disabled, a render is
N + U * F per pixel. The transfer U (masks x opacity x occlusion x later adjustments) is measured by
rendering a flat field of level G through the same chain: U = (unit - N) / (G * 255). Per sampled
frame, the cone amplitudes are a non-negative ridge least-squares fit of U * basis to (reference - N).
`--refine` renders the fitted project and fits again on the residual, which absorbs the chain's
non-linearities (clipping, exposure).

Precomps. A comp holding selected layers may be used as a precomp in the target comp, several times
(each instance at its own start time). It is fitted in its own time: a target frame belongs to an
instance when only that instance is live. Where two instances overlap (cross-fades), `xrender` +
`fit --xfade 1` fit the overlapping frames jointly: each live comp gets its own transfer (a project
where only that comp's selected layers carry the flat field), and the two colour sets are tied by a
ridge term to the keys interpolated from the single-instance fit, so what the two transfers cannot
tell apart stays continuous.

Steps (run in this order; see README.md):
  probe     inventory of the selected layers and their precomp instances -> WORK/glow_fit/probe.json
  render    render the sampled frames of the target comps (--tags nn,unit[,fit])
  ref       decode the reference frames at the same size
  xrender   render the cross-fade frames (--tags nn,unit1[,fit])
  xref      decode the reference frames of the cross-fades
  fit       fit -> params.json, then emit (--xfade 1 adds the joint cross-fade keys, --refine a 2nd pass)
  emit      write the overrides JSX from params.json
  check     weighted RMS residual (levels) per target: nn / predicted / fit render
  xcheck    the same per cross-fade window (--with-fit after `xrender --tags fit`)

Requires numpy and Pillow (`pip install numpy pillow`).
"""
import argparse
import json
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import parity_common as pc  # noqa: E402

try:
    import numpy as np
    from PIL import Image
except ImportError:
    sys.exit("glow_fit.py needs numpy and Pillow: pip install numpy pillow")

DEFAULTS = {
    "step": 10,             # fit one key every `step` target frames
    "downscale": 4,         # render the fit frames at 1/downscale resolution
    "fit_downscale": 2,     # further box downsampling of the fit grid
    "unit_level": 0.25,     # grey level of the transfer probe field
    "lambda": 50.0,         # ridge weight of the per-frame fit
    "smooth": 1,            # [1 2 1] temporal smoothing of the keys
    "grid": {"spacing": [160, 160], "radius": 320},
    "xfade": {"step": 2, "pad": 24, "lambda": 500.0},
    "prefix": "GLOWFIT ",
    "overrides": "glow_overrides.jsx",
}


# ---------------------------------------------------------------- setup

class Ctx:
    def __init__(self, args):
        pc.need_tools("ffmpeg", "ffprobe")
        self.cfg = pc.load_config(args.config)
        self.cli = pc.find_cli(self.cfg, args.cli)
        g = dict(DEFAULTS)
        g.update(self.cfg.get("glow_fit") or {})
        g["grid"] = {**DEFAULTS["grid"], **g.get("grid", {})}
        g["xfade"] = {**DEFAULTS["xfade"], **g.get("xfade", {})}
        sel = g.get("select") or {}
        if not (sel.get("layer_name") or sel.get("effect_match_name")):
            pc.die("glow_fit.select needs `layer_name` and/or `effect_match_name`")
        if not g.get("targets"):
            pc.die("glow_fit.targets lists the entries whose reference video drives the fit")
        self.g = g
        self.sel = sel
        self.work = self.cfg["work_dir"] / "glow_fit"
        self.work.mkdir(parents=True, exist_ok=True)
        self.cache = self.work / "cache"
        self.overrides = (self.cfg["_base"] / g["overrides"]).resolve()
        self.entries = {e["name"]: e for e in pc.entries(self.cfg, ",".join(g["targets"]))}
        self.only = args.only.split(",") if args.only else list(self.entries)
        self.Q = int(g["downscale"])
        self.FDS = int(g["fit_downscale"])
        self.STEP = int(g["step"])
        self.XSTEP = int(g["xfade"]["step"])
        self.XPAD = int(g["xfade"]["pad"])
        sp = g["grid"]["spacing"]
        self.SX, self.SY = (sp, sp) if isinstance(sp, (int, float)) else sp
        self.R = float(g["grid"]["radius"])
        self.G = float(g["unit_level"])
        self._targets = None

    # -- projects
    def tag_script(self, tag):
        js = self.work / f"_{tag}.jsx"
        if tag == "nn":
            return pc.jsx_with_data("glow_off.jsx", {"select": self.sel}, js)
        if tag == "unit":
            return pc.jsx_with_data("glow_unit.jsx", {"select": self.sel, "level": self.G, "only": ""}, js)
        if tag.startswith("unit1-"):
            g = self.probe()["slugs"][tag[len("unit1-"):]]
            return pc.jsx_with_data("glow_unit.jsx", {"select": self.sel, "level": self.G, "only": g}, js)
        if tag == "fit":
            if not self.overrides.exists():
                pc.die(f"{self.overrides} does not exist yet: run `fit` first")
            return self.overrides
        pc.die(f"unknown tag {tag}")

    def project(self, tag, entry):
        extra = [] if tag == "base" else [self.tag_script(tag)]
        return pc.entry_project(self.cfg, self.cli, entry, exclude=[self.overrides], extra=extra, tag=tag.replace(" ", "_"))

    # -- probe data
    def probe(self):
        p = self.work / "probe.json"
        if not p.exists():
            pc.die("run the `probe` step first")
        return json.loads(p.read_text(encoding="utf-8"))

    def targets(self):
        if self._targets is None:
            self._targets = build_targets(self)
        return self._targets


def apply_xf(xf, pts):
    """Layer space -> parent comp space for a 2D layer: (p - anchor) * scale, rotate (clockwise, y down), + position."""
    a = np.array(xf["anchor"][:2], float)
    s = np.array(xf["scale"][:2], float) / 100.0
    th = np.radians(float(xf["rotation"] or 0))
    c, sn = np.cos(th), np.sin(th)
    q = (pts - a) * s
    return np.stack([c * q[:, 0] - sn * q[:, 1], sn * q[:, 0] + c * q[:, 1]], 1) + np.array(xf["position"][:2], float)


def grid(ctx, size):
    gx = np.arange(0, size[0] + 1e-6, ctx.SX)
    gy = np.arange(0, size[1] + 1e-6, ctx.SY)
    return np.array([(x, y) for y in gy for x in gx], float)


def xf_warnings(what, xf):
    w = []
    if xf.get("animated"):
        w.append("animated transform (the fit uses its value at time 0)")
    if xf.get("parent"):
        w.append(f"parented to {xf['parent']} (the parent's transform is ignored)")
    if xf.get("threeD"):
        w.append("3D layer (treated as 2D)")
    for m in w:
        print(f"warning: {what}: {m}", file=sys.stderr)


def build_targets(ctx):
    pr = ctx.probe()
    out = {}
    for name in ctx.only:
        e = ctx.entries.get(name)
        if e is None:
            pc.die(f"{name} is not in glow_fit.targets")
        top = pr["tops"][e["comp"]]
        fps = float(top["fps"])
        info = pc.probe(e["reference"])
        off = int(e["offset"])
        n = min(info["frames"] - off, int(round(top["duration"] * fps)))
        inst = []
        if e["comp"] in pr["glow"]:
            inst.append(dict(glow=e["comp"], st=0.0, i=0, o=n, xf=[pr["glow"][e["comp"]]["xf"]]))
        for h in pr["hosts"]:
            if h["host"] != e["comp"] or not h["enabled"]:
                continue
            if abs(h.get("stretch", 100) - 100) > 1e-6 or h.get("timeRemap"):
                print(f"warning: {h['host']} #{h['index']}: time-stretched or remapped precomp, skipped", file=sys.stderr)
                continue
            xf_warnings(f"{h['host']} #{h['index']}", h["xf"])
            i, o = max(0, int(round(h["inPoint"] * fps))), min(n, int(round(h["outPoint"] * fps)))
            if o > i:
                inst.append(dict(glow=h["source"], st=h["startTime"] * fps, i=i, o=o,
                                 xf=[pr["glow"][h["source"]]["xf"], h["xf"]]))
        if not inst:
            pc.die(f"{name}: comp {e['comp']} neither holds selected layers nor hosts a comp that does")
        out[name] = dict(entry=e, top=e["comp"], size=(int(top["width"]), int(top["height"])), fps=fps,
                         n=n, off=off, inst=inst, rate=info["rate"])
    return out


# ---------------------------------------------------------------- numerics

def load(p):
    return np.asarray(Image.open(p).convert("RGB")).astype(np.float32)


def ds(a, k):
    h, w = a.shape[0] // k * k, a.shape[1] // k * k
    a = a[:h, :w]
    return a.reshape(h // k, k, w // k, k, *a.shape[2:]).mean((1, 3))


def open_mask(U):
    """Trust only pixels whose 3x3 neighbourhood is open (edges of occluders alias at low resolution)."""
    m = U.copy()
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            m = np.minimum(m, np.roll(np.roll(U, dy, 0), dx, 1))
    return (m > 0.02).astype(np.float32)


_BASIS = {}


def basis(ctx, inst, size, k, glow_size):
    """Cone basis in target-comp pixels on a grid downsampled by k: (npix, ncones)."""
    key = (json.dumps(inst["xf"], sort_keys=True), size, k)
    if key not in _BASIS:
        pts = grid(ctx, glow_size)
        for xf in inst["xf"]:
            pts = apply_xf(xf, pts)
        w, h = size[0] // k, size[1] // k
        ys, xs = np.mgrid[0:h, 0:w]
        xs = (xs + 0.5) * k
        ys = (ys + 0.5) * k
        B = [np.clip(1 - np.hypot(xs - cx, ys - cy) / ctx.R, 0, 1).ravel() for cx, cy in pts]
        _BASIS[key] = np.stack(B, 1).astype(np.float32)
    return _BASIS[key]


def nnls(A, b, lam, x0=None):
    """Non-negative ridge least squares in [0, 1] (ridge towards x0, default 0), projected coordinate descent."""
    G = A.T @ A + lam * np.eye(A.shape[1], dtype=A.dtype)
    h = A.T @ b
    if x0 is not None:
        h = h + lam * x0
    x = np.clip(np.linalg.solve(G, h), 0, 1)
    d = np.diag(G)
    for _ in range(300):
        old = x.copy()
        for i in range(len(x)):
            x[i] = min(1.0, max(0.0, x[i] - (G[i] @ x - h[i]) / d[i]))
        if np.abs(x - old).max() < 1e-7:
            break
    return x


def interp_keys(p, tau):
    taus = np.array(p["taus"])
    C = np.array(p["colors"])
    if tau <= taus[0]:
        return C[0]
    if tau >= taus[-1]:
        return C[-1]
    j = np.searchsorted(taus, tau) - 1
    f = (tau - taus[j]) / (taus[j + 1] - taus[j])
    return C[j] * (1 - f) + C[j + 1] * f


def glow_size(ctx, g):
    return tuple(ctx.probe()["glow"][g]["source"])


def rms(R, X, W):
    return float(np.sqrt((W * (R - X) ** 2).sum() / max(W.sum() * 3, 1)))


# ---------------------------------------------------------------- steps

def step_probe(ctx, a):
    base = ctx.project("base", next(iter(ctx.entries.values())))
    js = pc.jsx_with_data("probe_glow.jsx", {"select": ctx.sel}, ctx.work / "_probe.jsx")
    pr = pc.script_json(ctx.cli, js, base)
    if not pr["glow"]:
        pc.die(f"no layer matches {ctx.sel}")
    out = {"glow": {}, "hosts": pr["hosts"], "slugs": {}}
    for g, d in pr["glow"].items():
        ls = d["layers"]
        first = ls[0]
        if first["source"] is None:
            pc.die(f"{g} / {first['name']}: the selected layer has no source to replace")
        for l in ls[1:]:
            if l["source"] != first["source"] or l["xf"] != first["xf"] or abs(l["startTime"] - first["startTime"]) > 1e-6:
                print(f"warning: {g}: selected layers differ in source size, transform or start time; "
                      f"the fit uses #{first['index']}'s", file=sys.stderr)
        xf_warnings(f"{g} #{first['index']}", first["xf"])
        out["glow"][g] = dict(source=first["source"], xf=first["xf"], start=first["startTime"], fps=d["fps"],
                              layers=[l["index"] for l in ls])
        out["slugs"][pc.slug(g)] = g
    out["tops"] = pc.comp_info(ctx.cli, base, [e["comp"] for e in ctx.entries.values()], ctx.work)
    (ctx.work / "probe.json").write_text(json.dumps(out, indent=1), encoding="utf-8")
    for g, d in out["glow"].items():
        print(f"glow comp {g}: {len(d['layers'])} layer(s), source {d['source'][0]}x{d['source'][1]}, start {d['start']:.3f} s")
    for h in out["hosts"]:
        print(f"  instance: {h['host']} #{h['index']} -> {h['source']} start {h['startTime']:.3f} s, "
              f"in {h['inPoint']:.3f} out {h['outPoint']:.3f}")


def frames(ctx, t):
    return list(range(0, t["n"], ctx.STEP))


def step_render(ctx, a):
    for tag in a.tags.split(","):
        for key, t in ctx.targets().items():
            out = ctx.cache / tag / key
            shutil.rmtree(out, ignore_errors=True)
            pc.render_samples(ctx.cli, ctx.project(tag, t["entry"]), t["top"], frames(ctx, t), t["fps"], ctx.STEP, ctx.Q, out)
            print(tag, key, len(frames(ctx, t)), "frames", flush=True)


def ref_vf(ctx, t):
    w, h = t["size"][0] // ctx.Q, t["size"][1] // ctx.Q
    pre = (ctx.g.get("ref_vf") or {}).get(t["entry"]["name"])
    return (pre + "," if pre else "") + f"scale={w}:{h}:flags=area"


def step_ref(ctx, a):
    for key, t in ctx.targets().items():
        out = ctx.cache / "ref" / key
        fr = frames(ctx, t)
        pc.extract_frames(t["entry"]["reference"], [k + t["off"] for k in fr], out, name="{:05d}.png",
                          vf=ref_vf(ctx, t), key=lambda v, o=t["off"]: v - o)
        print("ref", key, len(fr), flush=True)


def owner(t, k):
    live = [x for x in t["inst"] if x["i"] <= k < x["o"]]
    return live[0] if len(live) == 1 else None


def target_rows(ctx, key, refine):
    t = ctx.targets()[key]
    rows = []
    for k in frames(ctx, t):
        ins = owner(t, k)
        if ins is None:
            continue
        f = f"{k:05d}.png"
        R = load(ctx.cache / "ref" / key / f)
        N = load(ctx.cache / "nn" / key / f)
        U = ((load(ctx.cache / "unit" / key / f) - N).mean(2) / (ctx.G * 255)).clip(0, None)
        FIT = load(ctx.cache / "fit" / key / f) if refine else None
        rows.append(dict(k=k, inst=ins, tau=(k - ins["st"]) / t["fps"], R=R, N=N, U=U, W=open_mask(U), T=R - N, FIT=FIT))
    return rows


def step_fit(ctx, a):
    pfile = ctx.work / "params.json"
    params = json.loads(pfile.read_text(encoding="utf-8")) if a.refine else {}
    lam = float(ctx.g["lambda"])
    k2 = ctx.Q * ctx.FDS
    out = {}
    for key, t in ctx.targets().items():
        rows = target_rows(ctx, key, a.refine)
        for r in rows:
            g = r["inst"]["glow"]
            B = basis(ctx, r["inst"], t["size"], k2, glow_size(ctx, g))
            U = ds(r["U"], ctx.FDS).ravel()
            W = ds(r["W"], ctx.FDS).ravel()
            T = ds(r["T"], ctx.FDS).reshape(-1, 3)
            if a.refine:
                c_prev = np.array(interp_keys(params[g], r["tau"]))
                pred = U[:, None] * (B @ c_prev) * 255
                T = T + (pred - ds(r["FIT"] - r["N"], ctx.FDS).reshape(-1, 3))
            sw = np.sqrt(W)
            A = (U[:, None] * B) * 255 * sw[:, None]
            cs = np.stack([nnls(A, T[:, ch] * sw, lam) for ch in range(3)], 1)
            out.setdefault(g, []).append((r["tau"], cs.tolist()))
        print("fit", key, len(rows), "frames", flush=True)
    if a.xfade and not a.refine:
        prior = {g: dict(taus=[k[0] for k in sorted(ks)], colors=[k[1] for k in sorted(ks)]) for g, ks in out.items()}
        for key, t in ctx.targets().items():
            if not xwindows(ctx, t):
                continue
            try:
                xk = xfit(ctx, key, prior)
            except FileNotFoundError as e:
                pc.die(f"{key} has cross-fades but no cross-fade frames ({e.filename}): run xrender and xref, or pass --xfade 0")
            for g, ks in xk.items():
                fps_g = float(ctx.probe()["glow"][g]["fps"])
                byf = {round(tau * fps_g): (tau, c) for tau, c in out.get(g, [])}
                byf.update({round(tau * fps_g): (tau, c) for tau, c in ks})
                out[g] = list(byf.values())
            print("xfade", key, {g: len(v) for g, v in xk.items()}, flush=True)
    res = json.loads(pfile.read_text(encoding="utf-8")) if (a.only and pfile.exists()) else {}
    for g, ks in out.items():
        ks = sorted(ks, key=lambda x: x[0])
        taus = np.array([k[0] for k in ks])
        C = np.array([k[1] for k in ks])
        if len(C) > 2 and int(ctx.g["smooth"]):
            Cs = C.copy()
            Cs[1:-1] = (C[:-2] + 2 * C[1:-1] + C[2:]) / 4
            C = Cs
        size = glow_size(ctx, g)
        res[g] = dict(grid=grid(ctx, size).tolist(), radius=ctx.R, size=list(size),
                      start=ctx.probe()["glow"][g]["start"], taus=taus.round(5).tolist(), colors=C.round(5).tolist())
    pfile.write_text(json.dumps(res), encoding="utf-8")
    print("wrote", pfile, {g: len(v["taus"]) for g, v in res.items()})
    step_emit(ctx, a)


def step_emit(ctx, a):
    p = json.loads((ctx.work / "params.json").read_text(encoding="utf-8"))
    fields = {}
    for g, d in p.items():
        C = np.array(d["colors"])
        blobs = [dict(c=xy, v=C[:, b].round(4).tolist()) for b, xy in enumerate(d["grid"])
                 if C[:, b].max() >= 1.0 / 512]          # skip cones that never reach half a level
        fields[g] = dict(w=d["size"][0], h=d["size"][1], r=d["radius"], start=d["start"],
                         t=[round(tt - d["start"], 5) for tt in d["taus"]], b=blobs)
    pc.jsx_with_data("glow_overrides.jsx", {"select": ctx.sel, "prefix": ctx.g["prefix"], "fields": fields}, ctx.overrides)
    print("wrote", ctx.overrides, {g: len(v["b"]) for g, v in fields.items()})


def step_check(ctx, a):
    pfile = ctx.work / "params.json"
    p = json.loads(pfile.read_text(encoding="utf-8")) if pfile.exists() else None
    for key, t in ctx.targets().items():
        acc = {"nn": [], "pred": [], "fit": []}
        for r in target_rows(ctx, key, a.with_fit):
            W = r["W"][..., None]
            acc["nn"].append(rms(r["R"], r["N"], W))
            g = r["inst"]["glow"]
            if p and g in p:
                B = basis(ctx, r["inst"], t["size"], ctx.Q, glow_size(ctx, g))
                c = np.array(interp_keys(p[g], r["tau"]))
                pred = r["N"] + (r["U"].ravel()[:, None] * (B @ c) * 255).reshape(r["N"].shape)
                acc["pred"].append(rms(r["R"], pred, W))
            if r["FIT"] is not None:
                acc["fit"].append(rms(r["R"], r["FIT"], W))
        print(key, "RMS levels", {k: round(float(np.mean(v)), 3) for k, v in acc.items() if v})


# ---------------------------------------------------------------- cross-fades

def xwindows(ctx, t):
    """Frame ranges [a, b) where two or more instances are live, padded by XPAD frames."""
    out = []
    for k in range(t["n"]):
        if sum(x["i"] <= k < x["o"] for x in t["inst"]) > 1:
            if out and out[-1][1] == k:
                out[-1][1] = k + 1
            else:
                out.append([k, k + 1])
    return [(max(0, a - ctx.XPAD), min(t["n"], b + ctx.XPAD)) for a, b in out]


def xframes(ctx, w):
    return [k for k in range(w[0], w[1]) if k % ctx.XSTEP == 0]


def live_glows(t, w):
    return sorted({x["glow"] for x in t["inst"] if x["i"] < w[1] and x["o"] > w[0]})


def step_xrender(ctx, a):
    for key, t in ctx.targets().items():
        for w in xwindows(ctx, t):
            for tag in a.tags.split(","):
                subs = [f"unit1-{pc.slug(g)}" for g in live_glows(t, w)] if tag == "unit1" else [tag]
                for sub in subs:
                    out = ctx.cache / "x" / sub / key / f"w{w[0]}"
                    shutil.rmtree(out, ignore_errors=True)
                    pc.render_samples(ctx.cli, ctx.project(sub, t["entry"]), t["top"], xframes(ctx, w), t["fps"],
                                      ctx.XSTEP, ctx.Q, out)
                    print(sub, key, w, len(xframes(ctx, w)), flush=True)


def step_xref(ctx, a):
    for key, t in ctx.targets().items():
        for w in xwindows(ctx, t):
            fr = xframes(ctx, w)
            pc.extract_frames(t["entry"]["reference"], [k + t["off"] for k in fr], ctx.cache / "x" / "ref" / key / f"w{w[0]}",
                              name="{:05d}.png", vf=ref_vf(ctx, t), key=lambda v, o=t["off"]: v - o)
            print("xref", key, w, len(fr), flush=True)


def xrows(ctx, key, with_fit=False):
    t = ctx.targets()[key]
    rows, warned = [], set()
    for w in xwindows(ctx, t):
        for k in xframes(ctx, w):
            f = f"{k:05d}.png"
            R = load(ctx.cache / "x" / "ref" / key / f"w{w[0]}" / f)
            N = load(ctx.cache / "x" / "nn" / key / f"w{w[0]}" / f)
            live = [x for x in t["inst"] if x["i"] <= k < x["o"]]
            if len({x["glow"] for x in live}) < len(live):
                # Two live instances of the same comp share one transfer probe: they cannot be told
                # apart, so these frames keep the keys interpolated from the single-instance fit.
                if w[0] not in warned:
                    print(f"warning: {key} frames {w[0]}-{w[1]}: one comp overlaps itself, no joint fit there",
                          file=sys.stderr)
                    warned.add(w[0])
                continue
            Us = []
            for x in live:
                Uu = load(ctx.cache / "x" / f"unit1-{pc.slug(x['glow'])}" / key / f"w{w[0]}" / f)
                Us.append(((Uu - N).mean(2) / (ctx.G * 255)).clip(0, None))
            FIT = load(ctx.cache / "x" / "fit" / key / f"w{w[0]}" / f) if with_fit else None
            rows.append(dict(k=k, w=w[0], live=[(x, (k - x["st"]) / t["fps"]) for x in live], R=R, N=N, Us=Us,
                             W=open_mask(sum(Us)), T=R - N, FIT=FIT))
    return rows


def xfit(ctx, key, prior):
    """Joint keys of the cross-fade frames of one target: {glow: [(tau, colours)]}."""
    t = ctx.targets()[key]
    lam = float(ctx.g["xfade"]["lambda"])
    k2 = ctx.Q * ctx.FDS
    out = {}
    for r in xrows(ctx, key):
        Bs = [basis(ctx, x, t["size"], k2, glow_size(ctx, x["glow"])) for x, _ in r["live"]]
        sw = np.sqrt(ds(r["W"], ctx.FDS).ravel())[:, None]
        A = np.concatenate([(ds(U, ctx.FDS).ravel()[:, None] * B) * 255 * sw for U, B in zip(r["Us"], Bs)], 1)
        T = ds(r["T"], ctx.FDS).reshape(-1, 3)
        x0 = np.concatenate([np.array(interp_keys(prior[x["glow"]], tau)) if x["glow"] in prior
                             else np.zeros((B.shape[1], 3)) for (x, tau), B in zip(r["live"], Bs)], 0)
        cs = np.stack([nnls(A, T[:, ch] * sw[:, 0], lam, x0[:, ch]) for ch in range(3)], 1)
        o = 0
        for (x, tau), B in zip(r["live"], Bs):
            nb = B.shape[1]
            out.setdefault(x["glow"], []).append((tau, cs[o:o + nb].tolist()))
            o += nb
    return out


def step_xcheck(ctx, a):
    p = json.loads((ctx.work / "params.json").read_text(encoding="utf-8"))
    for key, t in ctx.targets().items():
        res = {}
        for r in xrows(ctx, key, a.with_fit):
            W = r["W"][..., None]
            pred = r["N"].copy()
            for (x, tau), U in zip(r["live"], r["Us"]):
                if x["glow"] in p:
                    c = np.array(interp_keys(p[x["glow"]], tau))
                    B = basis(ctx, x, t["size"], ctx.Q, glow_size(ctx, x["glow"]))
                    pred += (U.ravel()[:, None] * (B @ c) * 255).reshape(r["N"].shape)
            d = res.setdefault(r["w"], {"nn": [], "pred": [], "fit": []})
            d["nn"].append(rms(r["R"], r["N"], W))
            d["pred"].append(rms(r["R"], pred, W))
            if r["FIT"] is not None:
                d["fit"].append(rms(r["R"], r["FIT"], W))
        for w, d in res.items():
            print(key, "window", w, "RMS levels", {k: round(float(np.mean(v)), 3) for k, v in d.items() if v})


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("config", type=Path)
    ap.add_argument("step", choices=["probe", "render", "ref", "xrender", "xref", "fit", "emit", "check", "xcheck"])
    ap.add_argument("--cli")
    ap.add_argument("--only", default="", help="comma-separated target entries")
    ap.add_argument("--tags", default="nn,unit", help="render: nn,unit[,fit]; xrender: nn,unit1[,fit]")
    ap.add_argument("--refine", action="store_true", help="fit: second pass on the residual of the fit render")
    ap.add_argument("--xfade", type=int, default=1, help="fit: add the joint cross-fade keys (needs xrender/xref)")
    ap.add_argument("--with-fit", action="store_true", help="check/xcheck: include the fit render")
    a = ap.parse_args()
    if a.step == "xrender" and a.tags == "nn,unit":
        a.tags = "nn,unit1"
    ctx = Ctx(a)
    try:
        {"probe": step_probe, "render": step_render, "ref": step_ref, "xrender": step_xrender, "xref": step_xref,
         "fit": step_fit, "emit": step_emit, "check": step_check, "xcheck": step_xcheck}[a.step](ctx, a)
    except pc.ParityError as e:
        pc.die(str(e))


if __name__ == "__main__":
    main()
