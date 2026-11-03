"""Ensure published source offers contain build inputs and exclude local state."""
from pathlib import Path
import tempfile
import unittest
import zipfile

from export_source import export, SUPPORT_FILES


class SourceOfferTests(unittest.TestCase):
    def test_clean_checkout_and_unpacked_offer(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            server = workspace
            required = [
                'LICENSE', 'NOTICE', 'Build-Pyrealium.ps1', 'export_source.py', 'Recover-StaleLock.ps1',
                'Server-Binaries.ps1', 'Find-AccountService.ps1',
                'emulator/Cargo.toml', 'emulator/Cargo.lock', 'emulator/build.rs',
                'emulator/src/server.rs', 'emulator/content/world.tsv',
                'emulator/examples/population_ground.rs',
                'emulator/docs/architecture.md', 'emulator/tools/prepare.py',
                'emulator/web/accounts/index.html', 'deploy/Caddyfile',
                'deploy/online.env.example', 'LICENSES/third-party/example.txt',
            ]
            excluded = [
                'data/accounts.journal', 'data/characters.json', '.env',
                'emulator/target/test.exe', 'emulator/tools/__pycache__/test.pyc',
                'emulator/content/portal.dat', 'emulator/content/.env.local',
                'emulator/source/old.zip', 'emulator/web/accounts/private.key',
            ]
            for relative in required + excluded:
                path = server / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture', encoding='utf-8')
            for relative in SUPPORT_FILES:
                path = workspace / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture', encoding='utf-8')
            reference_files=['References/AC2_Rusted/Cargo.toml','References/AC2_Rusted/LICENSE',
                             'Network/Bridge/src/main.rs','Network/Bridge/src/capture_decode.rs',
                             'References/AC2_Rusted/crates/ac2-physics/Cargo.toml',
                             'References/AC2_Rusted/crates/ac2-physics/src/lib.rs']
            for relative in reference_files:
                path=workspace/relative;path.parent.mkdir(parents=True,exist_ok=True);path.write_text('fixture')
            output = export(server)
            with zipfile.ZipFile(output) as archive:
                names = set(archive.namelist())
                self.assertEqual(names, set(required) | set(SUPPORT_FILES) | set(reference_files))
                unpacked = workspace / 'unpacked'
                archive.extractall(unpacked)
            with zipfile.ZipFile(export(unpacked)) as archive:
                self.assertEqual(set(archive.namelist()), names)


if __name__ == '__main__':
    unittest.main()
