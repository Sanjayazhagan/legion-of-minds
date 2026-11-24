"""Build the signature pieces worn only by the six named hybrids.

Run from game/:
  blender -b -P tools/blender/hybrids.py -- assets/troops/Hybrids.glb [--blend x.blend]

Every troop body shares the KayKit skeleton, so the pieces are made on the
bare Knight rig and saved with that skeleton only (no body, no clips). The
game wears them on whichever body a hybrid has, by bone name, the same way a
hybrid borrows its second class's gear (`Model::worn_by`).

  Sig_BattlePriest   Fighter + Healer    a sun standard raised behind the head
  Sig_SpiritRanger   Archer + Healer     a leaf mantle and three spirit wisps
  Sig_Spellblade     Fighter + Archer    three rune blades fanned on the back
  Sig_Bastion        Guard + Sentry      a crenellated tower shield on the back
  Sig_TempleKnight   Guard + Keeper      a candle nimbus behind the head
  Sig_Oracle         Sentry + Keeper     a floating eye in brass rings

All shapes are built here from primitives, so nothing is copied from other
art. Colours come from a small swatch texture made by this script.
"""
import math
import os
import sys

import bmesh
import bpy
from mathutils import Matrix, Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from troops import bone_line, colored, cone, load, on_bone, sphere, torus, box, transformed  # noqa: E402

# ----------------------------------------------------------------- swatches

CELLS = 8  # an 8 x 8 grid of flat colours
CELL_PX = 4


class Swatches:
    """Hands out one texture cell per colour; the image is painted at the end."""

    def __init__(self):
        self.cells = {}

    def uv(self, rgb):
        rgb = tuple(rgb)
        if rgb not in self.cells:
            if len(self.cells) >= CELLS * CELLS:
                raise RuntimeError("out of swatch cells")
            self.cells[rgb] = len(self.cells)
        i = self.cells[rgb]
        x, y = i % CELLS, i // CELLS
        return Vector(((x + 0.5) / CELLS, (y + 0.5) / CELLS))

    def material(self):
        size = CELLS * CELL_PX
        img = bpy.data.images.new("hybrid_swatches", size, size, alpha=False)
        px = [0.0] * (size * size * 4)
        for rgb, i in self.cells.items():
            cx, cy = i % CELLS, i // CELLS
            for yy in range(cy * CELL_PX, (cy + 1) * CELL_PX):
                for xx in range(cx * CELL_PX, (cx + 1) * CELL_PX):
                    k = (yy * size + xx) * 4
                    px[k:k + 4] = [rgb[0] / 255, rgb[1] / 255, rgb[2] / 255, 1.0]
        img.pixels = px
        img.file_format = "PNG"
        img.pack()
        return img


def make_material():
    mat = bpy.data.materials.new("Hybrid_Gear")
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = next(n for n in nt.nodes if n.type == "BSDF_PRINCIPLED")
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.interpolation = "Closest"
    nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    bsdf.inputs["Roughness"].default_value = 0.8
    return mat, tex


# ------------------------------------------------------------- extra shapes


def cyl(r, depth, segs=8, m=Matrix()):
    return cone(r, r, depth, segs=segs, m=m)


def blade(length, width, m=Matrix()):
    """A flat diamond-section blade pointing +Z, base at the origin."""
    bm = bmesh.new()
    t = width * 0.22
    body = [(-width / 2, 0, 0), (0, -t, 0), (width / 2, 0, 0), (0, t, 0)]
    top = [(-width / 2, 0, length * 0.8), (0, -t, length * 0.8), (width / 2, 0, length * 0.8), (0, t, length * 0.8)]
    a = [bm.verts.new(v) for v in body]
    b = [bm.verts.new(v) for v in top]
    tip = bm.verts.new((0, 0, length))
    for j in range(4):
        bm.faces.new((a[j], a[(j + 1) % 4], b[(j + 1) % 4], b[j]))
        bm.faces.new((b[j], b[(j + 1) % 4], tip))
    bm.faces.new(list(reversed(a)))
    return transformed(bm, m)


def leaf(length, width, m=Matrix()):
    """A pointed leaf lying in the XY plane, stem at the origin, tip along +X."""
    return sphere(0.5, m @ Matrix.Translation((length / 2, 0, 0)) @ Matrix.Diagonal((length, width, width * 0.25, 1)), segs=8, rings=4)


