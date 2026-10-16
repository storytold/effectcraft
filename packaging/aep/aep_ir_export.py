"""Export a py-aep project to versioned, loss-accounting JSON.

Run with py-aep/.venv/bin/python aep_ir_export.py source.aep -o output.json
"""

import argparse
import base64
import hashlib
import json
import re
import math
import struct
import sys
import unicodedata
from enum import Enum
from pathlib import Path, PureWindowsPath

if sys.version_info < (3, 11):
    raise SystemExit("AEP export requires Python 3.11 or newer; set EFFECTCRAFT_AEP_PYTHON to a supported interpreter")

import py_aep
from fontTools.ttLib import TTFont
from py_aep.binary.chunk import ListChunk
from py_aep.cos import CosName, Stream, cos_get
from py_aep.models.layers.shape_layer import ShapeLayer
from py_aep.models.layers.text_layer import TextLayer
from py_aep.models.properties.gradient import Gradient
from py_aep.models.properties.property_group import PropertyGroup
from py_aep.models.text.ranges import _char_run_spans, _kern_run_spans, _para_run_spans, u16_len
from py_aep.parsers.layer import parse_layer


SCHEMA_VERSION = 1
CHAR_STYLE_FIELDS = "font font_size fill_color stroke_color apply_fill apply_stroke stroke_over_fill stroke_width faux_bold faux_italic auto_leading leading kerning auto_kern_type tracking baseline_shift horizontal_scale vertical_scale font_caps_option font_baseline_option ligature no_break baseline_direction tsume digit_set line_join_type all_caps small_caps superscript subscript"
PARA_STYLE_FIELDS = "justification first_line_indent start_indent end_indent space_before space_after leading_type auto_hyphenate hanging_roman every_line_composer direction composer_engine"


def is_file(path):
    """Whether `path` is a file; a path this system can't even ask about (a Windows path saved
    in the project, longer than a file name may be here) is not one."""
    try:
        return path.is_file()
    except OSError:
        return False


def windows_bytes_to_text(text):
    """Bytes that weren't UTF-8 (kept as lone surrogates, e.g. a Windows-1252 ’ saved by AE on
    Windows) become the Windows-1252 character they stand for, so the IR is valid UTF-8."""
    def char(m):
        return bytes([ord(m.group()) - 0xDC00]).decode("cp1252", errors="replace")
    return re.sub("[\udc80-\udcff]", char, text)


def media_name(name):
    return unicodedata.normalize("NFC", name).casefold()


# Popup defaults where py-aep's shared last value is known to leak between instances. Measured
# on the SceneYard corpus: CC Page Turn's untouched Controls reads 6 (Bottom Right Corner) in 12
# of 13 instances, and aerender draws Paper News Highlight's untouched one flat as with 6.
POPUP_DEFAULTS = {"CC Page Turn-0008": 6}


