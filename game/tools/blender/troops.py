"""Rebuild the troop models with a chunkier, more characterful look and a
special-move clip per class, starting from the CC0 KayKit characters.

Run from game/:
  blender -b -P tools/blender/troops.py -- <model> assets/troops/<model>.glb [--blend x.blend]

<model> is a key of MODELS below (Barbarian, Knight, Mage, Rogue,
Rogue_Hooded). The source is always the untouched KayKit file in
assets/adventurers/, so the script can be re-run safely. Nothing here
touches battle rules: only meshes and animation clips change.

What it does to a model:
  * bulks up the build (thicker arms and legs, bigger fists, broader chest)
    by pushing skinned vertices away from their bones, so every existing
    animation still fits;
  * adds class gear as separate rigid meshes on bones (named <Class>_...),
    which the game shows per class through its loadout lists;
  * scales up the model's own weapons a little;
  * adds a "Special_<Class>" clip for each class that uses the model.
"""
import math
import os
import sys

import bmesh
import bpy
import numpy as np
from mathutils import Matrix, Quaternion, Vector

FPS = 24
# The game shows a special move for 1.1 s, so clips land their hit early
# and are finished by then.
SPECIAL_FRAMES = 26

# ----------------------------------------------------------------- set-up


def load(path):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.context.scene.render.fps = FPS
    # The "BLENDER" heuristic keeps each bone's axes equal to its glTF node,
    # so the hand slots that armoury weapons attach to come back unchanged.
    bpy.ops.import_scene.gltf(filepath=path, bone_heuristic="BLENDER")
    rig = next(o for o in bpy.data.objects if o.type == "ARMATURE")
    return rig


def export(path):
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        export_animation_mode="ACTIONS",
        export_force_sampling=True,
        # The game's loader interpolates every key linearly, so keep plain
        # sampled keys rather than the optimiser's STEP channels.
        export_optimize_animation_size=False,
        export_def_bones=False,
        export_yup=True,
        export_apply=False,
        export_texcoords=True,
        export_normals=True,
        export_skins=True,
        export_morph=False,
        export_lights=False,
        export_cameras=False,
        export_extras=False,
        export_image_format="AUTO",
    )


# ---------------------------------------------------------------- palette


class Palette:
    """Picks texels from a model's colour atlas, so new gear is coloured by
    the same texture (the game reads base colour textures, not materials)."""

    def __init__(self, image):
        self.image = image
        w, h = image.size
        px = np.empty(w * h * 4, dtype=np.float32)
        image.pixels.foreach_get(px)
        self.px = px.reshape(h, w, 4)
        self.w, self.h = w, h

    def uv(self, rgb):
        """UV of the texel closest to `rgb` (0-255, sRGB)."""
        target = np.array(rgb, dtype=np.float32) / 255.0
        # Image pixels are linear when the image is not flagged sRGB-only;
        # KayKit atlases are sRGB, Blender stores them as read.
        d = np.sum((self.px[:, :, :3] - target) ** 2, axis=2)
        y, x = np.unravel_index(np.argmin(d), d.shape)
        return Vector(((x + 0.5) / self.w, (y + 0.5) / self.h))


# ----------------------------------------------------------- mesh helpers


def new_object(name, bm, material, uv):
    me = bpy.data.meshes.new(name)
    bm.to_mesh(me)
    bm.free()
    me.materials.append(material)
    layer = me.uv_layers.new(name="UVMap")
    for loop in layer.data:
        loop.uv = uv
    for p in me.polygons:
        p.use_smooth = False
    ob = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(ob)
    return ob


def colored(parts, material, palette, name):
    """Join (bmesh, rgb) parts into one object, each part its own colour."""
    me = bpy.data.meshes.new(name)
    bm = bmesh.new()
    uvs = []
    for part, rgb in parts:
        tmp = bpy.data.meshes.new("tmp")
        part.to_mesh(tmp)
        part.free()
        n0 = len(bm.faces)
        bm.from_mesh(tmp)
        bpy.data.meshes.remove(tmp)
        uvs.append((n0, len(bm.faces), palette.uv(rgb)))
    bm.faces.ensure_lookup_table()
    layer = bm.loops.layers.uv.new("UVMap")
    for a, b, uv in uvs:
        for f in bm.faces[a:b]:
            for loop in f.loops:
                loop[layer].uv = uv
    bm.to_mesh(me)
    bm.free()
    me.materials.append(material)
    ob = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(ob)
    return ob


def transformed(bm, m):
    bmesh.ops.transform(bm, matrix=m, verts=bm.verts)
    return bm


def cone(r1, r2, depth, segs=8, m=Matrix()):
    bm = bmesh.new()
    bmesh.ops.create_cone(bm, cap_ends=True, segments=segs, radius1=r1, radius2=r2, depth=depth)
    return transformed(bm, m)


def sphere(r, m=Matrix(), segs=10, rings=6):
    bm = bmesh.new()
    bmesh.ops.create_uvsphere(bm, u_segments=segs, v_segments=rings, radius=r)
    return transformed(bm, m)


def box(size, m=Matrix()):
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    bmesh.ops.scale(bm, vec=Vector(size), verts=bm.verts)
    return transformed(bm, m)


def torus(r_major, r_minor, m=Matrix(), segs=16, sides=6):
    bm = bmesh.new()
    verts = []
    for i in range(segs):
        a = 2 * math.pi * i / segs
        ring = []
        for j in range(sides):
            b = 2 * math.pi * j / sides
            rr = r_major + r_minor * math.cos(b)
            ring.append(bm.verts.new((rr * math.cos(a), rr * math.sin(a), r_minor * math.sin(b))))
        verts.append(ring)
    for i in range(segs):
        for j in range(sides):
            a, b = verts[i], verts[(i + 1) % segs]
            bm.faces.new((a[j], b[j], b[(j + 1) % sides], a[(j + 1) % sides]))
    return transformed(bm, m)


