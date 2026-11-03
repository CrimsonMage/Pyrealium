"""Create the corresponding Pyrealium source archive served by the account backend."""
from pathlib import Path
import zipfile


# Everything needed to rebuild the server: the Rust workspace and the content, tools, page,
# docs, tests and deploy files beside it.
SOURCE_TREES = ('crates', 'content', 'tools', 'web', 'docs', 'tests', 'deploy', 'LICENSES')
SOURCE_SUFFIXES = {
    '.rs', '.toml', '.py', '.ps1', '.sh', '.bat', '.cmd', '.md', '.txt', '.tsv',
    '.json', '.html', '.css', '.js', '.service', '.slice', '.conf', '.example', '.sql', '.yml',
}
SOURCE_NAMES = {'LICENSE', 'NOTICE', 'Cargo.lock', 'Caddyfile', 'Dockerfile', '.gitignore', '.gitattributes'}
# `source` holds the generated archive itself (crates/pyrealium/source).
EXCLUDED_PARTS = {'target', 'dist', 'data', '__pycache__', '.git', 'source'}
OUTPUT = 'crates/pyrealium/source/Pyrealium-source.zip'


def source_files(root):
    """Work identically in a checkout and an unpacked source offer.

    Use only source directories, including current uncommitted edits. Never
    traverse an operator's stores or package game archives/build output.
    """
    root = root.resolve()
    candidates = list(root.iterdir())
    for tree in SOURCE_TREES:
        candidates.extend((root / tree).rglob('*'))
    for path in sorted(set(candidates)):
        relative = path.relative_to(root)
        if not path.is_file() or path.is_symlink() or path.resolve() != path:
            continue
        if any(part in EXCLUDED_PARTS or part.startswith('.env') for part in relative.parts):
            continue
        # An operator's settings (database password) beside the tree, never the deploy templates.
        if relative.as_posix() == 'pyrealium.toml':
            continue
        if path.name in SOURCE_NAMES or path.suffix.lower() in SOURCE_SUFFIXES:
            yield path, relative.as_posix()


def export(root):
    root = root.resolve()
    output = root / OUTPUT
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as archive:
        for path, name in source_files(root):
            archive.write(path, name)
    return output


if __name__ == '__main__':
    output = export(Path(__file__).resolve().parent)
    print(f'Pyrealium source offer: {output.name} ({output.stat().st_size} bytes)')
