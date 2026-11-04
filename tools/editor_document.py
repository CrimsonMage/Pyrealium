"""Bounded, conflict-checked offline editor transactions and reversible history."""
from collections import Counter
from pathlib import Path
from types import SimpleNamespace
import math
import os
import tempfile

import editor_content
from place_content import bounded_text, place


def read_document(path):
    """The world's placements as world text: its content database's builder rows (NAME.tsv names
    the world; NAME.content.sqlite holds it)."""
    return editor_content.world_text(path)


def rows_from_text(text):
    lines = (text if text is not None else "pyrealium-world-v2\n").splitlines()
    if not lines or lines[0] not in ("pyrealium-world-v2", "pyrealium-world-v3"):
        raise ValueError("Choose a world-v2 or world-v3 content file")
    size = 11 if lines[0].endswith("2") else 12
    rows = []
    identities = set()
    cells = Counter()
    for number, line in enumerate(lines[1:], 2):
        if not line or line.startswith("#"):
            continue
        try:
            row = line.split("\t")
            if len(row) != size or len(line.encode("utf-8")) > 512:
                raise ValueError("invalid placement shape")
            identity, entity, visual, cell = (int(value, 16) for value in row[:4])
            if identity in identities or identity >> 48 != 0x8140 or identity == 0x8140000000000000:
                raise ValueError("duplicate or invalid object identity")
            if entity >> 24 != 0x47 or visual >> 24 != 0x1f:
                raise ValueError("invalid asset identifier")
            low = cell & 65535
            if not 0 < cell <= 0xffffffff or not (1 <= low <= 64 or 256 <= low <= 65531):
                raise ValueError("invalid cell identifier")
            if any(not math.isfinite(float(v)) or abs(float(v)) >= 1_000_000 for v in row[4:7]):
                raise ValueError("invalid position")
            q = [float(v) for v in row[7:11]]
            if not all(math.isfinite(v) for v in q) or abs(sum(v*v for v in q)-1) > .01:
                raise ValueError("invalid orientation")
            if size == 12 and row[11] != "-":
                if not math.isfinite(float(row[11])) or not 0 < float(row[11]) <= 100:
                    raise ValueError("invalid scale")
            identities.add(identity)
            cells[cell] += 1
            rows.append(row)
        except (ValueError, OverflowError) as error:
            raise ValueError(f"Line {number}: {error}") from error
    if len(rows) > 16384 or any(count > 64 for count in cells.values()):
        raise ValueError("World/cell content limit exceeded")
    return rows


