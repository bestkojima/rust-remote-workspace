# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312", "psutil==7.1.3"]
# ///
"""Windows release benchmark: 64 unchanged source files and one new 8 MiB payload."""
import argparse
import asyncio
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import time

import asyncssh
import win32con as vk

from measure_workspace import measure_working_set
from smoke_host_key import profile, public_line, server, tui
from smoke_sync import enter, key


async def completed(ui, marker):
    until = time.monotonic() + 120
    while ui.proc.poll() is None and time.monotonic() < until:
        screen = ui.screen()
        if marker in screen:
            return screen
        if "Failed" in screen or "Error" in screen:
            raise AssertionError(screen)
        await asyncio.sleep(.06)
    raise AssertionError(f"Timed out waiting for {marker}: {ui.screen()}")


async def run(exe, rclone, root, snapshots=None):
    def snapshot(name, ui):
        if snapshots:
            snapshots.mkdir(parents=True, exist_ok=True)
            (snapshots / (name + ".txt")).write_text(ui.screen(), encoding="utf-8")
    local, remote = root / "project", root / "target"
    for directory in [local, remote]:
        directory.mkdir()
        for i in range(64):
            (directory / f"module-{i:03}.txt").write_bytes(bytes([i]) * 4096)
    payload = bytes(range(256)) * 32768
    (local / "asset.bin").write_bytes(payload)
    (remote / "sentinel").write_bytes(b"keep")
    host, identity = [asyncssh.generate_private_key("ssh-ed25519") for _ in range(2)]
    keyfile = root / "key"
    keyfile.write_bytes(identity.export_private_key())
    config = root / "test.toml"
    async with server(host, 0, identity.convert_to_public(), remote) as (port, _):
        config.write_text(profile(port, keyfile, "bench", public_line(host)) + f"""
[settings]
default_port=22
connection_timeout=10
rclone_path={json.dumps(str(rclone))}
[[sync_profiles]]
id="load"
name="Release workload"
device_id="bench"
local_root={json.dumps(str(local))}
remote_root="/"
direction="Upload"
excludes=[]
""",encoding="utf8")
        async with tui(exe, config, "bench", cwd=local) as ui:
            ui.key(vk.VK_F3)
            await ui.wait("Release workload")
            enter(ui)
            await ui.wait("Authorize remote directory")
            snapshot("remote-scope", ui)
            async with measure_working_set(ui.proc.pid, "Scan: 64 x 4 KiB unchanged + 1 x 8 MiB new"):
                key(ui, "e")  # this ephemeral SFTP root is exclusive for this run
                screen = await completed(ui, "Copy preview")
                assert "asset.bin" in screen and not (remote / "asset.bin").exists()
                snapshot("copy-preview", ui)
            enter(ui)
            await ui.wait("Confirm copy", "Files: 1", "Replacements: 0")
            async with measure_working_set(ui.proc.pid, "Transfer: one 8 MiB file"):
                key(ui,"y")
                await completed(ui,"Finished")
            assert (remote / "asset.bin").read_bytes()==payload
            assert (remote / "sentinel").read_bytes()==b"keep"
            enter(ui)
            ui.key(vk.VK_F4)
            await ui.wait("Connection List")


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=Path)
    parser.add_argument("rclone", type=Path)
    parser.add_argument("--snapshots",type=Path,help="save the fixture's actual console views")
    args=parser.parse_args()
    with TemporaryDirectory(prefix="rrw-load-") as tmp, asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner:
        runner.run(run(args.exe.resolve(strict=True),args.rclone.resolve(strict=True),Path(tmp),args.snapshots))