def horn(length, r, bend, segs=7, steps=6, m=Matrix()):
    """A tapered horn that curls up and forward, base at the origin pointing +X."""
    bm = bmesh.new()
    rings = []
    for k in range(steps + 1):
        t = k / steps
        a = bend * t
        # Centre line: an arc in the XZ plane.
        c = Vector((length * t * math.cos(a * 0.6), 0, length * t * math.sin(a * 0.6) + 0.35 * length * t * t))
        rad = r * (1 - t) + 0.008
        ring = []
        for j in range(segs):
            b = 2 * math.pi * j / segs
            ring.append(bm.verts.new(c + Vector((0, rad * math.cos(b), rad * math.sin(b)))))
        rings.append(ring)
    for k in range(steps):
        for j in range(segs):
            a, b = rings[k], rings[k + 1]
            bm.faces.new((a[j], a[(j + 1) % segs], b[(j + 1) % segs], b[j]))
    bm.faces.new(list(reversed(rings[0])))
    bm.faces.new(rings[-1])
    return transformed(bm, m)


def on_bone(ob, rig, bone, world):
    """Parent `ob` rigidly to `bone`, placed at `world` in the rest pose."""
    rig.data.pose_position = "REST"
    bpy.context.view_layer.update()
    ob.parent = rig
    ob.parent_type = "BONE"
    ob.parent_bone = bone
    bpy.context.view_layer.update()
    ob.matrix_world = world
    bpy.context.view_layer.update()
    rig.data.pose_position = "POSE"


# ------------------------------------------------------------ bulking up


def bone_line(rig, name):
    b = rig.data.bones[name]
    mw = rig.matrix_world
    return mw @ b.head_local, mw @ b.tail_local


def bulk(rig, rules):
    """Push skinned vertices away from bone axes. rules: bone -> (radial, along)
    where `radial` scales distance from the bone's axis and `along` is an
    extra uniform scale about the bone's middle (for fists)."""
    for ob in [o for o in bpy.data.objects if o.type == "MESH" and o.vertex_groups]:
        groups = {g.index: g.name for g in ob.vertex_groups}
        mw, mi = ob.matrix_world, ob.matrix_world.inverted()
        for v in ob.data.vertices:
            p = mw @ v.co
            d = Vector()
            for ge in v.groups:
                rule = rules.get(groups.get(ge.group))
                if not rule or ge.weight <= 0:
                    continue
                radial, along = rule
                h, t = bone_line(rig, groups[ge.group])
                axis = (t - h).normalized()
                foot = h + axis * (p - h).dot(axis)
                d += ge.weight * (radial - 1.0) * (p - foot)
                if along != 1.0:
                    mid = (h + t) / 2
                    d += ge.weight * (along - 1.0) * (p - mid)
            v.co = mi @ (p + d)


# ------------------------------------------------------------- animation


def sample_action(rig, action, frame):
    """matrix_basis of every pose bone with `action` at a (fractional) frame."""
    rig.animation_data.action = action
    f = int(math.floor(frame))
    bpy.context.scene.frame_set(f, subframe=frame - f)
    return {pb.name: pb.matrix_basis.copy() for pb in rig.pose.bones}


def hand_low_frame(rig, action, slot="handslot.r"):
    """Frame where a swing lands: the lowest point of the hand after its highest."""
    a, b = action.frame_range
    zs = []
    for k in range(int(a), int(b) + 1):
        rig.animation_data.action = action
        bpy.context.scene.frame_set(k)
        zs.append((k, (rig.matrix_world @ rig.pose.bones[slot].head).z))
    top = max(zs, key=lambda kz: kz[1])[0]
    after = [kz for kz in zs if kz[0] >= top]
    return top, min(after, key=lambda kz: kz[1])[0]


def ease(t):
    t = max(0.0, min(1.0, t))
    return t * t * (3 - 2 * t)


def curve(keys, f):
    """Piecewise smooth curve through (frame, value) keys."""
    if f <= keys[0][0]:
        return keys[0][1]
    for (fa, va), (fb, vb) in zip(keys, keys[1:]):
        if f <= fb:
            return va + (vb - va) * ease((f - fa) / (fb - fa))
    return keys[-1][1]


def make_clip(rig, name, n_frames, pose_at):
    """Bake a new clip. pose_at(f) returns {bone: matrix_basis}."""
    # Sample everything first: pose_at may switch the rig to other clips.
    poses = [pose_at(f) for f in range(n_frames + 1)]
    act = bpy.data.actions.new(name)
    act.use_fake_user = True
    rig.animation_data.action = act
    for f, basis in enumerate(poses):
        for pb in rig.pose.bones:
            if pb.name not in basis:
                continue
            loc, rot, sca = basis[pb.name].decompose()
            pb.rotation_mode = "QUATERNION"
            pb.location, pb.rotation_quaternion, pb.scale = loc, rot, sca
            for path in ("location", "rotation_quaternion", "scale"):
                pb.keyframe_insert(path, frame=f, group=pb.name)
    return act


def root_offset(rig, basis, off, squash=Vector((1, 1, 1)), spin=0.0, flip=0.0, pivot_z=0.6):
    """Move, turn and squash the whole body (through the root bone) in
    armature space. `spin` turns about the vertical, `flip` somersaults about
    the side axis through a point `pivot_z` up (both in degrees)."""
    pb = rig.pose.bones["root"]
    rest = pb.bone.matrix_local
    piv = Matrix.Translation((0, 0, pivot_z))
    rot = Matrix.Rotation(math.radians(spin), 4, "Z") @ piv @ Matrix.Rotation(math.radians(flip), 4, "X") @ piv.inverted()
    m = Matrix.Translation(off) @ rot @ Matrix.Diagonal(squash.to_4d()) @ rest @ basis["root"]
    basis["root"] = rest.inverted() @ m


