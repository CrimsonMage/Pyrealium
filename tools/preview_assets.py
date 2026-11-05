"""Read-only visual/setup traversal for the editor's native DAT geometry preview.

VisualDesc::StreamPack 0063d7f0, PartGroupDataDesc::StreamPack 006443c0,
CharacterSetup::UnPack 006105a0. Unsupported appearance graphs fail explicitly.
Appearance hashes follow AC2_Rusted's ac2_visual_desc::part_group.
"""
from collections import OrderedDict
import math
from pathlib import Path
import struct

from ac2_dat import Dat


# it_portal_shard.vd and it_portal_shard_gate.vd (DID names), the world's portal visuals.
PORTAL_VISUALS = frozenset({0x1f00031e, 0x1f000d09})
_ORB = []


def portal_orb(radius=1.0, height=1.6, rings=10, segments=16):
    """Editor stand-in for a portal: a purple sphere centred `height` above the origin."""
    if _ORB:
        return _ORB[0]
    vertices, normals, indices = [], [], []
    for ring in range(rings + 1):
        theta = math.pi * ring / rings
        for segment in range(segments + 1):
            phi = 2 * math.pi * segment / segments
            n = (math.sin(theta) * math.cos(phi), math.sin(theta) * math.sin(phi), math.cos(theta))
            normals.append(list(n))
            vertices.append([n[0] * radius, n[1] * radius, n[2] * radius + height])
    for ring in range(rings):
        for segment in range(segments):
            a = ring * (segments + 1) + segment
            b = a + segments + 1
            indices += [a, a + 1, b, a + 1, b + 1, b]
    color = (170, 70, 235)
    _ORB.append(dict(did=None, vertices=vertices, normals=normals, indices=indices,
                     uvs=[[0, 0]] * len(vertices), groups=[dict(start=0, count=len(indices), material=0)],
                     materials=[0], material_images={}, material_colors={0: list(color)},
                     material_diffuse={0: [color[0] / 255, color[1] / 255, color[2] / 255, 1.]},
                     material_states={0: {}}, triangle_colors=[color] * (len(indices) // 3),
                     attached=True, portal_marker=True))
    return _ORB[0]


class Reader:
    def __init__(self, data):
        self.data, self.offset = data, 0

    def take(self, fmt):
        size = struct.calcsize("<" + fmt)
        if self.offset + size > len(self.data):
            raise ValueError("Truncated visual/setup record")
        values = struct.unpack_from("<" + fmt, self.data, self.offset)
        self.offset += size
        return values[0] if len(values) == 1 else values


def group(reader):
    flags = reader.take("I")
    if flags & ~0x1ff:
        raise ValueError("Unknown part-group field mask")
    result = {}
    for bit, name in ((1, "key"), (2, "parent"), (4, "connection"), (8, "setup"), (16, "animation")):
        if flags & bit:
            result[name] = reader.take("I")
    if flags & 0x20:
        # AppearanceInfo hash: table DID -> (key, scalar) pairs. It picks surfaces,
        # not geometry, so the preview keeps it without applying it.
        appearance = {}
        for unused in range(hash_count(reader)):
            table = reader.take("I")
            appearance[table] = [reader.take("If") for unused in range(hash_count(reader))]
        result["appearance"] = appearance
    if flags & 0x40:
        result["effects"] = reader.take("I")
    if flags & 0x80:
        result["startup_effects"] = [reader.take("If") for unused in range(hash_count(reader))]
    if flags & 0x100:
        raise ValueError("Part-group effect overrides are not supported yet")
    return result


def hash_count(reader):
    """PackableHashTable count: the low 16 bits (the high bits are the table size)."""
    count = reader.take("I") & 0xffff
    if count * 4 > len(reader.data) - reader.offset:
        raise ValueError("Hash count exceeds the record")
    return count


def visual_record(data, did):
    reader = Reader(data)
    flags = reader.take("I")
    if flags & ~0x7ff or not flags & 1 or reader.take("I") != did:
        raise ValueError("Unsupported visual description header")
    result = {"parent": 0, "groups": {}}
    for bit, name in ((2, "parent"), (4, "motion"), (8, "behavior"), (16, "modes")):
        if flags & bit:
            result[name] = reader.take("I")
    if flags & 0x20:
        scale = reader.take("3f")
        if any(not math.isfinite(v) or v <= 0 or v > 1000 for v in scale):
            raise ValueError("Invalid visual scale")
        result["scale"] = scale
    for bit in (0x40, 0x400):
        if flags & bit:
            reader.take("f")
    if flags & 0x80:
        # IconDesc::StreamPack: a count, then (layer id, image DID, shift colour) per layer.
        count = reader.take("I")
        if count > 4096 or count * 12 > len(data) - reader.offset:
            raise ValueError("Visual icon-layer count exceeds the record")
        result["icons"] = [reader.take("III") for unused in range(count)]
    if flags & 0x100:
        result["global"] = group(reader)
    if flags & 0x200:
        count = reader.take("I") & 0xffffff
        if count > 256:
            raise ValueError("Visual part-group capacity")
        for unused in range(count):
            key = reader.take("I")
            if key in result["groups"]:
                raise ValueError("Duplicate visual part group")
            result["groups"][key] = group(reader)
    if reader.offset != len(data):
        raise ValueError("Unsupported trailing visual fields")
    return result


def setup_meshes(data, did):
    reader = Reader(data)
    if reader.take("I") != did:
        raise ValueError("Setup identity mismatch")
    reader.take("I")  # Native setup flags precede the mesh DID array.
    count = reader.take("I")
    if not 1 <= count <= 256:
        raise ValueError("Unsupported setup mesh count")
    meshes = [reader.take("I") for unused in range(count)]
    if any(mesh >> 24 not in (1, 6) for mesh in meshes):
        raise ValueError("Unsupported setup mesh family")
    return meshes


def entity_visual(data, did):
    """Read EntityDesc's explicit data-file reference (00617370).

    Static room entities often have no workbook catalog entry, but their native
    definition explicitly names the visual DID. Only the bounded known prefix
    is consumed; trailing property metadata is irrelevant to that reference.
    """
    reader = Reader(data)
    flags = reader.take("I")
    if flags & ~0x1ff or not flags & 1 or reader.take("I") != did or did >> 24 != 0x47:
        raise ValueError("Unsupported static entity definition")
    if flags & 2:
        reader.take("I")
    if flags & 4:
        reader.take("Q")
    if not flags & 8:
        raise ValueError("Static definition has no explicit visual reference")
    visual = reader.take("I")
    if visual >> 24 != 0x1f:
        raise ValueError("Static definition does not reference a visual description")
    return visual


class Assets:
    """Owned by one loader thread; bounded decoded geometry cache."""
    def __init__(self, folder):
        self.folder = Path(folder)
        self.portal = Dat(self.folder / "portal.dat")
        try:
            self.cells = Dat(self.folder / "cell_1.dat")
        except Exception:
            self.portal.close()
            raise
        self.cache = OrderedDict()
        self.materials = OrderedDict()
        self.animations = OrderedDict()
        self.definitions = OrderedDict()
        self.material_warnings = set()
        try:
            self.surfaces = Dat(self.folder / "highres.dat") if (self.folder / "highres.dat").exists() else None
        except Exception:
            self.portal.close()
            self.cells.close()
            raise

    def close(self):
        self.portal.close()
        self.cells.close()
        if self.surfaces:
            self.surfaces.close()

    def definition_visual(self, did, catalog):
        if did >> 24 == 0x1f:
            return did
        if did in catalog:
            return catalog[did]
        if did not in self.definitions:
            self.definitions[did] = entity_visual(self.portal.read(did), did)
            if len(self.definitions) > 256:
                self.definitions.popitem(last=False)
        self.definitions.move_to_end(did)
        return self.definitions[did]

    def mesh(self, did):
        from preview_mesh import decode_mesh
        if did in self.cache:
            self.cache.move_to_end(did)
            return self.cache[did]
        value = decode_mesh(self.portal.read(did))
        self.color_mesh(value)
        self.cache[did] = value
        # A scene uses a few hundred meshes; holding about three areas' worth makes a
        # rebuild after a selection or edit reuse them instead of decoding again.
        if len(self.cache) > 384:
            self.cache.popitem(last=False)
        return value

    def color_mesh(self, mesh):
        from preview_material import material_info
        colors = [(130, 145, 155)] * (len(mesh["indices"]) // 3)
        mesh["material_images"] = {}
        mesh["material_colors"] = {}
        mesh["material_diffuse"] = {}
        mesh["material_states"] = {}
        hidden_materials = set()
        for group in mesh["groups"]:
            material = mesh["materials"][group["material"]]
            if material not in self.materials:
                try:
                    self.materials[material] = material_info(self.portal, material, self.surfaces, max_size=256)
                except (OSError, ValueError, KeyError) as error:
                    self.materials[material] = {"average": [130, 145, 155], "image_error": str(error)}
                if len(self.materials) > 256:
                    self.materials.popitem(last=False)
            info = self.materials[material]
            self.materials.move_to_end(material)
            mesh["material_colors"][group["material"]] = info.get("average", [130, 145, 155])
            mesh["material_diffuse"][group["material"]] = info.get("diffuse", [1.,1.,1.,1.])
            mesh["material_states"][group["material"]] = info.get("render_state", {})
            if not info.get("visible", True):
                hidden_materials.add(group["material"])
                continue
            image = info.get("image")
            if image:
                mesh["material_images"][group["material"]] = image
            if info.get("image_error"):
                self.material_warnings.add(f"Material {material:08x}: {info['image_error']}")
            if info.get("render_state_error"):
                self.material_warnings.add(f"Material state {material:08x}: {info['render_state_error']}")
            for offset in range(group["start"], group["start"] + group["count"], 3):
                color = info.get("average", [130, 145, 155])
                if image and mesh["uvs"]:
                    indices = mesh["indices"][offset:offset+3]
                    u, v = (sum(mesh["uvs"][index][axis] for index in indices)/3 for axis in range(2))
                    x, y = int((u % 1) * image["width"]), int((v % 1) * image["height"])
                    pixel = (y * image["width"] + x) * 3
                    color = tuple(image["rgb"][pixel:pixel+3])
                colors[offset//3] = color
        mesh["triangle_colors"] = colors
        if hidden_materials:
            # Keep render and CPU picking geometry identical. These are native
            # alpha-zero helper surfaces, not missing textures or usable floors.
            indices, groups, visible_colors = [], [], []
            for group in mesh["groups"]:
                if group["material"] in hidden_materials:
                    continue
                start, count = group["start"], group["count"]
                groups.append(dict(group, start=len(indices)))
                indices.extend(mesh["indices"][start:start+count])
                visible_colors.extend(colors[start//3:(start+count)//3])
            mesh["indices"], mesh["groups"] = indices, groups
            mesh["triangle_colors"] = visible_colors

    def visual(self, did, trail=()):
        if did in trail or len(trail) >= 16 or did >> 24 != 0x1f:
            raise ValueError("Cyclic, invalid or excessive visual inheritance")
        result = visual_record(self.portal.read(did), did)
        if result["parent"]:
            parent = self.visual(result["parent"], trail + (did,))
            parent["groups"].update(result["groups"])
            if "scale" in result:
                parent["scale"] = result["scale"]
            result = parent
        return result

    def creature(self, visual):
        """The visual's meshes; a portal visual (its swirl is an FX the preview does not
        draw) also gets a solid purple orb so it can be seen, selected and used."""
        if visual not in PORTAL_VISUALS:
            return self.parts(visual)
        try:
            meshes = self.parts(visual)
        except (OSError, ValueError, KeyError):
            meshes = []
        return meshes + [(portal_orb(), (1, 1, 1))]

    def parts(self, visual):
        """The visual's meshes, each part group placed at its parent's connection point.

        A mesh of an attached (non-root) group is a moved copy marked `attached`; the root
        group's meshes are the cached ones, so the preview's animation drives only those.
        """
        from editor_setup import decode_setup, place_groups, placed_mesh
        description = self.visual(visual)
        groups = list(description["groups"].values())
        if not any(group.get("setup", 0) >> 24 == 4 for group in groups):
            raise ValueError("Visual has no supported character setup")
        setups = {}
        def read_setup(did):
            if did not in setups:
                setups[did] = decode_setup(self.portal.read(did), did)
            return setups[did]
        scale = description.get("scale", (1, 1, 1))
        result = []
        for setup, root in place_groups([g for g in groups if g.get("setup", 0) >> 24 == 4], read_setup):
            for did in setup_meshes(self.portal.read(setup), setup):
                result.append((placed_mesh(self.mesh(did), root), scale))
        if not result:
            raise ValueError("Visual's part groups do not resolve to a root group")
        return result

    def animation(self, visual):
        from preview_animation import load_animation_set
        if visual in self.animations:
            self.animations.move_to_end(visual)
            return self.animations[visual]
        description = self.visual(visual)
        groups = list(description["groups"].values())
        if len(groups) != 1 or groups[0].get("animation", 0) >> 24 != 3:
            raise ValueError("Visual has no supported single-group animation map")
        animation = load_animation_set(self.portal, groups[0]["setup"], groups[0]["animation"])
        self.animations[visual] = animation
        if len(self.animations) > 4:
            self.animations.popitem(last=False)
        return animation

    def scene(self, cell, visual, position, heading, scale, placements, catalog, reference=(), budget=50000):
        """`reference`: read-only base-world rows (world-manifest fields), drawn nearest
        first within the triangle budget after everything else."""
        from preview_cell import decode_cell
        warnings, instances = [], []
        self.material_warnings.clear()
        triangle_count = 0

        def add(mesh, origin, quaternion, sizes, role):
            nonlocal triangle_count
            count = len(mesh["indices"]) // 3
            if triangle_count + count > budget:
                raise ValueError(f"Scene geometry exceeds the {budget:,}-triangle authoring budget")
            triangle_count += count
            instances.append(dict(mesh=mesh, position=origin, quaternion=quaternion,
                                  scale=sizes, role=role, placement_scale=scale or 1))

        # Reserve the selected model first so background limits never omit it.
        rotation = (math.cos(heading * math.pi / 255), 0, 0, -math.sin(heading * math.pi / 255))
        if visual:
            try:
                for mesh, native_scale in self.creature(visual):
                    add(mesh, position, rotation, [v * (scale or 1) for v in native_scale], "preview")
            except (OSError, ValueError, KeyError) as error:
                warnings.append(f"Selected visual {visual:08x}: {error}")
            try:
                animation = self.animation(visual)
                for instance in instances:
                    if instance["role"] == "preview" and not instance["mesh"].get("attached"):
                        try:
                            animation.prepare_mesh(instance["mesh"])
                            instance["animation_set"] = animation
                        except ValueError as error:
                            warnings.append(f"Animation mesh {instance['mesh']['did']:08x}: {error}")
                warnings.extend(animation.warnings)
            except (OSError, ValueError, KeyError) as error:
                warnings.append(f"Animation: {error}")
        loaded = set()
        static = []
        outdoor = 1 <= (cell & 65535) <= 64
        block_side = 0
        if outdoor:
            from preview_terrain import terrain_scene
            terrain = terrain_scene(self.cells, self.portal, cell, neighbors=1)
            block_side = terrain["definitions"]["block_side"]
            grid = {name: terrain["definitions"][name] for name in ("cell_side", "cells", "block_side")}
            for source in instances:
                source["outdoor_grid"] = grid
            for source in terrain["instances"]:
                self.color_mesh(source["mesh"])
                add(source["mesh"],source["position"],source["quaternion"],source["scale"],"world")
                instances[-1]["terrain"] = True
                instances[-1]["outdoor_grid"] = grid
            loaded.update(terrain["loaded_cells"])
            static.extend(terrain["placements"])
            warnings.extend(terrain["warnings"])
        elif 0x100 <= (cell & 65535) <= 0xfffb:
            current = decode_cell(self.cells.read(cell))
            if current["cell"] != cell:
                raise ValueError("Cell record identity mismatch")
            neighbors = dict.fromkeys([cell] + current["visible_cells"] + current["shared_cells"])
            for identity in list(neighbors)[:32]:
                if identity >> 16 != cell >> 16 or not 0x100 <= (identity & 65535) <= 0xfffb:
                    continue
                try:
                    room = current if identity == cell else decode_cell(self.cells.read(identity))
                    if room["cell"] != identity:
                        raise ValueError("Neighbor identity mismatch")
                    mesh = self.mesh(room["mesh"])
                    if mesh["indices"]:
                        add(mesh, room["position"], room["quaternion"], (1, 1, 1), "world")
                        instances[-1]["cell"] = identity
                    loaded.add(identity)
                    static.extend(dict(row, cell=identity) for row in room["placements"])
                    for note in room["warnings"]:
                        note = (note if note == "Cell property metadata is omitted from the geometry preview"
                                else f"Room {identity:08x}: {note}")
                        if note not in warnings:
                            warnings.append(note)
                except (OSError, ValueError, KeyError) as error:
                    warnings.append(f"Room {identity:08x}: {error}")
            if len(neighbors) > 32:
                warnings.append("Neighbor room preview is limited to 32 cells")
        else:
            warnings.append("Invalid or reserved cell identifier")
        # Static DAT placements are already in block space, unlike room meshes.
        static.sort(key=lambda row: sum((a-b)**2 for a,b in zip(row["position"], position)))
        missing = 0
        seen = set()
        for row in static[:64]:
            key = (row["definition"], *row["position"])
            if key in seen:
                continue
            seen.add(key)
            try:
                target = self.definition_visual(row["definition"], catalog)
                for mesh, sizes in self.creature(target):
                    add(mesh, row["position"], row["quaternion"], [a*b for a,b in zip(sizes,row["scale"])], "world")
                    instances[-1]["cell"] = row.get("cell")
            except (OSError, ValueError, KeyError):
                missing += 1
        if missing:
            warnings.append(f"{missing} nearby static props have unsupported or unmapped visuals")
        for row in placements[:16384]:
            if int(row[3], 16) not in loaded:
                continue
            try:
                for mesh, sizes in self.creature(int(row[2], 16)):
                    factor = float(row[11]) if len(row) == 12 and row[11] != "-" else 1
                    origin = [float(v) for v in row[4:7]]
                    if outdoor:
                        identity = int(row[3],16)
                        origin[0] += ((identity>>24)-(cell>>24))*block_side
                        origin[1] += (((identity>>16)&255)-((cell>>16)&255))*block_side
                    add(mesh, origin, [float(v) for v in row[7:11]],
                        [v * factor for v in sizes], "saved")
                    instances[-1]["cell"] = int(row[3], 16)
                    instances[-1]["object_id"] = int(row[0], 16)
            except (OSError, ValueError, KeyError) as error:
                warnings.append(f"Saved object {row[0]}: {error}")
        saved_ids = {int(row[0], 16) for row in placements}
        nearby = []
        for row in reference:
            try:
                identity = int(row[3], 16)
                if identity not in loaded or int(row[0], 16) in saved_ids:
                    continue
                origin = [float(v) for v in row[4:7]]
                if outdoor:
                    origin[0] += ((identity>>24)-(cell>>24))*block_side
                    origin[1] += (((identity>>16)&255)-((cell>>16)&255))*block_side
                nearby.append((sum((a-b)**2 for a, b in zip(origin, position)), origin, row))
            except (ValueError, IndexError):
                continue
        nearby.sort(key=lambda item: item[0])
        skipped = 0
        for distance, origin, row in nearby:
            try:
                factor = float(row[11]) if len(row) >= 12 and row[11] != "-" else 1
                try:
                    meshes = self.creature(int(row[2], 16))
                except (OSError, ValueError, KeyError):
                    # Captured visuals can come from another client build; the
                    # type's own visual in these DATs is the reliable fallback.
                    meshes = self.creature(self.definition_visual(int(row[1], 16), catalog))
                if triangle_count + sum(len(mesh["indices"]) // 3 for mesh, _ in meshes) > budget:
                    skipped += 1
                    continue
                for mesh, sizes in meshes:
                    add(mesh, origin, [float(v) for v in row[7:11]], [v * factor for v in sizes], "base")
                    instances[-1]["cell"] = int(row[3], 16)
                    instances[-1]["object_id"] = int(row[0], 16)
            except (OSError, ValueError, KeyError):
                skipped += 1
        if skipped:
            warnings.append(f"{skipped} base-world objects not drawn (unsupported visual or the triangle budget); fly closer to see them")
        warnings.extend(sorted(self.material_warnings)[:8])
        return instances, warnings
