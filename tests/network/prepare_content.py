"""Content preparation now lives in the server: `pyrealium prepare-content` (emulator/src/content_prep.rs),
which `pyrealium run` also does by itself, storing the result in the database.

This module keeps the old entry points for the Python live tests: `prepare()` runs the Rust command
(PYREALIUM_BIN, else target/release/pyrealium under the repository), and the command line
matches the old script.
"""
import argparse
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/ac2'))
from dat_probe import Archive  # noqa: E402,F401  (re-exported for test_combat_recovery)


def binary():
    explicit = os.environ.get('PYREALIUM_BIN')
    if explicit:
        return Path(explicit)
    for name in ('pyrealium', 'pyrealium.exe'):
        path = ROOT / 'target/release' / name
        if path.exists():
            return path
    raise FileNotFoundError('Build the server first (Build-Pyrealium.ps1 or cargo build --release) or set PYREALIUM_BIN')


def prepare(directory, output, compare_directory=None, training_placements=True, shelter_quest=True):
    if compare_directory is not None:
        raise NotImplementedError('--compare-dat was not carried over to the Rust preparation')
    command = [str(binary()), 'prepare-content', str(directory), str(output),
               '--content', str(ROOT / 'content')]
    if not training_placements:
        command.append('--strip-training-placements')
    if not shelter_quest:
        command.append('--no-shelter-quest')
    subprocess.run(command, check=True)


if __name__ == '__main__':
    p = argparse.ArgumentParser()
    p.add_argument('dat_directory', type=Path)
    p.add_argument('output', type=Path)
    p.add_argument('--strip-training-placements', action='store_true')
    p.add_argument('--no-shelter-quest', action='store_true')
    a = p.parse_args()
    prepare(a.dat_directory, a.output, training_placements=not a.strip_training_placements,
            shelter_quest=not a.no_shelter_quest)