def mix(a, b, w):
    """Blend two {bone: matrix_basis} poses."""
    out = {}
    for k in a:
        la, ra, sa = a[k].decompose()
        lb, rb, sb = b[k].decompose()
        out[k] = Matrix.LocRotScale(la.lerp(lb, w), ra.slerp(rb, w), sa.lerp(sb, w))
    return out


def special(rig, name, segs, fwd=None, up=None, side=None, spin=None, flip=None, sq=None, turns=(), blend=3):
    """Bake a special clip from pieces of existing clips plus whole-body motion.

    segs: [(clip, [(dst_frame, src_frame), ...]), ...] in order; each piece
    plays its clip retimed through those keys and fades in over `blend`
    frames from the end of the piece before. fwd/up/side/spin/flip/sq are
    curves ([(frame, value), ...]) for root_offset; turns are
    (bone, axis, curve-in-degrees) for extra upper-body lean.
    """
    cache = {}

    def at(clip, src):
        key = (clip, round(src, 3))
        if key not in cache:
            cache[key] = sample_action(rig, bpy.data.actions[clip], src)
        return cache[key]

    def pose_at(f):
        i = max(k for k, (_, keys) in enumerate(segs) if keys[0][0] <= f or k == 0)
        clip, keys = segs[i]
        b = at(clip, remap(keys, f))
        if i > 0 and f - keys[0][0] < blend:
            pclip, pkeys = segs[i - 1]
            b = mix(at(pclip, pkeys[-1][1]), b, ease((f - keys[0][0] + 1) / (blend + 1)))
        b = {k: v.copy() for k, v in b.items()}
        c = lambda ks, d=0.0: curve(ks, f) if ks else d
        s = c(sq, 1.0)
        w = 1.0 / math.sqrt(s)
        off = FWD * c(fwd) + UP * c(up) + SIDE * c(side)
        root_offset(rig, b, off, Vector((w, w, s)), spin=c(spin), flip=c(flip))
        for bone, axis, ks in turns:
            turn(rig, b, bone, axis, curve(ks, f))
        return b

    return make_clip(rig, name, SPECIAL_FRAMES, pose_at)


def turn(rig, basis, bone, axis, deg):
    """Extra rotation of `bone` about an armature-space axis, on top of the pose."""
    pb = rig.pose.bones[bone]
    rest = pb.bone.matrix_local.to_3x3()
    local_axis = (rest.inverted() @ Vector(axis)).normalized()
    q = Quaternion(local_axis, math.radians(deg))
    basis[bone] = basis[bone] @ q.to_matrix().to_4x4()


def remap(src_keys, f):
    """Special-clip frame -> source-clip frame (piecewise linear)."""
    if f <= src_keys[0][0]:
        return src_keys[0][1]
    for (fa, va), (fb, vb) in zip(src_keys, src_keys[1:]):
        if f <= fb:
            return va + (vb - va) * (f - fa) / (fb - fa)
    return src_keys[-1][1]


# Forward is -Y in Blender space for these rigs (toes point to -Y).
FWD = Vector((0, -1, 0))
UP = Vector((0, 0, 1))
SIDE = Vector((1, 0, 0))

# -------------------------------------------------------------- classes


def special_fighter(rig):
    """Leaping overhead slam: crouch, jump with the axe raised, crash down."""
    chop = bpy.data.actions["2H_Melee_Attack_Chop"]
    top, low = hand_low_frame(rig, chop)
    land = 12
    timing = [(0, 0), (8, top), (land, low), (SPECIAL_FRAMES, chop.frame_range[1])]
    cache = {}

    def pose_at(f):
        src = remap(timing, f)
        if src not in cache:
            cache[src] = sample_action(rig, chop, src)
        b = {k: v.copy() for k, v in cache[src].items()}
        jump = curve([(0, 0), (2, -0.08), (7, 0.62), (land, 0.0), (land + 2, -0.1), (18, 0.0)], f)
        lunge = curve([(0, 0), (2, 0), (land, 0.55), (17, 0.55), (SPECIAL_FRAMES, 0.0)], f)
        sq = curve([(0, 1.0), (2, 0.86), (5, 1.12), (land - 1, 1.04), (land + 1, 0.8), (land + 5, 1.0)], f)
        side = 1.0 / math.sqrt(sq)
        root_offset(rig, b, FWD * lunge + UP * jump, Vector((side, side, sq)))
        turn(rig, b, "chest", SIDE, curve([(0, 0), (7, -18), (land, 26), (land + 6, 14), (SPECIAL_FRAMES, 0)], f))
        return b

    return make_clip(rig, "Special_Fighter", SPECIAL_FRAMES, pose_at)


