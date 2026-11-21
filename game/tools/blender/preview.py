"""Render a before/after still and a special-move clip for review.

blender -b -P tools/blender/preview.py -- <old.glb> <new.glb> <show,comma,list> <idle> <special> <outdir>

Meshes named in the show list (and every skinned body part) are shown;
other accessories stay hidden, as in the game.
"""
import math
import os
import sys

import bpy
from mathutils import Vector

old, new, show, idle, special, outdir = sys.argv[sys.argv.index("--") + 1:]
show = set(show.split(","))
os.makedirs(outdir, exist_ok=True)

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.render.fps = 24


def place(path, x):
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=path, bone_heuristic="BLENDER")
    objs = [o for o in bpy.data.objects if o not in before]
    rig = next(o for o in objs if o.type == "ARMATURE")
    rig.location.x = x
    for o in objs:
        if o.type == "MESH" and not o.vertex_groups:
            base = o.name.split(".")[0] if o.name[-4:-3] == "." else o.name
            o.hide_render = base not in show
    return rig


def action(name_prefix):
    # Second import renames actions with a suffix; take the newest match.
    acts = [a for a in bpy.data.actions if a.name == name_prefix or a.name.startswith(name_prefix + ".")]
    return sorted(acts, key=lambda a: a.name)[-1] if acts else None


rig_old = place(old, -1.1)
act_idle_old = action(idle)
rig_new = place(new, 1.1)
act_idle_new = action(idle)
act_special = action(special)

world = bpy.data.worlds.new("w")
world.use_nodes = True
world.node_tree.nodes["Background"].inputs[0].default_value = (0.55, 0.62, 0.7, 1)
world.node_tree.nodes["Background"].inputs[1].default_value = 0.9
scene.world = world
sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", "SUN"))
sun.data.energy = 3.2
sun.rotation_euler = (math.radians(50), math.radians(10), math.radians(-35))
scene.collection.objects.link(sun)
bpy.ops.mesh.primitive_plane_add(size=12)
ground = bpy.context.object
gm = bpy.data.materials.new("ground")
gm.diffuse_color = (0.35, 0.5, 0.28, 1)
gm.use_nodes = True
gm.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (0.3, 0.45, 0.22, 1)
ground.data.materials.append(gm)

cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
scene.collection.objects.link(cam)
scene.camera = cam
cam.data.type = "ORTHO"


def aim(at, dist=9.0, yaw=-25, pitch=22, scale=4.6):
    yaw, pitch = math.radians(yaw), math.radians(pitch)
    d = Vector((math.sin(yaw) * math.cos(pitch), -math.cos(yaw) * math.cos(pitch), math.sin(pitch)))
    cam.location = Vector(at) + d * dist
    cam.rotation_euler = (-d).to_track_quat("-Z", "Y").to_euler()
    cam.data.ortho_scale = scale


scene.render.engine = "BLENDER_EEVEE"
scene.render.resolution_x, scene.render.resolution_y = 1200, 800
scene.render.film_transparent = False
scene.view_settings.view_transform = "Standard"

# Still: before (left) and after (right), idle pose.
rig_old.animation_data.action = act_idle_old
rig_new.animation_data.action = act_idle_new
scene.frame_set(0)
aim((0, 0, 1.15), scale=5.8)
scene.render.image_settings.file_format = "PNG"
scene.render.filepath = os.path.join(outdir, "before_after.png")
bpy.ops.render.render(write_still=True)

# Special move: the new model alone, played as an MP4 (slowed to 0.5x after
# a real-speed pass so the motion reads).
rig_old.hide_render = True
for o in bpy.data.objects:
    if o.parent == rig_old:
        o.hide_render = True
rig_new.animation_data.action = act_special
aim((1.1, -0.4, 1.35), yaw=-60, pitch=14, scale=5.4)
scene.render.resolution_x, scene.render.resolution_y = 800, 800
n = int(act_special.frame_range[1])
# Real speed, then half speed via frame repeats in a second render.
scene.frame_start, scene.frame_end = 0, n
scene.render.image_settings.media_type = "VIDEO"
scene.render.image_settings.file_format = "FFMPEG"
scene.render.ffmpeg.format = "MPEG4"
scene.render.ffmpeg.codec = "H264"
scene.render.ffmpeg.constant_rate_factor = "HIGH"
scene.render.frame_map_old, scene.render.frame_map_new = 100, 300
scene.frame_end = n * 3
scene.render.filepath = os.path.join(outdir, special + "_slowmo.mp4")
bpy.ops.render.render(animation=True)

# A strip of key frames as one PNG for quick review.
scene.render.frame_map_old, scene.render.frame_map_new = 100, 100
scene.render.image_settings.media_type = "IMAGE"
scene.render.image_settings.file_format = "PNG"
scene.render.resolution_x, scene.render.resolution_y = 400, 400
for k in range(0, n + 1, 3):
    scene.frame_set(k)
    scene.render.filepath = os.path.join(outdir, f"{special}_{k:02d}.png")
    bpy.ops.render.render(write_still=True)
