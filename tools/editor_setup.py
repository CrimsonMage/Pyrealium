"""CSetup bones, rest pose and connection points, and multi-part visual assembly.

Read in `CharacterSetup::UnPack` order: the record's DID, flags, mesh DIDs, bones (parent
index, name id), placement frames of per-bone `TransformBase`s (offset, per-axis scale, then
the rotation as w, x, y, z), then the holding locations and connection points (id, bone index,
`TransformBase`). The rest of the record (cylspheres, spheres, physics, degrade info, bounds)
is not needed to place parts, so it is not read.

A visual's part groups attach as the client composes them (AC2_Rusted
`ac2_static_model::place_parts`): a group without a parent sits at the model's origin; a child
sits at its parent's root, composed with the parent Setup's rest-pose bone named by the
connection point and that point's own transform. A point the parent Setup lacks leaves the
child at its parent's root; a group whose parent never resolves is not drawn.
"""
import math
import struct

MAX_ITEMS = 4096
IDENTITY = ((0.0, 0.0, 0.0), (1.0, 1.0, 1.0), (1.0, 0.0, 0.0, 0.0))


class SetupFormatError(ValueError):
    pass


class _Reader:
    def __init__(self, data):
        self.data, self.at = data, 0

    def take(self, fmt):
        size = struct.calcsize("<" + fmt)
        if self.at + size > len(self.data):
            raise SetupFormatError("Setup record ended early")
        values = struct.unpack_from("<" + fmt, self.data, self.at)
        self.at += size
        return values if len(values) > 1 else values[0]

    def count(self, item_bytes):
        count = self.take("I")
        if count > MAX_ITEMS or count * item_bytes > len(self.data) - self.at:
            raise SetupFormatError("Setup item count exceeds the record")
        return count


def _normalized(q):
    length = math.sqrt(sum(v * v for v in q))
    return tuple(v / length for v in q) if length > 0 else (1.0, 0.0, 0.0, 0.0)


def read_transform(reader):
    offset = reader.take("3f")
    scale = reader.take("3f")
    w, x, y, z = reader.take("4f")
    return (tuple(offset), tuple(scale), _normalized((w, x, y, z)))


def decode_setup(data, did):
    """Meshes, bones, the first placement frame and the connection points of a Setup."""
    reader = _Reader(data)
    if reader.take("I") != did:
        raise SetupFormatError("Setup identity mismatch")
    reader.take("I")  # flags
    meshes = [reader.take("I") for unused in range(reader.count(4))]
    bones = []
    for unused in range(reader.count(8)):
        parent, name = reader.take("II")
        bones.append(None if parent == 0xFFFFFFFF else parent)
    frames = []
    for unused in range(reader.count(8)):
        reader.take("I")  # the frame's Placement kind
        frames.append([read_transform(reader) for unused in range(reader.count(40))])
    locations = []
    for unused in range(2):
        points = {}
        for unused in range(reader.count(48)):
            point_id, bone = reader.take("II")
            points.setdefault(point_id, (bone, read_transform(reader)))
        locations.append(points)
    return dict(meshes=meshes, bones=bones, rest=frames[0] if frames else [],
                holding=locations[0], connections=locations[1])


def quat_multiply(a, b):
    aw, ax, ay, az = a
    bw, bx, by, bz = b
    return (aw*bw - ax*bx - ay*by - az*bz, aw*bx + ax*bw + ay*bz - az*by,
            aw*by - ax*bz + ay*bw + az*bx, aw*bz + ax*by - ay*bx + az*bw)


def rotate(q, v):
    w, x, y, z = q
    # v + 2w(q x v) + 2 q x (q x v)
    cx, cy, cz = y*v[2] - z*v[1], z*v[0] - x*v[2], x*v[1] - y*v[0]
    dx, dy, dz = y*cz - z*cy, z*cx - x*cz, x*cy - y*cx
    return (v[0] + 2*(w*cx + dx), v[1] + 2*(w*cy + dy), v[2] + 2*(w*cz + dz))


def apply(transform, point):
    offset, scale, rotation = transform
    turned = rotate(rotation, (point[0]*scale[0], point[1]*scale[1], point[2]*scale[2]))
    return (offset[0] + turned[0], offset[1] + turned[1], offset[2] + turned[2])


def compose(outer, inner):
    """`outer` applied after `inner` (`Ac2Transform::compose`)."""
    return (apply(outer, inner[0]), tuple(outer[1][i] * inner[1][i] for i in range(3)),
            _normalized(quat_multiply(outer[2], inner[2])))


def rest_pose(setup):
    """Model-space transform of every bone in the first placement frame."""
    world = []
    for index, parent in enumerate(setup["bones"]):
        own = setup["rest"][index] if index < len(setup["rest"]) else IDENTITY
        world.append(compose(world[parent], own) if parent is not None and parent < index else own)
    return world


def place_groups(groups, read_setup):
    """[(setup DID, root transform)] for a visual's part groups, parents before children.

    `groups`: decoded part groups (dicts with key, parent, connection, setup);
    `read_setup(did)` returns `decode_setup`'s result for a Setup DID.
    """
    placed = []  # (key, setup, root)
    pending = [group for group in groups if group.get("setup", 0)]
    while pending:
        before = len(pending)
        for group in list(pending):
            parent_key = group.get("parent", 0)
            if parent_key == 0:
                root = IDENTITY
            else:
                parent = next((item for item in placed if item[0] == parent_key), None)
                if parent is None:
                    continue
                root = parent[2]
                try:
                    setup = read_setup(parent[1])
                    point = setup["connections"].get(group.get("connection", 0))
                    pose = rest_pose(setup)
                    if point is not None and point[0] < len(pose):
                        root = compose(compose(parent[2], pose[point[0]]), point[1])
                except (SetupFormatError, KeyError):
                    pass
            placed.append((group.get("key", 0), group["setup"], root))
            pending.remove(group)
        if len(pending) == before:
            break  # groups whose parent never resolves are not drawn
    return [(setup, root) for unused, setup, root in placed]


def placed_mesh(mesh, transform):
    """A copy of `mesh` with its vertices and normals moved by `transform`."""
    if transform == IDENTITY:
        return mesh
    copy = dict(mesh)
    copy["vertices"] = [list(apply(transform, vertex)) for vertex in mesh.get("vertices", ())]
    if mesh.get("normals"):
        copy["normals"] = [list(rotate(transform[2], normal)) for normal in mesh["normals"]]
    copy["attached"] = True
    points = copy["vertices"]
    if points:
        copy["bounds"] = [[min(p[i] for p in points) for i in range(3)], [max(p[i] for p in points) for i in range(3)]]
    return copy