def gear_fighter(rig, pal, mat):
    head = rig.data.bones["head"]
    hz = (rig.matrix_world @ head.head_local).z
    # Barbarian hat sits on the head; horns come out of its sides.
    hat = bpy.data.objects["Barbarian_Hat"]
    bb = [hat.matrix_world @ Vector(c) for c in hat.bound_box]
    hx = max(abs(v.x) for v in bb) * 0.9
    top = max(v.z for v in bb)
    cz = hz + (top - hz) * 0.55
    ivory, dark = (236, 224, 196), (70, 62, 60)
    parts = []
    for s in (1, -1):
        m = Matrix.Translation((s * hx, 0.02, cz)) @ Matrix.Rotation(0 if s > 0 else math.pi, 4, "Z") @ Matrix.Rotation(-0.25, 4, "Y")
        parts.append((horn(0.78, 0.15, 1.5, m=m), ivory))
        parts.append((torus(0.12, 0.035, m=m @ Matrix.Rotation(math.pi / 2, 4, "Y")), dark))
    horns = colored(parts, mat, pal, "Fighter_Horns")
    on_bone(horns, rig, "head", Matrix())

    metal, spike, strap = (96, 100, 114), (230, 226, 214), (110, 70, 45)
    for s, bone in ((1, "upperarm.l"), (-1, "upperarm.r")):
        h, _ = bone_line(rig, bone)
        c = h + Vector((s * 0.17, 0.0, 0.08))
        parts = [
            (sphere(0.25, Matrix.Translation(c) @ Matrix.Diagonal((1.15, 1.1, 0.72, 1))), metal),
            (torus(0.22, 0.04, Matrix.Translation(c - Vector((0, 0, 0.05))) @ Matrix.Diagonal((1.15, 1.1, 1, 1))), strap),
        ]
        for k, (dx, dy) in enumerate(((0.05, 0.0), (0.0, -0.12), (0.0, 0.12))):
            p = c + Vector((s * dx, dy, 0.14))
            parts.append((cone(0.07, 0.0, 0.2, segs=6, m=Matrix.Translation(p + Vector((0, 0, 0.08))) @ Matrix.Rotation(s * 0.35, 4, "Y")), spike))
        side = "L" if s > 0 else "R"
        ob = colored(parts, mat, pal, f"Fighter_Pauldron_{side}")
        on_bone(ob, rig, bone, Matrix())

    hips = bone_line(rig, "hips")[0]
    belt = colored(
        [
            (torus(0.36, 0.06, Matrix.Translation(hips + Vector((0, 0, 0.06))) @ Matrix.Diagonal((1.05, 0.95, 1.5, 1))), strap),
            (box((0.26, 0.08, 0.2), Matrix.Translation(hips + Vector((0, -0.37, 0.06)))), (214, 170, 60)),
            (sphere(0.05, Matrix.Translation(hips + Vector((0, -0.42, 0.06)))), (236, 224, 196)),
        ],
        mat,
        pal,
        "Fighter_Belt",
    )
    on_bone(belt, rig, "hips", Matrix())
    return ["Fighter_Horns", "Fighter_Pauldron_L", "Fighter_Pauldron_R", "Fighter_Belt"]


# ------------------------------------------------------- shared gear bits


def bbox(name):
    ob = bpy.data.objects[name]
    pts = [ob.matrix_world @ Vector(c) for c in ob.bound_box]
    lo = Vector((min(p.x for p in pts), min(p.y for p in pts), min(p.z for p in pts)))
    hi = Vector((max(p.x for p in pts), max(p.y for p in pts), max(p.z for p in pts)))
    return lo, hi


def pauldrons(rig, pal, mat, prefix, shell, rim, stud, out=0.17, r=0.25):
    for s, bone in ((1, "upperarm.l"), (-1, "upperarm.r")):
        h, _ = bone_line(rig, bone)
        c = h + Vector((s * out, 0.0, 0.08))
        parts = [
            (sphere(r, Matrix.Translation(c) @ Matrix.Diagonal((1.15, 1.1, 0.72, 1))), shell),
            (torus(r * 0.9, 0.04, Matrix.Translation(c - Vector((0, 0, 0.05))) @ Matrix.Diagonal((1.15, 1.1, 1, 1))), rim),
            (sphere(0.055, Matrix.Translation(c + Vector((s * 0.06, 0, 0.17)))), stud),
        ]
        ob = colored(parts, mat, pal, f"{prefix}_Pauldron_{'L' if s > 0 else 'R'}")
        on_bone(ob, rig, bone, Matrix())
    return [f"{prefix}_Pauldron_L", f"{prefix}_Pauldron_R"]


def neck():
    return bone_line(bpy.data.objects["Rig"], "head")[0]


# ------------------------------------------------------------ other gear


def gear_hunter(rig, pal, mat):
    n = neck()
    fur, fur2, tooth = (120, 104, 92), (160, 146, 130), (236, 224, 196)
    parts = [(torus(0.34, 0.11, Matrix.Translation(n + Vector((0, 0, -0.04))) @ Matrix.Diagonal((1.1, 1.0, 0.8, 1))), fur)]
    for k in range(12):
        a = 2 * math.pi * k / 12
        p = n + Vector((0.38 * math.cos(a), 0.36 * math.sin(a), -0.04))
        tilt = Matrix.Rotation(a, 4, "Z") @ Matrix.Rotation(1.2, 4, "Y")
        parts.append((cone(0.07, 0.0, 0.22, segs=5, m=Matrix.Translation(p) @ tilt), fur2 if k % 2 else fur))
    pelt = colored(parts, mat, pal, "Hunter_Pelt")
    on_bone(pelt, rig, "chest", Matrix())
    parts = []
    for k in range(-3, 4):
        a = -math.pi / 2 + k * 0.28
        p = n + Vector((0.3 * math.cos(a), 0.36 * math.sin(a) - 0.02, -0.22 + 0.012 * abs(k)))
        parts.append((cone(0.035, 0.0, 0.13, segs=5, m=Matrix.Translation(p - Vector((0, 0, 0.05))) @ Matrix.Rotation(math.pi, 4, "X")), tooth))
    teeth = colored(parts, mat, pal, "Hunter_Necklace")
    on_bone(teeth, rig, "chest", Matrix())
    return ["Hunter_Pelt", "Hunter_Necklace"]