def flame(r, m=Matrix(), stretch=2.6):
    """A flame or wisp: a ball drawn up into a soft point."""
    bm = sphere(r, segs=10, rings=7)
    for v in bm.verts:
        if v.co.z > 0:
            t = v.co.z / r
            v.co.z *= stretch
            k = 1 - 0.75 * t * t
            v.co.x *= k
            v.co.y *= k
    return transformed(bm, m)


# Forward is -Y, up is +Z; the back of the body is +Y.

GOLD, GOLD_DK, WHITE, IVORY = (236, 186, 64), (176, 128, 40), (245, 243, 236), (236, 224, 196)
STEEL, STEEL_DK, WOOD = (176, 184, 198), (96, 102, 116), (120, 82, 52)
RED, CRIMSON = (206, 54, 48), (150, 30, 40)


def sig_battle_priest(rig, sw, mat):
    """A sun standard: a pole up the back, a gold sunburst over the head,
    red-and-white streamers. Fighter's war banner, Healer's sun."""
    chest = bone_line(rig, "chest")[0]
    base = chest + Vector((0.0, 0.5, -0.35))
    top = Vector((0.0, 0.62, 3.05))
    axis = top - base
    tilt = Matrix.Translation(base) @ axis.to_track_quat("Z", "Y").to_matrix().to_4x4()
    n = axis.length
    parts = [
        (cyl(0.045, n, segs=8, m=tilt @ Matrix.Translation((0, 0, n / 2))), WOOD),
        (torus(0.07, 0.025, tilt @ Matrix.Translation((0, 0, 0.55))), GOLD_DK),
        (torus(0.07, 0.025, tilt @ Matrix.Translation((0, 0, 1.25))), GOLD_DK),
        # The strap that holds it to the back.
        (torus(0.42, 0.04, Matrix.Translation(chest + Vector((0, 0.02, 0.05))) @ Matrix.Rotation(0.5, 4, "Y") @ Matrix.Diagonal((1.0, 0.95, 1.0, 1))), WOOD),
    ]
    sun = Matrix.Translation(top) @ Matrix.Rotation(math.pi / 2, 4, "X")
    parts.append((cyl(0.24, 0.07, segs=16, m=sun), GOLD))
    parts.append((cyl(0.15, 0.1, segs=16, m=sun), WHITE))
    parts.append((torus(0.27, 0.03, sun, segs=16), GOLD_DK))
    for k in range(12):
        a = 2 * math.pi * k / 12
        long = 0.26 if k % 2 == 0 else 0.17
        m = sun @ Matrix.Rotation(a, 4, "Z") @ Matrix.Translation((0.27 + long / 2, 0, 0)) @ Matrix.Rotation(math.pi / 2, 4, "Y")
        parts.append((cone(0.06, 0.0, long, segs=4, m=m @ Matrix.Diagonal((1, 0.45, 1, 1))), GOLD))
    # Streamers hang from a crossbar under the sun, swept back a little.
    bar = top + Vector((0, 0.02, -0.4))
    parts.append((cyl(0.03, 0.7, segs=6, m=Matrix.Translation(bar) @ Matrix.Rotation(math.pi / 2, 4, "Y")), WOOD))
    for s, col in ((-1, RED), (1, WHITE)):
        for k in range(3):
            h = 0.22
            p = bar + Vector((s * 0.24, 0.04 + 0.05 * k, -0.08 - h * k))
            parts.append((box((0.2 - 0.03 * k, 0.03, h), Matrix.Translation(p) @ Matrix.Rotation(0.18 * k, 4, "X")), col if k % 2 == 0 else (RED if col == WHITE else WHITE)))
        parts.append((cone(0.1, 0.0, 0.16, segs=4, m=Matrix.Translation(bar + Vector((s * 0.24, 0.16, -0.66))) @ Matrix.Rotation(math.pi, 4, "X") @ Matrix.Diagonal((1, 0.3, 1, 1))), col))
    ob = colored(parts, mat, sw, "Sig_BattlePriest")
    on_bone(ob, rig, "chest", Matrix())


