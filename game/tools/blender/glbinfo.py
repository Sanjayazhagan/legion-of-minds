"""List a .glb's animations (with lengths in seconds), meshes, skins and the
largest vertex count per primitive (the game's loader needs < 65536).

blender -b -P tools/blender/glbinfo.py -- file.glb
"""
import json
import struct
import sys

path = sys.argv[sys.argv.index("--") + 1]
data = open(path, "rb").read()
n = struct.unpack_from("<I", data, 12)[0]
doc = json.loads(data[20:20 + n])
acc = doc["accessors"]
for a in doc.get("animations", []):
    t = max(acc[s["input"]].get("max", [0])[0] for s in a["samplers"])
    interp = {s.get("interpolation", "LINEAR") for s in a["samplers"]}
    print(f"ANIM {a.get('name')} len={t:.3f}s channels={len(a['channels'])} interp={sorted(interp)}")
for m in doc.get("meshes", []):
    v = max(acc[p["attributes"]["POSITION"]]["count"] for p in m["primitives"])
    print(f"MESH {m.get('name')} prims={len(m['primitives'])} maxverts={v}")
print(f"SKINS {len(doc.get('skins', []))} NODES {len(doc['nodes'])} IMAGES {len(doc.get('images', []))}")
