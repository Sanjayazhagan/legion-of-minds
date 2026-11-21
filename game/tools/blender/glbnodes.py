"""Compare the rest transforms of named nodes in two .glb files, without bpy.

Used to check that a Blender round trip keeps the hand slots (where armoury
weapons attach) exactly where they were.

blender -b --python-exit-code 1 -P tools/blender/glbnodes.py -- old.glb new.glb handslot.r handslot.l chest head
"""
import json
import struct
import sys

import mathutils as mu


def load(path):
    data = open(path, "rb").read()
    n = struct.unpack_from("<I", data, 12)[0]
    return json.loads(data[20:20 + n])


def globals_of(doc):
    nodes = doc["nodes"]
    parent = {}
    for i, n in enumerate(nodes):
        for c in n.get("children", []):
            parent[c] = i

    def local(n):
        if "matrix" in n:
            m = n["matrix"]
            return mu.Matrix([m[0:4], m[4:8], m[8:12], m[12:16]]).transposed()
        t = mu.Vector(n.get("translation", [0, 0, 0]))
        x, y, z, w = n.get("rotation", [0, 0, 0, 1])
        r = mu.Quaternion((w, x, y, z))
        s = n.get("scale", [1, 1, 1])
        return mu.Matrix.LocRotScale(t, r, mu.Vector(s))

    out = {}
    for i, n in enumerate(nodes):
        m = local(n)
        p = parent.get(i)
        while p is not None:
            m = local(nodes[p]) @ m
            p = parent.get(p)
        out[n.get("name", "")] = m
    return out


args = sys.argv[sys.argv.index("--") + 1:]
a, b, names = globals_of(load(args[0])), globals_of(load(args[1])), args[2:]
worst = 0.0
for name in names:
    ma, mb = a[name], b[name]
    d = max(abs(ma[r][c] - mb[r][c]) for r in range(4) for c in range(4))
    worst = max(worst, d)
    print(f"NODE {name} maxdiff={d:.5f}")
print(f"WORST {worst:.5f}")
sys.exit(0 if worst < 1e-3 else 1)
