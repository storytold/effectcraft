# EffectCraft camera tracker → Blender, original code under MIT OR Apache-2.0.
# Run in Blender's Scripting workspace (Open, Run Script), or:
# blender --background --python "Tracked Scene.py"
# Creates a NEW scene; existing scenes and objects are preserved. Save the .blend yourself.
# Public API types: bpy.types.Camera, bpy.types.ImageUser
import json
import math
import os

import bpy
from mathutils import Matrix

DATA = json.loads(__EFFECTCRAFT_DATA__)


def _curves(owner):
    """Blender 4.0–4.3 legacy actions and 4.4+/5.x layered actions."""
    anim = owner.animation_data
    if anim is None or anim.action is None:
        return
    action = anim.action
    if getattr(action, "is_action_layered", False):
        for layer in action.layers:
            for strip in layer.strips:
                if strip.type == 'KEYFRAME':
                    bag = strip.channelbag(anim.action_slot)
                    if bag is not None:
                        yield from bag.fcurves
    else:
        yield from action.fcurves


def _interpolation(owner):
    for curve in _curves(owner):
        # Footage indices and discrete switches must never interpolate between frames.
        stepped = ("frame_offset" in curve.data_path or "use_dof" in curve.data_path
                   or "aperture_blades" in curve.data_path)
        for key in curve.keyframe_points:
            key.interpolation = 'CONSTANT' if stepped else 'LINEAR'


def _media_path():
    original = DATA["media"]["path"]
    candidates = [original]
    # When sharing, place the video alongside the script or the .blend and run again.
    script_path = globals().get("__file__", "")
    if script_path:
        candidates.append(os.path.join(os.path.dirname(os.path.abspath(script_path)),
                                       os.path.basename(original)))
    if bpy.data.filepath:
        candidates.append(os.path.join(os.path.dirname(bpy.data.filepath),
                                       os.path.basename(original)))
    return next((p for p in candidates if os.path.isfile(p)), None)


