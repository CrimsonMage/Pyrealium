"""Ensure published source offers contain build inputs and exclude local state."""
from pathlib import Path
import tempfile
import unittest
import zipfile

from export_source import export


class SourceOfferTests(unittest.TestCase):
    def test_clean_checkout_and_unpacked_offer(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            server = workspace
            required = [
                'LICENSE', 'NOTICE', 'Build-Pyrealium.ps1', 'export_source.py', 'Start-Pyrealium.ps1',
                'Cargo.toml', 'Cargo.lock',
                'crates/pyrealium/Cargo.toml', 'crates/pyrealium/build.rs',
                'crates/pyrealium/migrations/0001_initial.sql', 'crates/pyrealium/src/server.rs',
                'crates/pyrealium/examples/population_ground.rs',
                'crates/ac2-physics/Cargo.toml', 'crates/ac2-physics/src/lib.rs',
                'crates/ac2-network-bridge/src/main.rs',
                'content/world.tsv', 'docs/architecture.md', 'tools/prepare.py', 'tools/ac2/dat_probe.py',
                'tests/network/test_network.py', 'web/accounts/index.html', 'deploy/Caddyfile',
                'deploy/pyrealium.service', 'deploy/docker/Dockerfile', 'deploy/docker/compose.yml',
                'deploy/docker/pyrealium.toml', 'LICENSES/third-party/example.txt',
            ]
            excluded = [
                'data/accounts.journal', 'data/characters.json', '.env', 'pyrealium.toml',
                'target/release/pyrealium.exe', 'crates/pyrealium/target/test.exe',
                'tools/__pycache__/test.pyc', 'content/portal.dat', 'content/.env.local',
                'crates/pyrealium/source/old.zip', 'web/accounts/private.key',
            ]
            for relative in required + excluded:
                path = server / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture', encoding='utf-8')
            output = export(server)
            with zipfile.ZipFile(output) as archive:
                names = set(archive.namelist())
                self.assertEqual(names, set(required))
                unpacked = workspace / 'unpacked'
                archive.extractall(unpacked)
            with zipfile.ZipFile(export(unpacked)) as archive:
                self.assertEqual(set(archive.namelist()), names)


if __name__ == '__main__':
    unittest.main()