class Exporter:
    def __init__(self, font_search_root=None):
        self.losses = []
        self.media_issues = []
        self.media_index = None
        self.source_dir = None
        self.font_index = None
        self.root_font_index = None
        self.font_hashes = {}
        self.font_search_root = Path(font_search_root).resolve() if font_search_root else None
        self.explicit_mattes = False
        self.effect_definitions = {}
        self.authored_values = None
        if self.font_search_root and not self.font_search_root.is_dir():
            raise ValueError(f"No font search root: {self.font_search_root}")

    def loss(self, scope, reason, detail):
        self.losses.append({"scope": scope, "reason": reason, "detail": str(detail)[:500]})

    def read(self, obj, name, scope, default=None):
        try:
            return getattr(obj, name)
        except Exception as exc:
            self.loss(scope + "/" + name, "read_error", f"{type(exc).__name__}: {exc}")
            return default

    @staticmethod
    def puppet_mesh(data):
        """Decode the two observed v2 PEdt topology layouts, rejecting ambiguous bytes."""
        if len(data) < 29 or data[:9] != b"\x0a\xd0\xbe\xef\x00\x00\x00\x02\x01":
            return None
        vertices, indices, _, _ = struct.unpack_from(">4I", data, 13)
        if not (3 <= vertices <= 20_000 and 3 <= indices <= 120_000 and indices % 3 == 0):
            return None
        vertex_end = 29 + 16 * vertices
        if vertex_end > len(data) or len(data) > 8_000_000:
            return None
        points = [struct.unpack_from(">2d", data, 29 + 16 * i) for i in range(vertices)]
        if any(not math.isfinite(x) or abs(x) > 1_000_000 for point in points for x in point):
            return None
        # PEdt embeds other data between the rest vertices and topology. Its
        # topology begins on the same 4-byte phase as the vertex block.
        for offset in range(vertex_end, len(data) - 4 * indices + 1, 4):
            first = struct.unpack_from(">3I", data, offset)
            if any(i >= vertices for i in first):
                continue
            edge = first[0] == first[2]
            # Edge layout stores (a,b,a,c,b,c) for each face; the header's
            # index count remains three per face, so the byte stream doubles it.
            # Rotated layout (meshes with a Triangle Density, SceneYard "Hand
            # balance collage") stores each face twice, (a,b,c) then (b,c,a).
            pair = (not edge and offset + 24 <= len(data)
                    and struct.unpack_from(">3I", data, offset + 12) == (first[1], first[2], first[0]))
            stored = 2 * indices if edge or pair else indices
            if offset + 4 * stored > len(data):
                continue
            preview = struct.unpack_from(f">{min(12, stored)}I", data, offset)
            if any(i >= vertices for i in preview):
                continue
            if edge and any(preview[i] != preview[i + 2] or preview[i + 1] != preview[i + 4] or preview[i + 3] != preview[i + 5] for i in range(0, len(preview) - 5, 6)):
                continue
            if not edge and any(len(set(preview[i:i + 3])) != 3 for i in range(0, len(preview) - 2, 3)):
                continue
            if pair and any(preview[i + 3:i + 6] != (preview[i + 1], preview[i + 2], preview[i]) for i in range(0, len(preview) - 5, 6)):
                continue
            values = struct.unpack_from(f">{stored}I", data, offset)
            if any(i >= vertices for i in values):
                continue
            if edge:
                groups = [values[i:i + 6] for i in range(0, stored, 6)]
                if any(a != c or b != e or d != f for a, b, c, d, e, f in groups):
                    continue
                triangles = [(a, d, b) for a, b, _, d, _, _ in groups]
            elif pair:
                groups = [values[i:i + 6] for i in range(0, stored, 6)]
                if any((d, e, f) != (b, c, a) for a, b, c, d, e, f in groups):
                    continue
                triangles = [(a, b, c) for a, b, c, _, _, _ in groups]
            else:
                triangles = [values[i:i + 3] for i in range(0, indices, 3)]
            if not triangles or any(len({a, b, c}) != 3 or
                    (points[b][0] - points[a][0]) * (points[c][1] - points[a][1]) -
                    (points[b][1] - points[a][1]) * (points[c][0] - points[a][0]) <= 1e-7
                    for a, b, c in triangles):
                continue
            return {"vertices": points, "triangles": triangles}
        return None

    def puppet_state(self, node, name, data, scope):
        if data is None:
            return
        if len(data) > 8_000_000:
            self.loss(scope, "puppet_state", f"oversized {name} state")
            return
        node[name] = base64.b64encode(data).decode("ascii")

    @staticmethod
    def vector_source_layer(source):
        """Read the saved AI/PDF layer binding without needing the external file."""
        opti = getattr(source, "_opti", None)
        name = getattr(opti, "text_layer_name", "")
        if not name:
            return None
        index = getattr(getattr(source, "_sspc", None), "layer_index", None)
        count = getattr(opti, "text_document_layers", 0)
        if not isinstance(index, int) or not 0 <= index < count <= 65535:
            raise ValueError("invalid vector source layer index/count")
        data = opti.tobytes()
        if len(data) < 32:
            raise ValueError("truncated vector artwork bounds")
        bounds = [v / 65536 for v in struct.unpack_from(">4i", data, 16)]
        x0, y0, x1, y1 = bounds
        if not (0 < x1 - x0 <= 30000 and 0 < y1 - y0 <= 30000):
            raise ValueError("invalid vector artwork bounds")
        return {"index": index, "name": name, "layer_size": True, "crop_bounds": bounds}

    @staticmethod
    def psd_source_layer(source, width, height):
        """The saved binding of a single Photoshop layer imported as footage (Footage: Choose
        Layer); None for a merged import. ``layer_size`` when the footage is the layer's own
        bounds rather than the document size."""
        opti = getattr(source, "_opti", None)
        if getattr(opti, "asset_type", None) != "8BPS" or getattr(opti, "psd_flattened", True):
            return None
        count = getattr(opti, "psd_layer_count", 0)
        index = getattr(opti, "psd_layer_index", None)
        if not count:
            return None
        if not isinstance(index, int) or not 0 <= index < count <= 65535:
            raise ValueError("invalid Photoshop source layer index/count")
        rect = (opti.psd_layer_right - opti.psd_layer_left, opti.psd_layer_bottom - opti.psd_layer_top)
        canvas = (opti.psd_canvas_width, opti.psd_canvas_height)
        size = (width, height)
        layer_size = size != canvas and size == rect
        binding = {"index": index, "name": getattr(opti, "psd_group_name", "") or "", "layer_size": layer_size}
        if layer_size:
            # AE trims a Layer Size footage to the layer's visible pixels (SceneYard Student
            # Computer Workflow: 894x929 out of a 1792x1024 layer record).
            binding["crop_bounds"] = [float(opti.psd_layer_left), float(opti.psd_layer_top),
                                      float(opti.psd_layer_right), float(opti.psd_layer_bottom)]
        return binding

    def scan_fonts(self, root):
        index = {}
        for path in root.rglob("*"):
            if not is_file(path) or path.suffix.lower() not in (".ttf", ".otf"):
                continue
            try:
                font = TTFont(path, lazy=True)
                try:
                    name = font["name"]
                    ps = name.getDebugName(6)
                    if ps:
                        index.setdefault(ps, []).append((path, name.getDebugName(5)))
                finally:
                    font.close()
            except Exception:
                continue
        return index

    def font_hash(self, path):
        if path not in self.font_hashes:
            with path.open("rb") as handle:
                self.font_hashes[path] = hashlib.file_digest(handle, "sha256").hexdigest()
        return self.font_hashes[path]

    def font_file(self, postscript, version):
        if not postscript:
            return None
        if self.font_index is None:
            self.font_index = self.scan_fonts(self.source_dir)
        candidates = self.font_index.get(postscript, [])
        if candidates:
            path, found_version = min(candidates, key=lambda entry: (entry[1] != version,
                                                                     len(entry[0].relative_to(self.source_dir).parts),
                                                                     str(entry[0])))
            return {"path": str(path.resolve()), "sha256": self.font_hash(path),
                    "origin": "project_folder",
                    "status": "verified_ps_version" if version and found_version == version else "verified_ps"}
        if not self.font_search_root:
            return None
        if self.root_font_index is None:
            self.root_font_index = self.scan_fonts(self.font_search_root)
        candidates = self.root_font_index.get(postscript, [])
        if version:
            candidates = [entry for entry in candidates if entry[1] == version]
        if not candidates or len({self.font_hash(path) for path, _ in candidates}) != 1:
            return None
        path, _ = min(candidates, key=lambda entry: (len(entry[0].relative_to(self.font_search_root).parts),
                                                     str(entry[0])))
        return {"path": str(path.resolve()), "sha256": self.font_hash(path),
                "origin": "search_root",
                "status": "verified_ps_version" if version else "verified_ps"}

    def value(self, obj, scope, depth=0):
        if obj is None or isinstance(obj, (str, bool, int)):
            return obj
        if isinstance(obj, float):
            if math.isfinite(obj):
                return obj
            self.loss(scope, "nonfinite_number", obj)
            return {"type": "opaque", "class": "float", "repr": repr(obj)}
        if isinstance(obj, Enum):
            return self.value(obj.value, scope, depth + 1)
        if isinstance(obj, CosName):
            return {"type": "CosName", "value": obj.value}
        if isinstance(obj, bytes):
            return {"type": "bytes", "base64": base64.b64encode(obj).decode("ascii")}
        if isinstance(obj, Stream):
            return {"type": "Stream", "dictionary": self.value(obj.dictionary, scope + "/dictionary", depth + 1),
                    "data": self.value(obj.data, scope + "/data", depth + 1)}
        if depth > 12:
            self.loss(scope, "value_depth_limit", type(obj).__name__)
            return {"type": "opaque", "class": type(obj).__name__, "repr": repr(obj)[:200]}
        if isinstance(obj, (list, tuple)):
            return [self.value(v, f"{scope}/{i}", depth + 1) for i, v in enumerate(obj)]
        if isinstance(obj, dict):
            return {str(k): self.value(v, f"{scope}/{k}", depth + 1) for k, v in obj.items()}
        cls = type(obj).__name__
        fields = {
            "TextDocument": "text font font_size fill_color stroke_color apply_fill apply_stroke stroke_width justification tracking leading auto_leading baseline_shift horizontal_scale vertical_scale box_text box_text_size box_text_pos all_caps small_caps faux_bold faux_italic",
            "Shape": "vertices in_tangents out_tangents closed feather_points",
            "FeatherPoint": "segment_loc rel_seg_loc radius type tension",
            "Gradient": "version color_stops alpha_stops",
            "GradientColorStop": "offset midpoint color",
            "GradientAlphaStop": "offset midpoint alpha",
            "MarkerValue": "comment chapter url frame_target cue_point_name event_cue_point duration frame_duration label navigation protected_region parameters",
            "KeyframeEase": "speed influence",
        }.get(cls)
        if fields:
            data = {}
            for field in fields.split():
                try:
                    data[field] = self.value(getattr(obj, field), f"{scope}/{field}", depth + 1)
                except (AttributeError, KeyError):
                    pass
                except Exception as exc:
                    self.loss(f"{scope}/{field}", "read_error", f"{type(exc).__name__}: {exc}")
            if cls == "TextDocument":
                font = self.read(obj, "font_object", scope)
                data["font_version"] = self.read(font, "version", scope + "/font_object") if font else None
                data["font_file"] = self.font_file(data.get("font"), data["font_version"])
                self.text_runs(obj, data, scope)
            if cls == "Shape" and getattr(obj, "_is_mask", False) and isinstance(getattr(obj, "_layer", None), (ShapeLayer, TextLayer)):
                # AE stores masks on shape and text layers in pixel space; py-aep
                # also multiplies them by layer size as it does for footage masks.
                width, height = obj._layer.width, obj._layer.height
                if width and height:
                    for field in ("vertices", "in_tangents", "out_tangents"):
                        data[field] = [[x / width, y / height] for x, y in data[field]]
                else:
                    self.loss(scope, "mask_unit_error", "shape or text layer has zero width or height")
            return {"type": cls, "data": data}
        self.loss(scope, "opaque_value", cls)
        return {"type": "opaque", "class": cls, "repr": repr(obj)[:500]}

    def text_runs(self, doc, data, scope):
        visible = u16_len(doc.text)
        for label, spans, fields in (("character_runs", _char_run_spans(doc), CHAR_STYLE_FIELDS),
                                     ("paragraph_runs", _para_run_spans(doc), PARA_STYLE_FIELDS)):
            runs = []
            for i, (start, raw_end, raw_style) in enumerate(spans):
                end = min(raw_end, visible)
                start_visible = min(start, visible)
                run_scope = f"{scope}/{label}/{i}"
                try:
                    view = doc.character_range(start_visible, end)
                    style = {field: self.value(self.read(view, field, run_scope),
                                               f"{run_scope}/{field}") for field in fields.split()}
                    if label == "character_runs":
                        font = self.read(view, "font_object", run_scope)
                        style["font_version"] = self.read(font, "version", run_scope + "/font_object") if font else None
                        style["font_file"] = self.font_file(style.get("font"), style["font_version"])
                except Exception as exc:
                    self.loss(run_scope, "text_range_error", f"{type(exc).__name__}: {exc}")
                    style = {}
                runs.append({"start_utf16": start_visible, "end_utf16": end,
                             "raw_end_utf16": raw_end, "style": style,
                             "raw_style": self.value(raw_style, run_scope + "/raw_style")})
            data[label] = runs
        data["kerning_runs"] = [{"start_utf16": min(start, visible),
                                 "end_utf16": min(end, visible), "raw_end_utf16": end,
                                 "raw_style": self.value(raw, f"{scope}/kerning_runs/{i}")}
                                for i, (start, end, raw) in enumerate(_kern_run_spans(doc))]
        data["leading_edge_kerning"] = self.value(cos_get(doc._doc, "0", "7"),
                                                   scope + "/leading_edge_kerning")

    def keyframe(self, key, scope):
        fields = ("time", "value", "in_interpolation_type", "out_interpolation_type",
                  "in_temporal_ease", "out_temporal_ease", "in_spatial_tangent",
                  "out_spatial_tangent", "temporal_auto_bezier", "temporal_continuous",
                  "spatial_auto_bezier", "spatial_continuous", "roving", "label")
        return {name: self.value(self.read(key, name, scope), f"{scope}/{name}") for name in fields}

    @staticmethod
    def plugin_default(definition):
        """An effect parameter's plugin default from py-aep's parameter definition (pard), in the
        IR's units, or `None` where the definition carries none (points, layers, groups)."""
        if not isinstance(definition, dict):
            return None
        kind = int(definition.get("property_control_type", -1))
        # Popups: pard's default index is 0 for most built-in effects whatever their real default
        # (Glow: Color Channels / Behind / Add), so it is not used; see POPUP_DEFAULTS.
        if kind in (4, 5):  # checkbox, colour
            return definition.get("default_value")
        if kind in (1, 2, 3, 10):  # integer, scalar, angle, float slider
            return definition.get("default")
        return None

    @staticmethod
    def value_key(value):
        """A comparable form of a static effect parameter value."""
        if isinstance(value, (list, tuple)):
            return tuple(round(float(v), 5) if isinstance(v, (int, float)) else v for v in value)
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            return round(float(value), 5)
        return value

    @staticmethod
    def authored_effect_values(project):
        """{(effect match name, parameter match name): {values set somewhere}}: where an
        untouched parameter can have picked up another instance's value. Keyframed values count:
        SceneYard Billiard Balls' untouched CC Sphere Rotation X/Z read as the keyframed ball's
        last keys (13, -56) and tilted the other balls; aerender draws them at 0."""
        found = {}

        def visit(effect_name, group, depth):
            if depth > 8:
                return
            for prop in group:
                if isinstance(prop, PropertyGroup):
                    visit(effect_name, prop, depth + 1)
                elif prop._is_live() and not getattr(prop, "expression", ""):
                    keys = getattr(prop, "keyframes", None) or []
                    values = [getattr(key, "value", None) for key in keys] if keys else [prop.value]
                    for value in values:
                        try:
                            key = Exporter.value_key(value)
                            hash(key)
                        except Exception:
                            continue
                        found.setdefault((effect_name, prop.match_name), set()).add(key)

        for comp in getattr(project, "compositions", []) or []:
            for layer in getattr(comp, "layers", []) or []:
                for effect in getattr(layer, "effects", None) or []:
                    try:
                        visit(effect.match_name, effect, 0)
                    except Exception:
                        continue
        return found

    @staticmethod
    def effect_default(prop, definitions=None, authored=None):
        """The plugin default of an effect parameter AE left out of the file (at its default).

        py-aep fills such a parameter from the project's shared parameter record, whose last value
        is whatever another instance of the effect was last set to (SceneYard Student Computer
        Workflow: an untouched Tint's Map White To read as another Tint's dark blue; aerender
        draws it white. Paper News Highlight: an untouched CC Page Turn's Controls read as another
        instance's 3 and turned the page over). `definitions` are py-aep's per-effect parameter
        definitions; without one, py-aep's own default is used. `None` keeps py-aep's value.
        """
        if prop._is_live() or not hasattr(prop, "last_value"):
            return None
        effect = getattr(prop, "parent_property", None)
        parent = effect
        while parent is not None and getattr(parent, "match_name", None) != "ADBE Effect Parade":
            parent = getattr(parent, "parent_property", None)
        if parent is None:
            return None
        value = getattr(prop, "value", None)
        # Only a parameter another instance of the effect sets can have leaked (its last value is
        # the shared record's, possibly 8-bit rounded); others (Levels, Hue/Saturation,
        # third-party effects) keep state py-aep reads correctly.
        if authored is not None and (getattr(effect, "match_name", None), prop.match_name) not in authored:
            return None
        definition = ((definitions or {}).get(getattr(effect, "match_name", None)) or {}).get(prop.match_name)
        default = POPUP_DEFAULTS.get(prop.match_name, Exporter.plugin_default(definition))
        if default is None:
            default = getattr(prop, "default_value", None)
        if default is None or isinstance(default, (str, bytes)) or default == value:
            return None
        if isinstance(value, (list, tuple)) != isinstance(default, (list, tuple)):
            return None
        if isinstance(value, (list, tuple)) and len(default) != len(value):
            return None
        if isinstance(value, bool):
            return bool(default)
        return list(default) if isinstance(default, tuple) else default

    def property(self, prop, path, scope):
        here = scope + "/" + ".".join(map(str, path))
        is_group = isinstance(prop, PropertyGroup)
        node = {"path": path, "kind": "group" if is_group else "property",
                "name": self.read(prop, "name", here),
                "match_name": self.read(prop, "match_name", here),
                "property_type": self.value(self.read(prop, "property_type", here), here),
                "enabled": self.read(prop, "enabled", here),
                "authored": prop._is_live(),
                "elided": self.read(prop, "elided", here)}
        if is_group:
            if node["match_name"] == "ADBE CurvesCustom":
                tdgp = getattr(prop, "_tdgp", None)
                blobs = [chunk.data for group in (tdgp.chunks if tdgp is not None else ())
                         if getattr(group, "list_type", None) == "aRbs"
                         for chunk in group.chunks if chunk.chunk_type == "aRbp"]
                if len(blobs) == 1 and len(blobs[0]) == 1644 and blobs[0][:4] == b"\0\1\0\1":
                    node["curves_state_hex"] = blobs[0].hex()
                else:
                    self.loss(here, "curves_state", "expected one 1644-byte Curves aRbp with header 00010001")
            if node["match_name"] == "APC Colorama":
                # The Output Cycle wheel (the palette actually used, preset or edited):
                # 64 × (u32 BE position / 65536, A R G B) + u32 BE stop count.
                tdgp = getattr(prop, "_tdgp", None)
                blobs = [chunk.data for group in (tdgp.chunks if tdgp is not None else ())
                         if getattr(group, "list_type", None) == "aRbs"
                         for chunk in group.chunks if chunk.chunk_type == "aRbp"]
                if len(blobs) == 1 and len(blobs[0]) == 520:
                    node["colorama_cycle_hex"] = blobs[0].hex()
                else:
                    self.loss(here, "colorama_cycle", "expected one 520-byte Colorama Output Cycle aRbp")
            if node["match_name"] in ("MB LookSuite3", "S_Monochrome", "S_CartoonPaint"):
                tdgp = getattr(prop, "_tdgp", None)
                groups = [chunk for chunk in (tdgp.chunks if tdgp is not None else ())
                          if getattr(chunk, "list_type", None) == "aRbs"]
                blobs = [chunk.data for group in groups for chunk in group.chunks
                         if chunk.chunk_type == "aRbp" and (node["match_name"] != "MB LookSuite3" or chunk.data.startswith(b"MBL2"))]
                if len(blobs) == 1 and len(blobs[0]) <= 1024 * 1024:
                    field = "mbl2_state_hex" if node["match_name"] == "MB LookSuite3" else "sapphire_mocha_state_hex"
                    node[field] = blobs[0].hex()
                elif blobs:
                    self.loss(here, "custom_effect_state", "ambiguous or oversized effect state")
            if self.read(prop, "is_mask", here):
                node["metadata"] = {name: self.value(self.read(prop, name, here), f"{here}/{name}")
                                    for name in ("mask_mode", "inverted", "color", "locked",
                                                 "mask_feather_falloff", "mask_motion_blur", "roto_bezier")}
            children = self.read(prop, "properties", here, []) or []
            node["children"] = [self.property(child, path + [i], here)
                                for i, child in enumerate(children, 1)]
            tdgp = getattr(prop, "_tdgp", None)
            if node["match_name"] == "ADBE FreePin3 ARAP Group" and tdgp is not None:
                outline = next((c.data for c in tdgp.chunks if c.chunk_type == "fpol"), None)
                self.puppet_state(node, "fpol_b64", outline, here)
            if node["match_name"] == "ADBE FreePin3 Mesh Atom" and tdgp is not None:
                state = next((c.data for c in tdgp.chunks if c.chunk_type == "fpms"), None)
                self.puppet_state(node, "fpms_b64", state, here)
            if node["match_name"] == "ADBE FreePin3 Mesh Group" and tdgp is not None:
                for child, child_node in zip(children, node["children"]):
                    if child_node["match_name"] != "ADBE FreePin3 Mesh Atom":
                        continue
                    position = next((i for i, chunk in enumerate(tdgp.chunks)
                                     if chunk is getattr(child, "_tdgp", None)), None)
                    if position is None:
                        continue
                    following = []
                    for chunk in tdgp.chunks[position + 1:]:
                        if isinstance(chunk, ListChunk) and chunk.list_type == "tdgp":
                            break
                        following.append(chunk)
                    state = next((c.data for c in following if c.chunk_type == "PEdt"), None)
                    if state is not None:
                        self.puppet_state(child_node, "pedt_b64", state, here)
                        geometry = self.puppet_mesh(state)
                        if geometry is not None:
                            child_node["mesh_geometry"] = geometry
                        elif len(state) != 30:
                            self.loss(here, "puppet_mesh", "unsupported or malformed PEdt topology")
        else:
            raw_value = self.read(prop, "value", here)
            default_gradient = False
            default = self.effect_default(prop, self.effect_definitions, self.authored_values)
            if default is not None:
                raw_value = default
            if raw_value is None and prop.match_name in ("gradientFill/gradient", "outerGlow/gradient", "innerGlow/gradient"):
                style = prop.parent_property
                if style is not None and style.enabled:
                    if prop.match_name == "gradientFill/gradient":
                        default_gradient = True
                    else:
                        choice = next((child for child in style.properties
                                       if child.match_name.endswith("/AEColorChoice")), None)
                        default_gradient = choice is not None and choice.value == 2
                if default_gradient:
                    # py-aep's GradientKind materializes this AE default for an unedited leaf.
                    raw_value = Gradient()
            node.update(value_type=self.value(self.read(prop, "property_value_type", here), here),
                        value=self.value(raw_value, here + "/value"),
                        expression=self.read(prop, "expression", here),
                        expression_enabled=self.read(prop, "expression_enabled", here),
                        dimensions_separated=self.read(prop, "dimensions_separated", here),
                        keyframes=[])
            if default_gradient:
                node["value_origin"] = "ae_default"
            for i, key in enumerate(self.read(prop, "keyframes", here, []) or [], 1):
                node["keyframes"].append(self.keyframe(key, f"{here}/keyframe/{i}"))
        return node

    @staticmethod
    def names_matte_layers(version):
        """Whether a project saved by After Effects `version` ("25.5x4") names each track matte
        layer (AE 23 and later); older projects use the layer above."""
        try:
            return int(str(version).split(".", 1)[0]) >= 23
        except ValueError:
            return False

    @staticmethod
    def track_matte(layer, matte, explicit):
        """The track matte layer of a layer whose Track Matte type is set. py-aep falls back to
        the layer above when no matte layer is named; AE 23+ applies no matte then (a type kept
        with the layer set to None), so a project that names matte layers gets none."""
        if matte is not None and explicit and getattr(layer, "_matte_layer_id", None) == 0:
            return None
        return matte

    @staticmethod
    def av_anchor_depth(properties, height):
        """py-aep reads an AV layer's Anchor Point z in units of the source's height, x and y in
        pixels. Measured with aerender: a stored z of 0.5 on a 1024x1536 footage layer at 275.6%
        moves it 2117 px (0.5 x 1536 x 2.756) along z; a precomp of height 1920 stores 0.0479166
        for 92 px. Shape and text layers store pixels; cameras and lights (Point of Interest) too."""
        if not (isinstance(height, (int, float)) and math.isfinite(height) and 0 < height <= 32768):
            return
        transform = next((p for p in properties if p["match_name"] == "ADBE Transform Group"), None)
        anchor = next((p for p in transform["children"] if p["match_name"] == "ADBE Anchor Point"), None) if transform else None
        if not anchor:
            return
        corrected = False
        for value in [anchor.get("value"), *(key.get("value") for key in anchor.get("keyframes", []))]:
            if isinstance(value, list) and len(value) == 3 and isinstance(value[2], (int, float)) and math.isfinite(value[2]) and value[2] != 0:
                value[2] *= height
                corrected = True
        for key in anchor.get("keyframes", []):
            for name in ("in_spatial_tangent", "out_spatial_tangent"):
                tangent = key.get(name)
                if isinstance(tangent, list) and len(tangent) == 3 and isinstance(tangent[2], (int, float)) and math.isfinite(tangent[2]):
                    tangent[2] *= height
        if corrected:
            anchor["value_origin"] = "ae_av_anchor_z_height"

    def layer(self, layer, index, comp_id):
        scope = f"item/{comp_id}/layer/{layer.id}"
        source = self.read(layer, "source", scope) if hasattr(type(layer), "source") else None
        parent = self.read(layer, "parent", scope)
        matte = (self.track_matte(layer, self.read(layer, "track_matte_layer", scope), self.explicit_mattes)
                 if hasattr(type(layer), "has_track_matte") and self.read(layer, "has_track_matte", scope) else None)
        timing = {name: self.read(layer, name, scope) for name in
                  ("start_time", "in_point", "out_point", "stretch")}
        switches = {name: self.value(self.read(layer, name, scope), scope + "/" + name)
                    for name in ("enabled", "locked", "shy", "solo", "null_layer", "is_3d", "auto_orient",
                                 "adjustment_layer", "audio_enabled", "collapse_transformation",
                                 "effects_active", "environment_layer", "frame_blending_type",
                                 "guide_layer", "motion_blur", "preserve_transparency",
                                 "quality", "sampling_quality", "three_d_layer", "three_d_per_char",
                                 "blending_mode", "time_remap_enabled") if hasattr(type(layer), name)}
        props = self.read(layer, "properties", scope, []) or []
        properties = [self.property(prop, [i], scope) for i, prop in enumerate(props, 1)]
        if type(layer).__name__ == "ThreeDModelLayer":
            transform = next((p for p in properties if p["match_name"] == "ADBE Transform Group"), None)
            anchor = next((p for p in transform["children"] if p["match_name"] == "ADBE Anchor Point"), None) if transform else None
            if anchor and not anchor["authored"]:
                if anchor["keyframes"] or anchor["expression"]:
                    self.loss(scope + "/model_anchor", "inconsistent_default", "un-authored model anchor has animation or expression")
                else:
                    anchor["value"] = [0.0, 0.0, 0.0]
                    anchor["value_origin"] = "ae_model_anchor_default"
            elif anchor and anchor["authored"]:
                width = self.read(source, "width", scope) if source else None
                height = self.read(source, "height", scope) if source else None
                if all(isinstance(n, (int, float)) and math.isfinite(n) and 0 < n <= 32768
                       for n in (width, height)):
                    corrected = False
                    for value in [anchor.get("value"), *(key.get("value") for key in anchor["keyframes"])]:
                        if value is None:
                            continue
                        if isinstance(value, list) and len(value) == 3 and all(isinstance(n, (int, float)) and math.isfinite(n) for n in value):
                            value[0] /= width
                            value[1] /= height
                            corrected = True
                        else:
                            self.loss(scope + "/model_anchor", "unsupported_value", value)
                    if corrected:
                        anchor["value_origin"] = "ae_model_anchor_xy"
                else:
                    self.loss(scope + "/model_anchor", "missing_source_dimensions", source.id if source else None)
        elif type(layer).__name__ == "AVLayer" and source is not None:
            self.av_anchor_depth(properties, self.read(source, "height", scope))
        return {"id": layer.id, "index": index, "kind": type(layer).__name__,
                "name": self.read(layer, "name", scope), "match_name": self.read(layer, "match_name", scope),
                "light_type": self.value(self.read(layer, "light_type", scope), scope + "/light_type")
                if hasattr(type(layer), "light_type") else None,
                "source_id": source.id if source else None,
                "parent_id": parent.id if parent else None,
                "matte": {"layer_id": matte.id,
                           "type": self.value(self.read(layer, "track_matte_type", scope), scope)} if matte else None,
                "timing": timing, "switches": switches,
                "properties": properties}

    def item(self, item):
        scope = f"item/{item.id}"
        parent = self.read(item, "parent_folder", scope)
        kind = "comp" if hasattr(item, "layers") else "footage" if hasattr(item, "main_source") else "folder" if hasattr(item, "num_items") else "unknown"
        result = {"id": item.id, "kind": kind, "name": self.read(item, "name", scope),
                  "parent_id": parent.id if parent and parent is not item else None,
                  "comment": self.read(item, "comment", scope), "label": self.value(self.read(item, "label", scope), scope)}
        if kind == "comp":
            for name in ("width", "height", "pixel_aspect", "duration", "frame_rate",
                         "display_start_time", "work_area_start", "work_area_duration", "bg_color", "renderer"):
                result[name] = self.value(self.read(item, name, scope), scope + "/" + name)
            result["layers"] = [self.layer(layer, i, item.id)
                                for i, layer in enumerate(self.read(item, "layers", scope, []) or [], 1)]
            default_chunk = next((chunk for chunk in item._item_list.chunks
                                  if isinstance(chunk, ListChunk) and chunk.list_type == "DLay"), None)
            result["default_camera"] = None
            if default_chunk is not None:
                try:
                    camera = parse_layer(default_chunk, item, {})
                    hidden = self.layer(camera, 0, item.id)
                    hidden["implicit"] = True
                    def camera_value(group_name, property_name):
                        group = next((p for p in camera.properties if p.match_name == group_name), None)
                        prop = next((p for p in group.properties if p.match_name == property_name), None) if group else None
                        return self.value(prop.value, f"{scope}/default_camera/{property_name}") if prop else None
                    hidden["pose"] = {
                        "position": camera_value("ADBE Transform Group", "ADBE Position"),
                        "point_of_interest": camera_value("ADBE Transform Group", "ADBE Anchor Point"),
                        "zoom": camera_value("ADBE Camera Options Group", "ADBE Camera Zoom"),
                    }
                    position, poi, zoom = (hidden["pose"][k] for k in ("position", "point_of_interest", "zoom"))
                    transform = next((p for p in hidden["properties"] if p["match_name"] == "ADBE Transform Group"), None)
                    position_prop = next((p for p in transform["children"] if p["match_name"] == "ADBE Position"), None) if transform else None
                    if (position_prop and not position_prop["authored"] and not position_prop["keyframes"] and not position_prop["expression"]
                            and isinstance(position, list) and isinstance(poi, list)
                            and len(position) == len(poi) == 3 and position == poi
                            and isinstance(zoom, (int, float)) and math.isfinite(zoom) and zoom > 0):
                        position[2] = poi[2] - zoom
                        position_prop["value"][2] = position[2]
                        position_prop["value_origin"] = "ae_default_camera_eye"
                    result["default_camera"] = hidden
                except Exception as exc:
                    self.loss(scope + "/default_camera", "parse_error", f"{type(exc).__name__}: {exc}")
        elif kind == "footage":
            for name in ("width", "height", "duration", "frame_rate", "pixel_aspect",
                         "file", "footage_missing", "has_audio", "has_video", "asset_type"):
                result[name] = self.value(self.read(item, name, scope), scope + "/" + name)
            source = self.read(item, "main_source", scope)
            result["source_type"] = type(source).__name__ if source else None
            # 3D model footage: the object scale of AE's model settings ("mdls", big-endian
            # f64 first), applied on top of the layer's Scale around its anchor.
            mdls = next((c for c in getattr(getattr(item, "_item_list", None), "chunks", ())
                         if c.chunk_type == "mdls"), None)
            if mdls is not None:
                data = getattr(mdls, "data", b"") or b""
                scale = struct.unpack(">d", data[:8])[0] if len(data) >= 8 else None
                if scale is not None and math.isfinite(scale) and 0 < scale <= 1e6:
                    result["model_scale"] = scale
                else:
                    self.loss(scope + "/model_scale", "unsupported_value", data[:8].hex())
            if source and type(source).__name__ == "SolidSource":
                result["color"] = self.value(self.read(source, "color", scope), scope + "/color")
            elif source and type(source).__name__ == "FileSource":
                try:
                    binding = (self.vector_source_layer(source)
                               or self.psd_source_layer(source, result.get("width"), result.get("height")))
                    if binding is not None:
                        result["source_layer"] = binding
                except (ValueError, TypeError, struct.error) as exc:
                    self.loss(scope + "/source_layer", "source_layer", str(exc))
                raw = result["file"]
                result["resolved_path"] = None
                result["media_status"] = "missing"
                if raw:
                    exact = Path(raw)
                    if is_file(exact):
                        matches = [exact.resolve()]
                        result["media_status"] = "exact"
                    else:
                        if self.media_index is None:
                            self.media_index = {}
                            for path in self.source_dir.rglob("*"):
                                if is_file(path):
                                    self.media_index.setdefault(media_name(path.name), []).append(path)
                        basename = PureWindowsPath(raw).name
                        matches = self.media_index.get(media_name(basename), [])
                        result["media_status"] = "unique_basename" if len(matches) == 1 else "ambiguous" if matches else "missing"
                    if len(matches) == 1:
                        resolved = matches[0]
                        result["resolved_path"] = str(resolved)
                        result["file_size"] = resolved.stat().st_size
                    else:
                        self.media_issues.append({"item_id": item.id, "raw_path": raw,
                                                  "status": result["media_status"]})
                else:
                    self.media_issues.append({"item_id": item.id, "raw_path": None,
                                              "status": "missing"})
        elif kind == "unknown":
            self.loss(scope, "unknown_item", type(item).__name__)
            result["class"] = type(item).__name__
        return result

    def export(self, source):
        source = Path(source).resolve()
        self.source_dir = source.parent
        app = py_aep.parse(source)
        self.explicit_mattes = self.names_matte_layers(self.read(app, "version", "application"))
        project = app.project
        self.effect_definitions = getattr(project, "_effect_param_defs", None) or {}
        self.authored_values = self.authored_effect_values(project)
        items = [self.item(item) for item in project]
        with source.open("rb") as handle:
            digest = hashlib.file_digest(handle, "sha256").hexdigest()
        return {"schema_version": SCHEMA_VERSION,
                "source": {"path": str(source), "sha256": digest},
                "project": {"name": source.stem,
                            "bits_per_channel": self.value(self.read(project, "bits_per_channel", "project"), "project"),
                            "expression_engine": self.read(project, "expression_engine", "project")},
                "items": items, "losses": self.losses, "media_issues": self.media_issues,
                "counts": {"items": len(items), "comps": sum(i["kind"] == "comp" for i in items),
                           "layers": sum(len(i.get("layers", [])) for i in items),
                           "losses": len(self.losses), "media_issues": len(self.media_issues)}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--font-search-root", type=Path,
                        help="Optional bounded fallback for exact PostScript font matches")
    destination = parser.add_mutually_exclusive_group(required=True)
    destination.add_argument("-o", "--output", type=Path)
    destination.add_argument("--stdout", action="store_true")
    args = parser.parse_args()
    data = Exporter(font_search_root=args.font_search_root).export(args.source)
    encoded = windows_bytes_to_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")))
    if args.stdout:
        sys.stdout.write(encoded + "\n")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded)
        print(json.dumps({"output": str(args.output), **data["counts"]}))


if __name__ == "__main__":
    main()
