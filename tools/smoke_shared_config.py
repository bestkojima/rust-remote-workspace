# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312; sys_platform == 'win32'"]
# ///
"""Two native workspaces share a device file; stale writes and trust prompts must fail closed."""
import argparse
import asyncio
from pathlib import Path
import socket
from tempfile import TemporaryDirectory

import asyncssh
import win32con as vk
from smoke_host_key import Keyboard, PROMPT, profile, public_line, server, stored_pin, trust_prompt, tui


async def run(exe, root):
    old, changed, client = (asyncssh.generate_private_key("ssh-ed25519") for _ in range(3))
    identity = root / "id"
    identity.write_bytes(client.export_private_key("openssh"))
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    config = root / "devices.toml"
    config.write_text(profile(port, identity, "Trust fixture") + "[settings]\ndefault_port=22\nconnection_timeout=10\n")
    who = client.convert_to_public()
    async with tui(exe, config) as stale:
        # This client has already seen a key, but its human has not approved it.
        async with server(changed, port, who, root) as (_, auth):
            await trust_prompt(stale, port, changed.get_fingerprint("sha256"))
            assert not auth

        # Independently verify a different server on the same endpoint in the other window.
        async with server(old, port, who, root), tui(exe, config) as active:
            await trust_prompt(active, port, old.get_fingerprint("sha256"))
            active.key(ord("Y"), "y")
            await active.wait("verified-shell-ready")
            active.key(vk.VK_F8)
            await active.wait("Connection List")
        saved = config.read_bytes()
        assert stored_pin(config) == public_line(old)
        # pywin32 can only attach a controller to one console at a time.
        stale.input = Keyboard(stale.proc).input
        async with server(changed, port, who, root) as (_, auth):
            stale.key(ord("Y"), "y")
            await stale.wait("Host key changed", old.get_fingerprint("sha256"), changed.get_fingerprint("sha256"))
            stale.key(vk.VK_ESCAPE)
            stale.key(vk.VK_RETURN, "\r")
            await stale.wait("Host key changed", old.get_fingerprint("sha256"), changed.get_fingerprint("sha256"),
                             absent=[PROMPT])
            assert not auth
            stale.key(vk.VK_ESCAPE)
            stale.key(vk.VK_F3)
            await stale.wait("Folder sync")
            stale.key(ord("R"), "r")
            await stale.wait("Sync tool")
            stale.key(vk.VK_RETURN, "\r")
            await stale.wait("Error", "Reopen the workspace")
            assert config.read_bytes() == saved
            stale.key(vk.VK_ESCAPE)
            stale.key(vk.VK_ESCAPE)
            stale.key(vk.VK_F4)
            await stale.wait("Connection List")
    print("Two workspaces: stale approval and reconnect blocked before authentication; stale settings write preserved original file.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=Path)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="rrw-cas-") as tmp, asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner:
        runner.run(run(args.exe.resolve(strict=True), Path(tmp)))
