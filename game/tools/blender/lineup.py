"""Render all classes side by side (idle) and, per class, a strip of its
special move, for review.

blender -b -P tools/blender/lineup.py -- <outdir> [old]

With "old", renders the original KayKit models instead (no gear).
Loadouts mirror loadout() in crates/app/src/render.rs.
"""
import math
import os
import sys

import bpy
from mathutils import Vector

args = sys.argv[sys.argv.index("--") + 1:]
outdir = args[0]
old = "old" in args
PITCH = next((float(a[6:]) for a in args if a.startswith("pitch=")), 20.0)
os.makedirs(outdir, exist_ok=True)

CLASSES = [
    ("Fighter", "Barbarian", ["2H_Axe", "Barbarian_Hat", "Barbarian_Cape", "Fighter_Horns", "Fighter_Pauldron_L", "Fighter_Pauldron_R", "Fighter_Belt"], "2H_Melee_Idle"),
    ("Hunter", "Barbarian", ["1H_Axe", "1H_Axe_Offhand", "Barbarian_Cape", "Hunter_Pelt", "Hunter_Necklace"], "Idle"),
    ("Guard", "Knight", ["1H_Sword", "Badge_Shield", "Knight_Helmet", "Knight_Cape", "Guard_Plume", "Guard_Pauldron_L", "Guard_Pauldron_R"], "Idle"),
    ("Paladin", "Knight", ["2H_Sword", "Knight_Helmet", "Knight_Cape", "Paladin_Wings", "Paladin_Pauldron_L", "Paladin_Pauldron_R"], "2H_Melee_Idle"),
    ("Healer", "Mage", ["2H_Staff", "Mage_Hat", "Mage_Cape", "Healer_Charm"], "Idle"),
    ("Keeper", "Mage", ["Spellbook_open", "1H_Wand", "Mage_Cape", "Keeper_Lantern", "Keeper_Collar"], "Idle"),
    ("Mage", "Mage", ["1H_Wand", "Spellbook", "Mage_Hat", "Mage_Orbs"], "Idle"),
    ("Warlock", "Mage", ["2H_Staff", "Mage_Cape", "Warlock_Horns", "Warlock_Collar"], "Idle"),
    ("Sentry", "Rogue", ["1H_Crossbow", "Knife_Offhand", "Rogue_Cape", "Sentry_Goggles", "Sentry_Pauldron_L", "Sentry_Pauldron_R"], "Idle"),
    ("Archer", "Rogue_Hooded", ["2H_Crossbow", "Rogue_Cape", "Archer_Quiver", "Archer_Feather"], "Idle"),
    ("Assassin", "Rogue_Hooded", ["Knife", "Knife_Offhand", "Assassin_Mask", "Assassin_Sash"], "Idle"),
]
COLOURS = {  # class colours for the cape tint, roughly as in the game
    "Fighter": (0.8, 0.25, 0.2), "Hunter": (0.55, 0.4, 0.2), "Guard": (0.25, 0.45, 0.8), "Paladin": (0.9, 0.8, 0.35),
    "Healer": (0.35, 0.8, 0.45), "Keeper": (0.45, 0.35, 0.7), "Mage": (0.3, 0.5, 0.95), "Warlock": (0.5, 0.2, 0.6),
    "Sentry": (0.4, 0.6, 0.5), "Archer": (0.3, 0.65, 0.3), "Assassin": (0.25, 0.25, 0.3),
}

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.render.fps = 24
here = os.path.dirname(os.path.abspath(__file__))
assets = os.path.join(here, "..", "..", "assets")


def place(cls, model, show, x):
    before = set(bpy.data.objects)
    acts_before = set(bpy.data.actions)
    folder = "adventurers" if old else "troops"
    bpy.ops.import_scene.gltf(filepath=os.path.join(assets, folder, model + ".glb"), bone_heuristic="BLENDER")
    objs = [o for o in bpy.data.objects if o not in before]
    acts = {a.name.split(".")[0] if a.name[-4:-3] == "." else a.name: a for a in bpy.data.actions if a not in acts_before}
    rig = next(o for o in objs if o.type == "ARMATURE")
    rig.location.x = x
    for o in objs:
        if o.type == "MESH" and not o.vertex_groups:
            base = o.name.split(".")[0] if o.name[-4:-3] == "." else o.name
            o.hide_render = base not in show
    return rig, acts


world = bpy.data.worlds.new("w")
world.use_nodes = True
world.node_tree.nodes["Background"].inputs[0].default_value = (0.55, 0.62, 0.7, 1)
world.node_tree.nodes["Background"].inputs[1].default_value = 0.9
scene.world = world
sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", "SUN"))
sun.data.energy = 3.2
sun.rotation_euler = (math.radians(50), math.radians(10), math.radians(-35))
scene.collection.objects.link(sun)
bpy.ops.mesh.primitive_plane_add(size=60)
gm = bpy.data.materials.new("ground")
gm.use_nodes = True
gm.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (0.3, 0.45, 0.22, 1)
bpy.context.object.data.materials.append(gm)
cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
scene.collection.objects.link(cam)
scene.camera = cam
cam.data.type = "ORTHO"
scene.render.engine = "BLENDER_EEVEE"
scene.view_settings.view_transform = "Standard"


def aim(at, scale, dist=30.0, yaw=-20, pitch=20):
    yaw, pitch = math.radians(yaw), math.radians(pitch)
    d = Vector((math.sin(yaw) * math.cos(pitch), -math.cos(yaw) * math.cos(pitch), math.sin(pitch)))
    cam.location = Vector(at) + d * dist
    cam.rotation_euler = (-d).to_track_quat("-Z", "Y").to_euler()
    cam.data.ortho_scale = scale


rigs = []
gap = 2.3
for i, (cls, model, show, idle) in enumerate(CLASSES):
    x = (i - (len(CLASSES) - 1) / 2) * gap
    rig, acts = place(cls, model, show, x)
    rig.animation_data.action = acts[idle]
    rigs.append((cls, rig, acts))

scene.frame_set(0)
aim((0, 0, 1.0), scale=len(CLASSES) * gap + 0.6, pitch=PITCH)
scene.render.resolution_x, scene.render.resolution_y = 2400, 560
scene.render.filepath = os.path.join(outdir, "lineup_old.png" if old else "lineup.png")
bpy.ops.render.render(write_still=True)

if not old:
    # Special-move strips: 6 frames per class, one image per class.
    scene.render.resolution_x, scene.render.resolution_y = 300, 340
    base_hidden = {o.name: o.hide_render for o in bpy.data.objects}
    for cls, rig, acts in rigs:
        for c2, r2, _ in rigs:
            vis = c2 == cls
            r2.hide_render = not vis
            for o in r2.children:
                o.hide_render = (not vis) or base_hidden[o.name]
        rig.animation_data.action = acts["Special_" + cls]
        for k in (0, 5, 9, 13, 18, 25):
            scene.frame_set(k)
            aim((rig.location.x, -0.4, 1.3), scale=4.6, yaw=-55, pitch=14)
            scene.render.filepath = os.path.join(outdir, f"sp_{cls}_{k:02d}.png")
            bpy.ops.render.render(write_still=True)