def gear_guard(rig, pal, mat):
    lo, hi = bbox("Knight_Helmet")
    red, gold = (200, 52, 52), (222, 176, 64)
    parts = []
    for k in range(7):
        t = k / 6
        p = Vector((0, 0.05 + 0.42 * t, hi.z - 0.02 + 0.22 * math.sin(math.pi * (0.15 + 0.7 * t))))
        parts.append((sphere(0.13 - 0.04 * t, Matrix.Translation(p) @ Matrix.Diagonal((0.55, 1.0, 1.0, 1))), red))
    parts.append((box((0.1, 0.16, 0.08), Matrix.Translation((0, 0.02, hi.z - 0.06))), gold))
    plume = colored(parts, mat, pal, "Guard_Plume")
    on_bone(plume, rig, "head", Matrix())
    return ["Guard_Plume"] + pauldrons(rig, pal, mat, "Guard", (150, 156, 170), gold, gold, out=0.2, r=0.27)


def gear_paladin(rig, pal, mat):
    lo, hi = bbox("Knight_Helmet")
    gold, white = (222, 176, 64), (240, 240, 240)
    parts = []
    for s in (1, -1):
        for k in range(4):
            a = 0.35 + 0.3 * k
            base = Vector((s * hi.x * 0.95, 0.05, (lo.z + hi.z) / 2 + 0.05))
            m = (Matrix.Translation(base) @ Matrix.Rotation(s * -a, 4, "Y") @ Matrix.Translation((0, 0.09 * k, 0.36 - 0.04 * k))
                 @ Matrix.Diagonal((0.25, 1.0, 1.0, 1)))
            parts.append((cone(0.13, 0.0, 0.8 - 0.1 * k, segs=6, m=m), white if k % 2 else gold))
    wings = colored(parts, mat, pal, "Paladin_Wings")
    on_bone(wings, rig, "head", Matrix())
    return ["Paladin_Wings"] + pauldrons(rig, pal, mat, "Paladin", gold, white, white, out=0.2, r=0.27)


def gear_archer(rig, pal, mat):
    chest = bone_line(rig, "chest")[0]
    leather, wood, feather, green = (120, 76, 48), (150, 110, 70), (236, 236, 236), (90, 150, 80)
    back = chest + Vector((0.1, 0.36, 0.1))
    tilt = Matrix.Translation(back) @ Matrix.Rotation(-0.45, 4, "Y")
    parts = [(cone(0.13, 0.15, 0.62, segs=8, m=tilt), leather), (torus(0.15, 0.03, tilt @ Matrix.Translation((0, 0, 0.2))), wood)]
    for k, (dx, dy) in enumerate(((0.05, 0.0), (-0.05, 0.04), (0.0, -0.05))):
        parts.append((cone(0.018, 0.018, 0.4, segs=5, m=tilt @ Matrix.Translation((dx, dy, 0.42))), wood))
        parts.append((box((0.02, 0.1, 0.16), tilt @ Matrix.Translation((dx, dy, 0.6))), feather if k != 1 else green))
    q = colored(parts, mat, pal, "Archer_Quiver")
    on_bone(q, rig, "chest", Matrix())
    lo, hi = bbox("Rogue_Head_Hooded")
    f = Matrix.Translation((hi.x * 0.85, 0.15, hi.z - 0.25)) @ Matrix.Rotation(-0.5, 4, "Y") @ Matrix.Rotation(0.4, 4, "X")
    fe = colored([(cone(0.1, 0.0, 0.8, segs=4, m=f @ Matrix.Diagonal((0.4, 1.0, 1.0, 1)) @ Matrix.Translation((0, 0, 0.35))), (200, 60, 50))],
                 mat, pal, "Archer_Feather")
    on_bone(fe, rig, "head", Matrix())
    return ["Archer_Quiver", "Archer_Feather"]


def gear_assassin(rig, pal, mat):
    lo, hi = bbox("Rogue_Head_Hooded")
    dark, red, steel = (46, 44, 56), (170, 40, 46), (190, 196, 210)
    zc = lo.z + 0.3
    mask = colored([(sphere(0.38, Matrix.Translation((0, lo.y + 0.32, zc)) @ Matrix.Diagonal((1.15, 0.7, 0.45, 1)), segs=12, rings=6), dark)],
                   mat, pal, "Assassin_Mask")
    on_bone(mask, rig, "head", Matrix())
    hips = bone_line(rig, "hips")[0]
    parts = [(torus(0.36, 0.06, Matrix.Translation(hips + Vector((0, 0, 0.06))) @ Matrix.Diagonal((1.05, 0.95, 1.4, 1))), red)]
    for s in (1, -1):
        m = Matrix.Translation(hips + Vector((s * 0.3, 0.12, 0.0))) @ Matrix.Rotation(s * 0.5, 4, "Y")
        parts.append((box((0.05, 0.03, 0.3), m @ Matrix.Translation((0, 0, -0.12))), steel))
        parts.append((box((0.06, 0.06, 0.1), m @ Matrix.Translation((0, 0, 0.06))), dark))
    sash = colored(parts, mat, pal, "Assassin_Sash")
    on_bone(sash, rig, "hips", Matrix())
    return ["Assassin_Mask", "Assassin_Sash"]


def gear_sentry(rig, pal, mat):
    lo, hi = bbox("Rogue_Head")
    brass, glass, strap = (210, 160, 70), (110, 190, 220), (80, 60, 50)
    z = hi.z - 0.3
    parts = [(torus(0.56, 0.035, Matrix.Translation((0, (lo.y + hi.y) / 2, z)) @ Matrix.Diagonal((0.98, 0.93, 1.6, 1))), strap)]
    for s in (1, -1):
        m = Matrix.Translation((s * 0.17, lo.y + 0.06, z + 0.02)) @ Matrix.Rotation(math.pi / 2, 4, "X")
        parts.append((torus(0.11, 0.04, m), brass))
        parts.append((cone(0.1, 0.1, 0.03, segs=10, m=m), glass))
    g = colored(parts, mat, pal, "Sentry_Goggles")
    on_bone(g, rig, "head", Matrix())
    return ["Sentry_Goggles"] + pauldrons(rig, pal, mat, "Sentry", (110, 80, 60), brass, brass, out=0.14, r=0.21)


