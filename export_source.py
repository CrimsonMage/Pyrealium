"""Create the corresponding Pyrealium source archive served by the account backend."""
from pathlib import Path
import zipfile


SOURCE_TREES = (
    'emulator/src', 'emulator/tests', 'emulator/benches', 'emulator/content',
    'emulator/tools', 'emulator/web', 'emulator/docs', 'emulator/examples', 'deploy', 'LICENSES',
)
SOURCE_SUFFIXES = {
    '.rs', '.toml', '.py', '.ps1', '.sh', '.bat', '.cmd', '.md', '.txt', '.tsv',
    '.json', '.html', '.css', '.js', '.service', '.slice', '.conf', '.example',
}
SOURCE_NAMES = {'LICENSE', 'NOTICE', 'Cargo.lock', 'Caddyfile', '.gitignore', '.gitattributes'}
EXCLUDED_PARTS = {'target', 'dist', 'data', '__pycache__', '.git', 'source'}
SUPPORT_FILES = ('Network/prepare_content.py', 'Network/content_profile.py',
                 'Network/Bridge/Cargo.toml', 'Network/Bridge/Cargo.lock',
                 'Tools/AC2/dat_probe.py', 'Tools/AC2/population_probe.py',
                 'Docs/CAPTURE_REFERENCE_IMPORT.md')


def source_files(root):
    """Work identically in a monorepo and an unpacked source offer.

    Use only source directories, including current uncommitted edits. Never
    traverse an operator's stores or package game archives/build output.
    """
    root = root.resolve()
    candidates = list(root.iterdir()) + list((root / 'emulator').iterdir())
    for tree in SOURCE_TREES:
        candidates.extend((root / tree).rglob('*'))
    for path in sorted(set(candidates)):
        relative = path.relative_to(root)
        if not path.is_file() or path.is_symlink() or path.resolve() != path:
            continue
        if any(part in EXCLUDED_PARTS or part.startswith('.env') for part in relative.parts):
            continue
        if path.name in SOURCE_NAMES or path.suffix.lower() in SOURCE_SUFFIXES:
            yield path, relative.as_posix()


def export(root):
    root = root.resolve()
    output = root / 'emulator/source/Pyrealium-source.zip'
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as archive:
        for path, name in source_files(root):
            archive.write(path, name)
        for relative in SUPPORT_FILES:
            archive.write(root / relative, relative)
        # The optional capture audit tools use this native protocol adapter.
        bridge=root/'Network/Bridge/src'
        for path in sorted(bridge.rglob('*.rs')):
            if path.is_file() and not path.is_symlink():
                archive.write(path,path.relative_to(root).as_posix())
        # Native collision dependencies are pinned local source, not a binary SDK.
        # Preserve their workspace layout so the offered server rebuilds offline
        # from this tree after crates.io dependencies have been installed.
        reference = root / 'References/AC2_Rusted'
        if reference.is_dir():
            for path in sorted(reference.rglob('*')):
                relative = path.relative_to(reference)
                if not path.is_file() or path.is_symlink():
                    continue
                if any(part in EXCLUDED_PARTS or part in {'Saved', 'decompiled-pdb'} for part in relative.parts):
                    continue
                if relative.parts[0] not in {'crates', 'tools'} and len(relative.parts) != 1:
                    continue
                if path.suffix.lower() in SOURCE_SUFFIXES | {'.wgsl', '.h', '.c', '.ron'} or path.name in SOURCE_NAMES:
                    archive.write(path, 'References/AC2_Rusted/' + relative.as_posix())
    return output


if __name__ == '__main__':
    output = export(Path(__file__).resolve().parent)
    print(f'Pyrealium source offer: {output.name} ({output.stat().st_size} bytes)')