def sig_spirit_ranger(rig, sw, mat):
    """A mantle of leaves round the shoulders and three pale spirit wisps
    drifting behind: the ranger's woods and the healer's grace."""
    n = bone_line(rig, "head")[0]
    greens = ((84, 150, 70), (120, 184, 80), (60, 120, 64))
    parts = [(torus(0.4, 0.07, Matrix.Translation(n + Vector((0, 0.02, -0.06))) @ Matrix.Diagonal((1.08, 1.0, 0.8, 1))), (96, 70, 46))]
    for k in range(16):
        a = 2 * math.pi * k / 16
        p = n + Vector((0.42 * math.cos(a), 0.4 * math.sin(a), -0.05 + 0.02 * (k % 2)))
        m = Matrix.Translation(p) @ Matrix.Rotation(a, 4, "Z") @ Matrix.Rotation(0.55, 4, "Y")
        parts.append((leaf(0.34, 0.16, m), greens[k % 3]))
    # A few longer fronds down the back.
    for s in (-1, 0, 1):
        p = n + Vector((0.18 * s, 0.4, -0.1))
        m = Matrix.Translation(p) @ Matrix.Rotation(math.pi / 2 + 0.2 * s, 4, "Z") @ Matrix.Rotation(1.1, 4, "Y")
        parts.append((leaf(0.55, 0.2, m), greens[(s + 1) % 3]))
    glow, core = (170, 255, 210), (240, 255, 246)
    for x, y, z, r in ((0.72, 0.42, 1.65, 0.13), (-0.72, 0.42, 1.65, 0.13), (0.0, 0.72, 2.55, 0.16)):
        c = Vector((x, y, z))
        parts.append((flame(r, Matrix.Translation(c), stretch=1.9), glow))
        parts.append((sphere(r * 0.55, Matrix.Translation(c + Vector((0, -r * 0.4, 0))), segs=8, rings=5), core))
        parts.append((torus(r * 1.5, 0.012, Matrix.Translation(c) @ Matrix.Rotation(0.5, 4, "X")), (120, 220, 170)))
    ob = colored(parts, mat, sw, "Sig_SpiritRanger")
    on_bone(ob, rig, "chest", Matrix())


def sig_spellblade(rig, sw, mat):
    """Three rune blades hovering in a fan behind the back, edges glowing."""
    chest = bone_line(rig, "chest")[0]
    pivot = chest + Vector((0, 0.62, -0.2))
    edge, rune, hilt = (110, 200, 255), (200, 245, 255), (60, 66, 96)
    parts = [(torus(0.2, 0.04, Matrix.Translation(pivot) @ Matrix.Rotation(math.pi / 2, 4, "X"), segs=16), hilt),
             (sphere(0.11, Matrix.Translation(pivot)), edge)]
    for k, ang in enumerate((-0.62, 0.0, 0.62)):
        length = 1.45 if k == 1 else 1.15
        m = Matrix.Translation(pivot) @ Matrix.Rotation(ang, 4, "Y") @ Matrix.Translation((0, 0, 0.22))
        parts.append((box((0.3, 0.07, 0.06), m), hilt))
        parts.append((cyl(0.035, 0.22, segs=6, m=m @ Matrix.Translation((0, 0, -0.12))), hilt))
        parts.append((blade(length, 0.2, m @ Matrix.Translation((0, 0, 0.03))), STEEL))
        parts.append((blade(length * 0.86, 0.06, m @ Matrix.Translation((0, 0, 0.05)) @ Matrix.Diagonal((1, 3.2, 1, 1))), edge))
        for j in range(3):
            parts.append((box((0.07, 0.24, 0.07), m @ Matrix.Translation((0, 0, 0.32 + 0.25 * j)) @ Matrix.Rotation(math.pi / 4, 4, "Y")), rune))
    ob = colored(parts, mat, sw, "Sig_Spellblade")
    on_bone(ob, rig, "chest", Matrix())