def gear_healer(rig, pal, mat):
    n = neck()
    gold, white, green = (230, 190, 70), (245, 245, 240), (110, 180, 90)
    c = n + Vector((0, -0.36, -0.2))
    m = Matrix.Translation(c) @ Matrix.Rotation(math.pi / 2, 4, "X")
    parts = [(torus(0.13, 0.04, m), gold), (box((0.05, 0.03, 0.17), Matrix.Translation(c)), white), (box((0.17, 0.03, 0.05), Matrix.Translation(c)), white)]
    for k in range(8):
        a = 2 * math.pi * k / 8
        parts.append((cone(0.03, 0.0, 0.1, segs=4, m=m @ Matrix.Rotation(a, 4, "Z") @ Matrix.Translation((0.2, 0, 0)) @ Matrix.Rotation(math.pi / 2, 4, "Y")), gold))
    parts.append((torus(0.34, 0.04, Matrix.Translation(n + Vector((0, 0, -0.04))) @ Matrix.Diagonal((1.05, 1.0, 1, 1))), green))
    charm = colored(parts, mat, pal, "Healer_Charm")
    on_bone(charm, rig, "chest", Matrix())
    return ["Healer_Charm"]


def gear_keeper(rig, pal, mat):
    hips = bone_line(rig, "hips")[0]
    metal, glow, dark = (70, 70, 80), (250, 210, 110), (90, 60, 110)
    base = hips + Vector((0.52, -0.18, -0.1))
    parts = [
        (box((0.17, 0.17, 0.04), Matrix.Translation(base + Vector((0, 0, 0.13)))), metal),
        (box((0.17, 0.17, 0.04), Matrix.Translation(base - Vector((0, 0, 0.1)))), metal),
        (sphere(0.08, Matrix.Translation(base)), glow),
        (torus(0.05, 0.015, Matrix.Translation(base + Vector((0, 0, 0.2))) @ Matrix.Rotation(math.pi / 2, 4, "X")), metal),
    ]
    for dx, dy in ((0.07, 0.07), (-0.07, 0.07), (0.07, -0.07), (-0.07, -0.07)):
        parts.append((box((0.02, 0.02, 0.22), Matrix.Translation(base + Vector((dx, dy, 0.015)))), metal))
    lan = colored(parts, mat, pal, "Keeper_Lantern")
    on_bone(lan, rig, "hips", Matrix())
    n = neck()
    parts = []
    for k in range(10):
        a = math.pi * (0.1 + 0.8 * k / 9)
        p = n + Vector((0.34 * math.cos(a), 0.3 * math.sin(a), 0.02))
        parts.append((box((0.14, 0.05, 0.3), Matrix.Translation(p) @ Matrix.Rotation(a + math.pi / 2, 4, "Z") @ Matrix.Rotation(-0.25, 4, "X")), dark))
    col = colored(parts, mat, pal, "Keeper_Collar")
    on_bone(col, rig, "chest", Matrix())
    return ["Keeper_Lantern", "Keeper_Collar"]


def gear_mage(rig, pal, mat):
    chest = bone_line(rig, "chest")[0]
    parts = []
    for k, col in enumerate(((120, 90, 230), (90, 200, 240), (240, 120, 200))):
        a = math.pi * (0.25 + 0.5 * k) + math.pi
        p = chest + Vector((0.8 * math.cos(a), 0.5 * math.sin(a) + 0.2, 0.2 + 0.18 * (k % 2)))
        parts.append((sphere(0.13, Matrix.Translation(p), segs=8, rings=5), col))
        parts.append((torus(0.15, 0.015, Matrix.Translation(p) @ Matrix.Rotation(0.6 * k, 4, "X")), (240, 230, 160)))
    orbs = colored(parts, mat, pal, "Mage_Orbs")
    on_bone(orbs, rig, "chest", Matrix())
    return ["Mage_Orbs"]


def gear_warlock(rig, pal, mat):
    lo, hi = bbox("Mage_Head")
    dark, purple = (50, 40, 60), (120, 60, 160)
    parts = []
    for s in (1, -1):
        m = (Matrix.Translation((s * hi.x * 0.75, 0.08, hi.z - 0.28)) @ Matrix.Rotation(0 if s > 0 else math.pi, 4, "Z")
             @ Matrix.Rotation(-0.5, 4, "Y"))
        parts.append((horn(0.6, 0.13, 1.2, m=m), dark))
    horns = colored(parts, mat, pal, "Warlock_Horns")
    on_bone(horns, rig, "head", Matrix())
    n = neck()
    parts = []
    for k in range(9):
        a = math.pi * (0.05 + 0.9 * k / 8)
        p = n + Vector((0.36 * math.cos(a), 0.3 * math.sin(a), 0.05))
        parts.append((cone(0.08, 0.0, 0.4, segs=4, m=Matrix.Translation(p) @ Matrix.Rotation(a + math.pi / 2, 4, "Z") @ Matrix.Rotation(-0.35, 4, "X")
                           @ Matrix.Translation((0, 0, 0.12))), purple if k % 2 else dark))
    col = colored(parts, mat, pal, "Warlock_Collar")
    on_bone(col, rig, "chest", Matrix())
    return ["Warlock_Horns", "Warlock_Collar"]


# --------------------------------------------------------- other specials
# Clip lengths in frames at 24 fps: 1H swings ~25, 2H chop 39, spin 58,
# Spellcast_Long 61, Spellcast_Raise 50, Spellcast_Shoot 22, Dodge 10,
# Blocking 26, ranged shots 26, Dualwield chop 30.

