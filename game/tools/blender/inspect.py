"""Print what a character .glb holds: objects, bones, actions.

blender -b -P tools/blender/inspect.py -- assets/adventurers/Barbarian.glb
"""
import sys
import bpy

path = sys.argv[sys.argv.index("--") + 1]
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=path)
for o in bpy.data.objects:
    extra = ""
    if o.type == "MESH":
        extra = f"verts={len(o.data.vertices)} mats={[m.name for m in o.data.materials]} groups={len(o.vertex_groups)}"
    print(f"OBJ {o.name} type={o.type} parent={o.parent.name if o.parent else None} "
          f"ptype={o.parent_type} pbone={o.parent_bone} dims={tuple(round(d, 3) for d in o.dimensions)} {extra}")
arm = next(o for o in bpy.data.objects if o.type == "ARMATURE")
for b in arm.data.bones:
    print(f"BONE {b.name} parent={b.parent.name if b.parent else None} head={tuple(round(v, 3) for v in b.head_local)}")
for a in bpy.data.actions:
    print(f"ACTION {a.name} frames={tuple(a.frame_range)}")
for im in bpy.data.images:
    print(f"IMAGE {im.name} size={tuple(im.size)}")