def atomic_write(path, text):
    descriptor, name = tempfile.mkstemp(prefix=path.name + ".", suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as file:
            file.write(text)
            file.flush()
            os.fsync(file.fileno())
        Path(name).replace(path)
    finally:
        Path(name).unlink(missing_ok=True)


def commit_document(path, expected, result):
    """Writes the placements into the world's content database (its builder rows), under the CLI
    writer's lock; the dump before the save is kept as NAME.content.sql.bak."""
    path = Path(path)
    rows_from_text(result)
    if len(result.encode("utf-8")) > 4*1024*1024:
        raise ValueError("Content exceeds 4 MiB")
    path.parent.mkdir(parents=True, exist_ok=True)
    lock = path.with_name(path.name + ".edit.lock")
    with lock.open("x", encoding="utf-8") as guard:
        try:
            guard.write(str(os.getpid()))
            guard.flush()
            if read_document(path) != expected:
                raise ValueError("This world changed outside the editor. Reload it before saving.")
            dump = editor_content.content_path(path)
            if dump.exists():
                atomic_write(dump.with_name(dump.name + ".bak"), dump.read_text(encoding="utf-8"))
            editor_content.write_world(path, result)
        finally:
            guard.close()
            lock.unlink()


def prepare_change(args, original, identity=None, delete=False, preserve_orientation=False):
    """Validate additions/updates through the existing native DAT authoring path."""
    rows = rows_from_text(original)
    target = next((row for row in rows if int(row[0], 16) == identity), None)
    if identity is not None and target is None:
        raise ValueError("Selected object no longer exists")
    lines = (original if original is not None else "pyrealium-world-v2\n").splitlines()
    target_index = next((i for i, line in enumerate(lines) if target and line == "\t".join(target)), None)
    if target_index is not None:
        lines.pop(target_index)
    if delete:
        if target is None:
            raise ValueError("Select a saved placement to delete")
        return "\n".join(lines) + "\n", identity
    with tempfile.TemporaryDirectory(prefix="pyrealium-editor-") as folder:
        staged = Path(folder) / "world.tsv"
        staged.write_text("\n".join(lines) + "\n", encoding="utf-8")
        staged_args = SimpleNamespace(**vars(args))
        staged_args.output = staged
        new_id = place(staged_args)
        result = bounded_text(staged)
    if target is None:
        return result, new_id
    new_row = rows_from_text(result)[-1]
    new_row[0] = target[0]
    if preserve_orientation:
        new_row[7:11] = target[7:11]
    # Keep the original ordering and comments; only change this object's row.
    result_lines = result.splitlines()
    header = result_lines[0]
    original_lines = original.splitlines()
    if header != original_lines[0]:
        original_lines = [header] + [
            line + "\t-" if line and not line.startswith("#") else line
            for line in original_lines[1:]]
    original_lines[target_index] = "\t".join(new_row)
    return "\n".join(original_lines) + "\n", identity


def heading_from_row(row):
    return round((math.atan2(-float(row[10]), float(row[7])) * 255 / math.pi) % 255)


def adopt_row(original, raw, label=None):
    """The working copy with a prepared-world object copied in under its own identity.

    The server lets a content-file object with a prepared-world identity replace that
    object, so the copy is an override: moving, turning or rescaling it moves the
    prepared object, and deleting it puts the prepared object back. `raw` is the
    prepared row's first 12 fields (identity ... quaternion, scale or "-").
    """
    text = original if original is not None else "pyrealium-world-v2\n"
    lines = text.splitlines()
    identity = raw[0].lower()
    if any(row[0].lower() == identity for row in rows_from_text(text)):
        return text
    fields = list(raw[:11])
    scale = raw[11] if len(raw) >= 12 else "-"
    header = lines[0]
    if scale != "-" and header == "pyrealium-world-v2":
        # Scale needs the v3 layout; existing rows keep the native size.
        lines = ["pyrealium-world-v3"] + [line + "\t-" if line and not line.startswith("#") else line for line in lines[1:]]
        header = lines[0]
    if header == "pyrealium-world-v3":
        fields.append(scale)
    lines.append("# Prepared-world override" + (f": {label}" if label else ""))
    lines.append("\t".join(fields))
    return "\n".join(lines) + "\n"


def drop_unchanged_overrides(text, base):
    """Remove override rows identical to the prepared object they copy (and their comment)."""
    lines = text.splitlines()
    keep = []
    for index, line in enumerate(lines):
        if line and not line.startswith("#"):
            fields = line.split("\t")
            prepared = base.get(fields[0].lower())
            if prepared is not None and _same_row(fields, prepared):
                if keep and keep[-1].startswith("# Prepared-world override"):
                    keep.pop()
                continue
        keep.append(line)
    return "\n".join(keep) + "\n"


def _same_row(fields, prepared):
    if [value.lower() for value in fields[:4]] != [value.lower() for value in prepared[:4]]:
        return False
    try:
        if any(abs(float(a) - float(b)) > 1e-4 for a, b in zip(fields[4:11], prepared[4:11])):
            return False
    except ValueError:
        return False
    mine = fields[11] if len(fields) >= 12 else "-"
    theirs = prepared[11] if len(prepared) >= 12 else "-"
    return mine == theirs or (mine != "-" and theirs != "-" and abs(float(mine) - float(theirs)) <= 1e-4)
