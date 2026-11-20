# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312"]
# ///
"""Windows: save a throwaway SSH password, restart the TUI, and log in on loopback."""
import asyncio
import base64
import os
from pathlib import Path
import secrets
import sys
from tempfile import TemporaryDirectory
import tomllib

import asyncssh
import win32con as vk
import win32crypt
import win32file

from smoke_host_key import PROMPT, public_line, tui
from smoke_settings import ctrl, write


async def run(exe, root):
    host = asyncssh.generate_private_key("ssh-ed25519")
    password = " " + secrets.token_hex(14) + " "  # leading and trailing spaces are significant
    config = root / "devices.toml"
    config.write_text('connections=[]\n[settings]\ndefault_port=22\nconnection_timeout=10\nstartup_log="debug"\n',
                      encoding="utf-8")
    attempts = []

    class PasswordOnly(asyncssh.SSHServer):
        def password_auth_supported(self):
            return True

        def validate_password(self, username, supplied):
            matches = username == "fixture" and secrets.compare_digest(password, supplied)
            attempts.append(matches)
            return matches

    async def echo(process):
        process.stdout.write("password-login-ready\n")
        async for line in process.stdin:
            process.stdout.write(f"echo:{line.strip()}\n")

    async with asyncssh.listen("127.0.0.1", 0, config=None, server_factory=PasswordOnly,
                               server_host_keys=[host], process_factory=echo) as server:
        async with tui(exe, config, "No saved SSH devices") as ui:
            ui.key(ord("N"), "n")
            await ui.wait("New SSH Connection")
            for value in ("127.0.0.1", str(server.get_port()), "fixture", password, ""):
                write(ui, value)
                ui.key(vk.VK_TAB)
            write(ui, "pw fixture")
            ctrl(ui, "s")
            await ui.wait("Connection List", "pw fixture", absent=["New SSH Connection"])
        data = config.read_text("utf-8")
        record, = tomllib.loads(data)["connections"]
        envelope = record["auth_method"]["password"]
        assert password.strip() not in data and not attempts and "public_key" not in record
        # Legacy TermiRs blobs are unprefixed Base64. A fresh Windows save must use DPAPI.
        assert envelope.startswith("dpapi-v1:"), "not a DPAPI envelope"
        blob = base64.b64decode(envelope.split(":", 1)[1], validate=True)
        _, fixture_bytes = win32crypt.CryptUnprotectData(blob, None, None, None, 1)
        assert fixture_bytes == password.encode()

        async with tui(exe, config, "pw fixture") as ui:
            ui.key(ord("D"), "d")
            await ui.wait("Delete Connection", "pw fixture")
            before = config.read_bytes()
            # Deny writes to this fixture only. A rejected deletion must not become a pending edit.
            handle = win32file.CreateFile(str(config), vk.GENERIC_READ, vk.FILE_SHARE_READ, None,
                                          vk.OPEN_EXISTING, 0, None)
            try:
                ui.key(ord("Y"), "y")
                await ui.wait("Error")
            finally:
                handle.Close()
            assert config.read_bytes() == before
            ui.key(vk.VK_ESCAPE)
            await ui.wait("Connection List", "pw fixture", absent=["Error"])
            ui.key(vk.VK_RETURN)
            await ui.wait(PROMPT, host.get_fingerprint("sha256"))
            assert not attempts
            ui.key(ord("Y"), "y")
            await ui.wait("password-login-ready")
            ui.key(vk.VK_F8)
            await ui.wait("Connection List", "pw fixture")
            ui.key(ord("E"), "e")
            await ui.wait("Edit SSH Connection")
            ui.key(vk.VK_RETURN)
            await ui.wait("Connection List", absent=["Edit SSH Connection"])

        async with tui(exe, config, "pw fixture") as ui:
            ui.key(vk.VK_RETURN)
            await ui.wait("password-login-ready", absent=[PROMPT])
            ui.text("restarted")
            await ui.wait("echo:restarted")
            ui.key(vk.VK_F8)
            await ui.wait("Connection List")
        assert attempts == [True, True]

    saved = tomllib.loads(config.read_text("utf-8"))["connections"][0]
    assert saved["public_key"] == public_line(host)
    log = (root / "termirs.log").read_text("utf-8")
    assert "Logging enabled at level: debug" in log
    for artifact in (root / "termirs.log", root / "termirs.log.1"):
        if artifact.exists():
            content = artifact.read_bytes()
            assert all(s.encode() not in content for s in (password.strip(), envelope, saved["auth_method"]["password"]))
    print("DPAPI offline save; refused deletion preserved the device. Two native logins matched the exact password across edit/restart. Debug log had no saved credential.")


if __name__ == "__main__":
    os.environ.pop("RUST_LOG", None)  # exercise the fixture's saved debug preference
    with TemporaryDirectory(prefix="rrw-password-") as tmp, asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner:
        runner.run(run(Path(sys.argv[1]).resolve(strict=True), Path(tmp)))