def sig_bastion(rig, sw, mat):
    """A tower shield slung on the back, crenellated like a castle wall, with
    an arrow slit: the guard's shield and the sentry's lookout in one."""
    chest = bone_line(rig, "chest")[0]
    c = chest + Vector((0, 0.56, -0.15))
    stone, stone_dk, brass = (150, 146, 138), (104, 100, 96), (210, 160, 70)
    w, h, d = 0.86, 1.3, 0.1
    parts = []
    # The face is three slightly angled panels, so it reads as curved.
    for k, a in ((-1, 0.28), (0, 0.0), (1, -0.28)):
        m = Matrix.Translation(c + Vector((k * w / 3 * 0.97, 0.03 * abs(k) * -1, 0))) @ Matrix.Rotation(a, 4, "Z")
        parts.append((box((w / 3 + 0.01, d, h), m), stone if k == 0 else stone_dk))
    # Courses of stone.
    for j in range(1, 4):
        parts.append((box((w + 0.04, d + 0.04, 0.035), Matrix.Translation(c + Vector((0, 0.0, -h / 2 + j * h / 4)))), stone_dk))
    # Battlements.
    for k in (-1, 0, 1):
        parts.append((box((0.2, d + 0.02, 0.2), Matrix.Translation(c + Vector((k * 0.3, 0, h / 2 + 0.1)))), stone))
    # Brass rim and rivets.
    for s in (-1, 1):
        parts.append((box((0.05, d + 0.06, h + 0.04), Matrix.Translation(c + Vector((s * (w / 2 + 0.02), 0, 0))) @ Matrix.Rotation(-s * 0.28, 4, "Z")), brass))
        for j in range(4):
            parts.append((sphere(0.035, Matrix.Translation(c + Vector((s * (w / 2 - 0.03), d / 2 + 0.03, -h / 2 + 0.2 + j * 0.3)))), brass))
    parts.append((box((w + 0.08, d + 0.06, 0.06), Matrix.Translation(c + Vector((0, 0, -h / 2)))), brass))
    # Arrow slit and a brass spyglass hung beside it.
    parts.append((box((0.07, 0.03, 0.42), Matrix.Translation(c + Vector((0, d / 2 + 0.01, 0.12)))), (34, 30, 36)))
    parts.append((box((0.22, 0.03, 0.06), Matrix.Translation(c + Vector((0, d / 2 + 0.01, 0.16)))), (34, 30, 36)))
    sg = Matrix.Translation(c + Vector((0.34, d / 2 + 0.07, -0.25))) @ Matrix.Rotation(0.35, 4, "Y")
    parts.append((cone(0.06, 0.045, 0.44, segs=8, m=sg), brass))
    parts.append((cyl(0.07, 0.06, segs=8, m=sg @ Matrix.Translation((0, 0, 0.22))), (110, 190, 220)))
    # Straps over the shoulders.
    parts.append((torus(0.42, 0.04, Matrix.Translation(chest + Vector((0, 0.02, 0.05))) @ Matrix.Rotation(0.5, 4, "Y")), (96, 64, 44)))
    parts.append((torus(0.42, 0.04, Matrix.Translation(chest + Vector((0, 0.02, 0.05))) @ Matrix.Rotation(-0.5, 4, "Y")), (96, 64, 44)))
    ob = colored(parts, mat, sw, "Sig_Bastion")
    on_bone(ob, rig, "chest", Matrix())


def sig_temple_knight(rig, sw, mat):
    """A gold nimbus standing behind the head, crowned with candle flames and
    a keeper's star at the centre."""
    head = bone_line(rig, "head")[0]
    c = head + Vector((0, 0.66, 0.95))
    face = Matrix.Translation(c) @ Matrix.Rotation(math.pi / 2, 4, "X")
    flame_c, wax = (255, 200, 80), (240, 234, 214)
    parts = [
        (torus(0.78, 0.06, face, segs=24, sides=6), GOLD),
        # A pale disc of light inside the ring, set just behind it.
        (cyl(0.74, 0.02, segs=24, m=face @ Matrix.Translation((0, 0, -0.03))), (236, 206, 120)),
        (torus(0.56, 0.022, face @ Matrix.Translation((0, 0, 0.012)), segs=24, sides=5), GOLD_DK),
    ]
    # Short rays round the rim and a keeper's star in the middle.
    for k in range(16):
        a = 2 * math.pi * k / 16
        m = face @ Matrix.Rotation(a, 4, "Z") @ Matrix.Translation((0.88, 0, 0)) @ Matrix.Rotation(math.pi / 2, 4, "Y")
        if not (0.3 < a < 2.85):  # the candles stand on the upper arc
            parts.append((cone(0.045, 0.0, 0.16, segs=4, m=m @ Matrix.Diagonal((1, 0.5, 1, 1))), GOLD))
    for k in range(4):
        a = math.pi / 4 + k * math.pi / 2
        m = face @ Matrix.Rotation(a, 4, "Z") @ Matrix.Translation((0.14, 0, 0)) @ Matrix.Rotation(math.pi / 2, 4, "Y")
        parts.append((cone(0.08, 0.0, 0.28, segs=4, m=m @ Matrix.Diagonal((1, 0.4, 1, 1))), (150, 80, 190)))
    parts.append((sphere(0.1, Matrix.Translation(c)), (255, 226, 90)))
    # Candles on the upper arc.
    for k in range(5):
        a = math.radians(30 + 30 * k)
        p = c + Vector((0.78 * math.cos(a), 0, 0.78 * math.sin(a)))
        parts.append((cyl(0.055, 0.2, segs=8, m=Matrix.Translation(p + Vector((0, 0, 0.1)))), wax))
        parts.append((flame(0.05, Matrix.Translation(p + Vector((0, 0, 0.27)))), flame_c))
    # A bracket down to the collar.
    parts.append((box((0.06, 0.06, 0.8), Matrix.Translation(head + Vector((0, 0.66, 0.05)))), GOLD_DK))
    # Drawn at a comfortable size above, then shrunk about its centre so it
    # frames the head without hiding the body from the battle camera.
    shrink = Matrix.Translation(c) @ Matrix.Scale(0.7, 4) @ Matrix.Translation(-c)
    parts = [(transformed(bm, shrink), col) for bm, col in parts]
    ob = colored(parts, mat, sw, "Sig_TempleKnight")
    on_bone(ob, rig, "head", Matrix())