N = SPECIAL_FRAMES


def special_hunter(rig):
    """Whirling pounce: hop in spinning, axes out."""
    return special(rig, "Special_Hunter", [("2H_Melee_Attack_Spin", [(0, 0), (3, 8), (19, 50), (N, 57)])],
                   up=[(0, 0), (2, -0.06), (8, 0.45), (14, 0.2), (17, 0), (19, -0.06), (22, 0)],
                   fwd=[(0, 0), (3, 0), (14, 0.7), (20, 0.7), (N, 0)],
                   spin=[(0, 0), (4, 0), (18, -360), (N, -360)],
                   sq=[(0, 1), (2, 0.88), (6, 1.1), (16, 1.0), (18, 0.86), (21, 1.0)])


def special_guard(rig):
    """Shield rush: shield up, charge, bash."""
    chop = bpy.data.actions["1H_Melee_Attack_Slice_Horizontal"]
    return special(rig, "Special_Guard", [("Blocking", [(0, 0), (11, 10)]), ("1H_Melee_Attack_Slice_Horizontal", [(11, 6), (17, 14), (N, chop.frame_range[1])])],
                   fwd=[(0, 0), (2, -0.08), (10, 1.0), (19, 1.0), (N, 0.0)],
                   up=[(0, 0), (5, 0.08), (9, 0.0)],
                   sq=[(0, 1), (2, 0.9), (5, 1.06), (10, 1.0), (12, 0.88), (15, 1.0)],
                   turns=[("chest", SIDE, [(0, 0), (3, 4), (9, 20), (13, 8), (N, 0)])])


def special_paladin(rig):
    """Judgement: sword to the sky, leap, smite."""
    chop = bpy.data.actions["2H_Melee_Attack_Chop"]
    top, low = hand_low_frame(rig, chop)
    return special(rig, "Special_Paladin", [("Spellcast_Raise", [(0, 0), (8, 24)]), ("2H_Melee_Attack_Chop", [(8, top), (13, low), (N, chop.frame_range[1])])],
                   up=[(0, 0), (3, 0.12), (8, 0.5), (13, 0), (15, -0.08), (18, 0)],
                   fwd=[(0, 0), (6, 0.1), (13, 0.45), (18, 0.45), (N, 0)],
                   spin=[(0, 0), (8, 0), (13, 0)],
                   sq=[(0, 1), (3, 1.12), (8, 1.05), (12, 1.0), (14, 0.82), (18, 1.0)],
                   turns=[("chest", SIDE, [(0, 0), (6, -14), (13, 24), (19, 10), (N, 0)])])


def special_archer(rig):
    """Backflip shot: flip away, land, loose a bolt."""
    return special(rig, "Special_Archer", [("Dodge_Backward", [(0, 0), (10, 9)]), ("2H_Ranged_Shoot", [(10, 2), (N, 25)])],
                   up=[(0, 0), (1, -0.06), (6, 0.85), (11, 0), (13, -0.06), (15, 0)],
                   fwd=[(0, 0), (11, -0.8), (20, -0.8), (N, 0)],
                   flip=[(0, 0), (2, 0), (10, -360), (N, -360)],
                   sq=[(0, 1), (1, 0.86), (4, 1.08), (10, 1.0), (12, 0.86), (15, 1.0)])


def special_sentry(rig):
    """Spin and fire: pirouette out of reach and shoot twice."""
    return special(rig, "Special_Sentry", [("Dodge_Backward", [(0, 0), (8, 9)]), ("1H_Ranged_Shoot", [(8, 4), (16, 25)]), ("1H_Ranged_Shoot", [(16, 4), (N, 25)])],
                   up=[(0, 0), (4, 0.35), (8, 0)],
                   side=[(0, 0), (8, 0.6), (20, 0.6), (N, 0)],
                   spin=[(0, 0), (8, 360), (N, 360)],
                   sq=[(0, 1), (1, 0.9), (4, 1.06), (8, 0.9), (10, 1.0)])


def special_assassin(rig):
    """Shadow strike: crouch, blur forward, spinning double slash."""
    return special(rig, "Special_Assassin", [("Dualwield_Melee_Attack_Chop", [(0, 0), (6, 8), (14, 20), (N, 30)]),],
                   fwd=[(0, 0), (3, -0.1), (8, 1.3), (18, 1.3), (N, 0)],
                   up=[(0, 0), (3, -0.1), (6, 0.15), (9, 0)],
                   spin=[(0, 0), (8, 0), (14, 360), (N, 360)],
                   sq=[(0, 1), (3, 0.82), (6, 1.12), (9, 1.0), (14, 0.92), (17, 1.0)],
                   turns=[("chest", SIDE, [(0, 0), (3, 22), (8, 10), (14, 18), (N, 0)])])


def special_healer(rig):
    """Blessing: rise up, staff high, settle gently."""
    return special(rig, "Special_Healer", [("Spellcast_Raise", [(0, 0), (12, 22), (N, 50)])],
                   up=[(0, 0), (2, -0.05), (10, 0.4), (17, 0.4), (23, 0)],
                   spin=[(0, 0), (4, 0), (18, 360), (N, 360)],
                   sq=[(0, 1), (2, 0.9), (6, 1.08), (12, 1.0), (23, 0.94), (N, 1.0)])


def special_keeper(rig):
    """Tome slam: hop, slam the book down, shockwave stance."""
    return special(rig, "Special_Keeper", [("Spellcast_Long", [(0, 0), (10, 30), (14, 40), (N, 60)])],
                   up=[(0, 0), (3, -0.05), (9, 0.4), (13, 0), (15, -0.08), (18, 0)],
                   sq=[(0, 1), (3, 0.88), (7, 1.1), (12, 1.0), (14, 0.8), (18, 1.0)],
                   turns=[("chest", SIDE, [(0, 0), (8, -12), (13, 22), (N, 0)])])