def import_scene():
    if bpy.app.version < (4, 0, 0):
        raise RuntimeError("EffectCraft scene import requires Blender 4.0 or newer")
    scene = bpy.data.scenes.new(DATA["name"])
    scene.frame_start = DATA["startFrame"]
    scene.frame_end = DATA["endFrame"]
    fps = DATA["fpsNumerator"] / DATA["fpsDenominator"]
    scene.render.fps = max(1, round(fps))
    scene.render.fps_base = scene.render.fps / fps
    scene.render.resolution_x, scene.render.resolution_y = map(int, DATA["resolution"])
    scene.render.resolution_percentage = 100
    # The tracker assumes square pixels, regardless of the source's interpretation metadata.
    scene.render.pixel_aspect_x = scene.render.pixel_aspect_y = 1.0
    scene.render.film_transparent = True
    scene.render.motion_blur_shutter = max(0.0, min(2.0, DATA["shutter"]))
    scene.unit_settings.system = 'METRIC'
    scene.unit_settings.scale_length = 1.0
    scene["effectcraft_unit_scale"] = DATA["unitScale"]
    scene["effectcraft_scale_is_arbitrary"] = True
    scene["effectcraft_shutter_phase_degrees"] = DATA["shutterPhase"]

    collection = bpy.data.collections.new("EffectCraft Tracking")
    scene.collection.children.link(collection)
    camera_data = bpy.data.cameras.new("3D Tracker Camera")
    camera = bpy.data.objects.new("3D Tracker Camera", camera_data)
    collection.objects.link(camera)
    scene.camera = camera
    camera.rotation_mode = 'QUATERNION'
    camera_data.type = 'PERSP'
    camera_data.sensor_fit = 'HORIZONTAL'
    camera_data.sensor_width = DATA["sensorWidth"]
    camera_data.sensor_height = (DATA["sensorWidth"] * DATA["resolution"][1]
                                 / DATA["resolution"][0])
    camera_data.show_background_images = True
    camera_data.show_limits = True
    extent = max([DATA["groundSize"], 1.0]
                 + [math.dist(p["location"], f["location"])
                    for p in DATA["points"][::max(1, len(DATA["points"]) // 100)]
                    for f in DATA["frames"][::max(1, len(DATA["frames"]) // 10)]])
    camera_data.clip_start = max(0.00001, min(0.1, extent / 10000.0))
    camera_data.clip_end = max(1000.0, extent * 10.0)

    previous = None
    for sample in DATA["frames"]:
        frame = sample["frame"]
        camera.location = sample["location"]
        # Exported right/up/back vectors are COLUMNS of Blender's local → world basis.
        quaternion = Matrix(sample["rotation"]).transposed().to_quaternion()
        if previous is not None and previous.dot(quaternion) < 0:
            quaternion.negate()
        camera.rotation_quaternion = quaternion
        previous = quaternion.copy()
        camera.keyframe_insert(data_path="location", frame=frame)
        camera.keyframe_insert(data_path="rotation_quaternion", frame=frame)
        camera_data.lens = sample["lens"]
        camera_data.keyframe_insert(data_path="lens", frame=frame)
        camera_data.dof.use_dof = sample["dof"]
        camera_data.dof.focus_distance = sample["focus"]
        camera_data.dof.aperture_fstop = sample["fstop"]
        camera_data.dof.aperture_blades = sample["blades"]
        camera_data.dof.aperture_rotation = (sample["irisRotation"] + math.pi) % math.tau - math.pi
        camera_data.dof.aperture_ratio = sample["irisRatio"]
        for prop in ("use_dof", "focus_distance", "aperture_fstop", "aperture_blades",
                     "aperture_rotation", "aperture_ratio"):
            camera_data.keyframe_insert(data_path="dof." + prop, frame=frame)

    path = _media_path()
    if path is not None:
        try:
            image = bpy.data.images.load(path, check_existing=False)
            image.source = DATA["media"]["kind"]
            background = camera_data.background_images.new()
            background.source = 'IMAGE'
            background.image = image
            background.alpha = 1.0
            background.display_depth = 'BACK'
            background.frame_method = 'STRETCH'
            user = background.image_user
            user.use_auto_refresh = True
            user.use_cyclic = False
            user.frame_start = DATA["startFrame"]
            user.frame_duration = max(1, min(1048574, image.frame_duration))
            if image.source == 'MOVIE':
                for sample in DATA["frames"]:
                    # Blender uses 1-based image frames; sourceFrame is 0-based.
                    user.frame_offset = sample["sourceFrame"] - (sample["frame"] - user.frame_start)
                    camera_data.keyframe_insert(
                        data_path="background_images[0].image_user.frame_offset",
                        frame=sample["frame"])
        except (RuntimeError, ValueError, OSError) as error:
            DATA["warnings"].append("Blender could not load the background video: " + str(error))
    else:
        DATA["warnings"].append("Video not found: " + DATA["media"]["path"]
                                + ". Place it beside this script or relink the camera background.")

    if DATA["points"]:
        mesh = bpy.data.meshes.new("Solved Track Points")
        mesh.from_pydata([p["location"] for p in DATA["points"]], [], [])
        mesh.update()
        cloud = bpy.data.objects.new("Solved Track Points", mesh)
        collection.objects.link(cloud)
        cloud.show_in_front = True
        cloud.hide_render = True
        for name, kind, key in (("track_id", 'INT', "id"), ("reprojection_error", 'FLOAT', "error"),
                                ("first_frame", 'INT', "first"), ("last_frame", 'INT', "last")):
            attr = mesh.attributes.new(name=name, type=kind, domain='POINT')
            for dest, point in zip(attr.data, DATA["points"]):
                # Track IDs are u32 in EffectCraft; keep the full IDs in the JSON text too.
                dest.value = point[key] if kind == 'FLOAT' else min(2147483647, point[key])

    if DATA["ground"]:
        size = DATA["groundSize"]
        mesh = bpy.data.meshes.new("Solved Ground Plane")
        mesh.from_pydata([(-size, -size, 0), (size, -size, 0),
                          (size, size, 0), (-size, size, 0)], [], [(0, 1, 2, 3)])
        mesh.update()
        ground = bpy.data.objects.new("Solved Ground Plane", mesh)
        collection.objects.link(ground)
        ground.display_type = 'WIRE'
        ground.hide_render = True
    for marker in DATA["markers"]:
        scene.timeline_markers.new(marker["name"], frame=marker["frame"])

    _interpolation(camera)
    _interpolation(camera_data)
    metadata = bpy.data.texts.new("EffectCraft Tracking Metadata.json")
    metadata.write(json.dumps(DATA, ensure_ascii=False, indent=2))
    scene["effectcraft_metadata_text"] = metadata.name
    camera_data["effectcraft_average_error_pixels"] = DATA["solve"]["averageError"]
    camera_data["effectcraft_solve_method"] = DATA["solve"]["methodUsed"]
    camera_data["effectcraft_focus_source"] = "See warnings in " + metadata.name
    distortion = DATA["solve"].get("distortion")
    if distortion:
        for key in ("k1", "k2", "radius"):
            camera_data["effectcraft_distortion_" + key] = distortion[key]
    if bpy.context.window is not None:
        bpy.context.window.scene = scene
        for area in bpy.context.window.screen.areas:
            if area.type == 'VIEW_3D':
                area.spaces.active.region_3d.view_perspective = 'CAMERA'
    scene.frame_set(scene.frame_start)
    for warning in DATA["warnings"]:
        print("EffectCraft: " + warning)
    print("EffectCraft: imported", len(DATA["frames"]), "baked camera frames into", scene.name)
    return scene


if __name__ == "__main__":
    EFFECTCRAFT_SCENE = import_scene()