def sig_oracle(rig, sw, mat):
    """An all-seeing eye hovering over the left shoulder inside two brass
    rings, like a small orrery: the sentry's watch and the keeper's lore."""
    c = Vector((0.82, 0.1, 2.35))
    brass, brass_dk = (214, 168, 76), (150, 110, 50)
    look = Matrix.Translation(c) @ Matrix.Rotation(math.pi / 2, 4, "X")
    parts = [
        (sphere(0.22, Matrix.Translation(c), segs=16, rings=10), IVORY),
        (cyl(0.13, 0.06, segs=16, m=Matrix.Translation(c + Vector((0, -0.19, 0))) @ Matrix.Rotation(math.pi / 2, 4, "X")), (70, 150, 220)),
        (cyl(0.06, 0.07, segs=12, m=Matrix.Translation(c + Vector((0, -0.215, 0))) @ Matrix.Rotation(math.pi / 2, 4, "X")), (20, 20, 30)),
        (sphere(0.025, Matrix.Translation(c + Vector((0.04, -0.25, 0.04)))), WHITE),
        # Lids.
        (torus(0.2, 0.035, look @ Matrix.Translation((0, 0, 0.06)) @ Matrix.Diagonal((1, 0.75, 1, 1)), segs=16), (100, 60, 130)),
        (torus(0.34, 0.025, Matrix.Translation(c) @ Matrix.Rotation(0.5, 4, "X") @ Matrix.Rotation(0.3, 4, "Y"), segs=20), brass),
        (torus(0.42, 0.02, Matrix.Translation(c) @ Matrix.Rotation(-0.7, 4, "Y"), segs=20), brass_dk),
    ]
    # Two small moons on the rings and a tassel of runes below.
    parts.append((sphere(0.06, Matrix.Translation(c + Vector((0.34, 0, 0.0)) )), (150, 90, 200)))
    parts.append((sphere(0.05, Matrix.Translation(c + Vector((-0.3, 0.1, 0.28)))), (110, 190, 220)))
    for j in range(3):
        parts.append((box((0.06, 0.02, 0.09), Matrix.Translation(c + Vector((0, 0, -0.32 - 0.12 * j))) @ Matrix.Rotation(math.pi / 4, 4, "Y")), brass))
    ob = colored(parts, mat, sw, "Sig_Oracle")
    on_bone(ob, rig, "chest", Matrix())


PIECES = [sig_battle_priest, sig_spirit_ranger, sig_spellblade, sig_bastion, sig_temple_knight, sig_oracle]


def main():
    args = sys.argv[sys.argv.index("--") + 1:]
    out = args[0]
    here = os.path.dirname(os.path.abspath(__file__))
    rig = load(os.path.join(here, "..", "..", "assets", "adventurers", "Knight.glb"))
    # Keep the bare skeleton: no body, no weapons, no clips.
    for o in list(bpy.data.objects):
        if o.type != "ARMATURE":
            bpy.data.objects.remove(o, do_unlink=True)
    rig.animation_data_clear()
    for a in list(bpy.data.actions):
        bpy.data.actions.remove(a)
    rig.data.pose_position = "POSE"
    for pb in rig.pose.bones:
        pb.matrix_basis = Matrix()

    sw = Swatches()
    mat, tex = make_material()
    for make in PIECES:
        make(rig, sw, mat)
    tex.image = sw.material()

    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.export_scene.gltf(
        filepath=out,
        export_format="GLB",
        export_animations=False,
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
    print("BUILT", [f.__name__ for f in PIECES], len(sw.cells), "colours")
    if "--blend" in args:
        bpy.ops.wm.save_as_mainfile(filepath=args[args.index("--blend") + 1])


main()