def special_mage(rig):
    """Arcane burst: float, gather, blast, recoil."""
    return special(rig, "Special_Mage", [("Spellcast_Shoot", [(0, 0), (11, 8), (15, 13), (N, 22)])],
                   up=[(0, 0), (8, 0.45), (15, 0.5), (22, 0)],
                   fwd=[(0, 0), (11, 0.1), (15, -0.4), (22, -0.4), (N, 0)],
                   sq=[(0, 1), (4, 1.06), (10, 0.9), (13, 1.14), (17, 1.0)],
                   turns=[("chest", SIDE, [(0, 0), (10, 12), (14, -20), (N, 0)])])


def special_warlock(rig):
    """Dark rite: levitate spinning, then call down the curse."""
    return special(rig, "Special_Warlock", [("Spellcast_Long", [(0, 0), (16, 36), (N, 60)])],
                   up=[(0, 0), (8, 0.55), (16, 0.6), (20, 0), (22, -0.06), (N, 0)],
                   spin=[(0, 0), (3, 0), (16, -540), (N, -540)],
                   sq=[(0, 1), (2, 0.9), (6, 1.1), (16, 1.0), (20, 0.84), (23, 1.0)])


BULK_LIGHT = {
    "upperarm.l": (1.12, 1.0), "upperarm.r": (1.12, 1.0),
    "lowerarm.l": (1.15, 1.0), "lowerarm.r": (1.15, 1.0),
    "wrist.l": (1.15, 1.08), "wrist.r": (1.15, 1.08),
    "hand.l": (1.1, 1.22), "hand.r": (1.1, 1.22),
    "upperleg.l": (1.08, 1.0), "upperleg.r": (1.08, 1.0),
    "lowerleg.l": (1.08, 1.0), "lowerleg.r": (1.08, 1.0),
    "foot.l": (1.06, 1.06), "foot.r": (1.06, 1.06),
}

BULK_HEAVY = {
    "upperarm.l": (1.3, 1.0), "upperarm.r": (1.3, 1.0),
    "lowerarm.l": (1.35, 1.0), "lowerarm.r": (1.35, 1.0),
    "wrist.l": (1.3, 1.1), "wrist.r": (1.3, 1.1),
    "hand.l": (1.2, 1.32), "hand.r": (1.2, 1.32),
    "chest": (1.12, 1.0), "spine": (1.06, 1.0),
    "upperleg.l": (1.18, 1.0), "upperleg.r": (1.18, 1.0),
    "lowerleg.l": (1.15, 1.0), "lowerleg.r": (1.15, 1.0),
    "foot.l": (1.1, 1.1), "foot.r": (1.1, 1.1),
}

MODELS = {
    "Barbarian": dict(
        src="adventurers/Barbarian.glb",
        bulk=BULK_HEAVY,
        weapons={"2H_Axe": 1.25, "1H_Axe": 1.2, "1H_Axe_Offhand": 1.2},
        gear=[gear_fighter, gear_hunter],
        specials=[special_fighter, special_hunter],
    ),
    "Knight": dict(
        src="adventurers/Knight.glb",
        bulk=BULK_HEAVY,
        weapons={"1H_Sword": 1.15, "2H_Sword": 1.15, "Badge_Shield": 1.2},
        gear=[gear_guard, gear_paladin],
        specials=[special_guard, special_paladin],
    ),
    "Mage": dict(
        src="adventurers/Mage.glb",
        bulk=BULK_LIGHT,
        weapons={"2H_Staff": 1.15, "1H_Wand": 1.2, "Spellbook": 1.25, "Spellbook_open": 1.2},
        gear=[gear_healer, gear_keeper, gear_mage, gear_warlock],
        specials=[special_healer, special_keeper, special_mage, special_warlock],
    ),
    "Rogue": dict(
        src="adventurers/Rogue.glb",
        bulk=BULK_LIGHT,
        weapons={"1H_Crossbow": 1.15, "Knife_Offhand": 1.15},
        gear=[gear_sentry],
        specials=[special_sentry],
    ),
    "Rogue_Hooded": dict(
        src="adventurers/Rogue_Hooded.glb",
        bulk=BULK_LIGHT,
        weapons={"2H_Crossbow": 1.12, "Knife": 1.2, "Knife_Offhand": 1.2},
        gear=[gear_archer, gear_assassin],
        specials=[special_archer, special_assassin],
    ),
}


def main():
    args = sys.argv[sys.argv.index("--") + 1:]
    key, out = args[0], args[1]
    cfg = MODELS[key]
    here = os.path.dirname(os.path.abspath(__file__))
    src = os.path.join(here, "..", "..", "assets", cfg["src"])
    rig = load(src)
    if rig.animation_data is None:
        rig.animation_data_create()
    mesh = next(o for o in bpy.data.objects if o.type == "MESH" and o.vertex_groups)
    mat = mesh.data.materials[0]
    img = next(n.image for n in mat.node_tree.nodes if n.type == "TEX_IMAGE")
    pal = Palette(img)

    bulk(rig, cfg["bulk"])
    for name, k in cfg["weapons"].items():
        if name in bpy.data.objects:
            bpy.data.objects[name].scale *= k
    added = []
    for g in cfg["gear"]:
        added += g(rig, pal, mat)
    for s in cfg["specials"]:
        s(rig)
    rig.animation_data.action = bpy.data.actions["Idle"]
    bpy.context.scene.frame_set(0)
    print("ADDED", added)
    export(out)
    if "--blend" in args:
        bpy.ops.wm.save_as_mainfile(filepath=args[args.index("--blend") + 1])


main()
