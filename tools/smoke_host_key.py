# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312; sys_platform == 'win32'"]
# ///
"""Windows: uv run tools/smoke_host_key.py target/debug/rust-remote.exe"""
import asyncio
from contextlib import asynccontextmanager
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
from tempfile import TemporaryDirectory
import tomllib

import asyncssh
import win32api
import win32con as vk
import win32file

from smoke_workspace import Keyboard

PROMPT = "Verify SSH host key"


def public_line(key):
    return key.export_public_key("openssh").decode().strip()


def profile(port, identity, label, pin=None):
    return f"""[[connections]]
id = {json.dumps(label)}
display_name = {json.dumps(label)}
host = "127.0.0.1"
port = {port}
username = "fixture"
auth_method = {{ public_key = {{ private_key_path = {json.dumps(str(identity))} }} }}
created_at = "2026-01-01T00:00:00Z"
""" + (f"public_key = {json.dumps(pin)}\n" if pin else "")


def stored_pin(path):
    return tomllib.loads(path.read_text(encoding="utf-8"))["connections"][0].get("public_key")


@asynccontextmanager
async def server(host, port, client, root, *, reverse_port=None, on_sftp_open=None):
    # begin_auth observes the earliest SSH user-auth request, even auth "none".
    auth_requests = []

    class Recorder(asyncssh.SSHServer):
        def begin_auth(self, username):
            auth_requests.append(username)
            return True

        def public_key_auth_supported(self):
            return True

        def validate_public_key(self, username, key):
            return username == "fixture" and key == client

        def server_requested(self, addr, port):
            return (addr, port) == ("127.0.0.1", reverse_port)

    async def echo(proc):
        proc.stdout.write("verified-shell-ready\n")
        async for line in proc.stdin:
            proc.stdout.write(f"verified-reply:{line.strip()}\n")

    def sftp(ch):
        if on_sftp_open:
            on_sftp_open()
        return asyncssh.SFTPServer(ch, chroot=os.fsencode(root))

    async with asyncssh.listen(
        "127.0.0.1", port, config=None, server_factory=Recorder, server_host_keys=[host],
        process_factory=echo, sftp_factory=sftp,
    ) as listener:
        yield listener.get_port(), auth_requests


@asynccontextmanager
async def tui(exe, config, initial="Trust fixture", *, cwd=None):
    startup = subprocess.STARTUPINFO()
    startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 0
    proc = subprocess.Popen([exe, "--config", config], cwd=cwd or config.parent, startupinfo=startup,
                            creationflags=subprocess.CREATE_NEW_CONSOLE)
    try:
        ui = Keyboard(proc)
        await ui.wait(initial, "F4 Devices")
        yield ui
        ui.key(ord("Q"), "q")
        assert await asyncio.to_thread(proc.wait, 5) == 0
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)


async def trust_prompt(ui, port, fingerprint):
    ui.key(vk.VK_RETURN, "\r")
    return await ui.wait(PROMPT, f"127.0.0.1:{port}", fingerprint)


async def devices(ui):
    await ui.wait("F4 Devices", "Trust fixture", "Connection List", absent=[PROMPT, "Error", "Host key changed"])


async def files_switch(exe, root, config, identity, new, client):
    # Introduce an independent untrusted endpoint while the first Files pane stays connected.
    other = root / "other"
    other.mkdir()
    (other / "B.txt").touch()
    async with server(new, 0, client, other) as (other_port, other_auth):
        with config.open("a", encoding="utf-8") as f:
            f.write("\n" + profile(other_port, identity, "Fixture B"))
        async with tui(exe, config) as ui:
            ui.key(vk.VK_F2)
            await ui.wait("Choose Connection")
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("SFTP File Transfer", "REMOTE.txt")
            for accept in (False, True):
                ui.key(ord("S"), "s")
                await ui.wait("Select Left Pane Source", "Fixture B")
                ui.key(vk.VK_DOWN)
                await trust_prompt(ui, other_port, new.get_fingerprint("sha256"))
                assert not other_auth
                if not accept:
                    ui.key(vk.VK_ESCAPE, "\x1b")
                    await ui.wait("SFTP File Transfer", "REMOTE.txt", "Local", absent=[PROMPT, "B.txt"])
                    assert "public_key" not in tomllib.loads(config.read_text("utf-8"))["connections"][1]
                else:
                    ui.key(ord("Y"), "y")
                    await ui.wait("Host key saved", "Repeat the action")
                    assert not other_auth
                    ui.key(vk.VK_ESCAPE, "\x1b")
                    ui.key(ord("S"), "s")
                    await ui.wait("Select Left Pane Source", "Fixture B")
                    ui.key(vk.VK_DOWN)
                    ui.key(vk.VK_RETURN, "\r")
                    view = await ui.wait("SFTP File Transfer", "B.txt", "REMOTE.txt", absent=[PROMPT])
                    col = lambda name: next(row.index(name) for row in view.splitlines() if name in row)
                    assert col("B.txt") < col("REMOTE.txt") and other_auth
                    assert tomllib.loads(config.read_text("utf-8"))["connections"][1]["public_key"] == public_line(new)
                    # A cached row would still be visible if A had been disconnected. Refresh a fresh marker.
                    (root / "sftp" / "A2.txt").touch()
                    ui.key(vk.VK_TAB, "\t")
                    ui.key(ord("R"), "r")
                    await ui.wait("B.txt", "A2.txt", "Refreshed")
                    ui.key(vk.VK_ESCAPE, "\x1b")
            ui.key(ord("Q"), "q")
            await devices(ui)
    print("Files: rejected and then approved untrusted left pane B in place; right pane A stayed open.")



async def forward_start(exe, root, identity, host, port, auth):
    config = root / "forwards.toml"
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        listener = probe.getsockname()
    config.write_text(profile(port, identity, "Trust fixture") + f"""
[settings]
default_port = 22
connection_timeout = 10
[[port_forwards]]
id = "forward"
connection_id = "Trust fixture"
forward_type = "Dynamic"
local_addr = "127.0.0.1"
local_port = {listener[1]}
display_name = "Loopback socks"
created_at = "2026-01-01T00:00:00Z"
""", encoding="utf-8")
    before = config.read_bytes()
    count = len(auth)
    async with tui(exe, config) as ui:
        ui.key(vk.VK_F5)
        await ui.wait("Workspace settings")
        ui.key(ord("P"), "p")
        await ui.wait("Port Forwards", "Loopback socks")
        await trust_prompt(ui, port, host.get_fingerprint("sha256"))
        ui.key(vk.VK_ESCAPE, "\x1b")
        await ui.wait("Port Forwards", "Loopback socks", absent=[PROMPT])
        assert len(auth) == count and config.read_bytes() == before
        await trust_prompt(ui, port, host.get_fingerprint("sha256"))
        ui.key(ord("Y"), "y")
        await ui.wait("Host key saved", "Repeat the action")
        assert len(auth) == count and stored_pin(config) == public_line(host)
        with socket.socket() as probe:
            probe.bind(listener)  # Approval alone must not have started a listener.
        ui.key(vk.VK_ESCAPE, "\x1b")
        await ui.wait("Port Forwards", "Loopback socks", absent=["Repeat the action"])
        ui.key(vk.VK_RETURN, "\r")
        await ui.wait("Loopback socks", "started", absent=[PROMPT])
        assert len(auth) > count
        ui.key(vk.VK_ESCAPE, "\x1b")
        ui.key(vk.VK_RETURN, "\r")
        await ui.wait("Loopback socks", "stopped", absent=[PROMPT])
        ui.key(vk.VK_ESCAPE, "\x1b")
        ui.key(ord("E"), "e")
        await ui.wait("Edit Port Forward")
        for _ in range(4):  # dynamic rule: type, device, address, port, name
            ui.key(vk.VK_TAB)
        ui.key(vk.VK_HOME)
        for _ in "Loopback socks":
            ui.key(vk.VK_DELETE)
        for ch in "Rule v2":
            ui.key(win32api.VkKeyScan(ch) & 255, ch)
        ui.key(vk.VK_RETURN, "\r")
        await ui.wait("Port forward updated successfully")
        ui.key(vk.VK_ESCAPE)
        ui.key(ord("Q"), "q")
        await devices(ui)
    assert tomllib.loads(config.read_text("utf-8"))["port_forwards"][0]["display_name"] == "Rule v2"
    async with tui(exe, config) as ui:
        ui.key(vk.VK_F5)
        ui.key(ord("P"), "p")
        await ui.wait("Port Forwards", "Rule v2")
        ui.key(ord("Q"), "q")
        await devices(ui)
    print("Port Forwards: only explicit retry started the listener; after stop the edited rule survived restart.")


async def reverse_forward(exe, root, identity, host, client, remote):
    async def echo(reader, writer):
        try:
            while data := await reader.read(128):
                writer.write(data)
                await writer.drain()
        finally:
            writer.close()
    config = root / "reverse.toml"
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        reverse_port = sock.getsockname()[1]
    echo_server = await asyncio.start_server(echo, "127.0.0.1", 0)
    service = echo_server.sockets[0].getsockname()[1]
    async with echo_server, server(host, 0, client, remote, reverse_port=reverse_port) as (ssh_port, _):
        config.write_text(profile(ssh_port, identity, "Trust fixture", public_line(host)) + f"""
[settings]
default_port=22
connection_timeout=10
[[port_forwards]]
id="reverse"
connection_id="Trust fixture"
display_name="Loopback reverse"
forward_type="Remote"
remote_bind_addr="127.0.0.1"
local_addr="127.0.0.1"
local_port={reverse_port}
service_host="127.0.0.1"
service_port={service}
created_at="2026-01-01T00:00:00Z"
""", encoding="utf-8")
        async with tui(exe, config) as ui:
            ui.key(vk.VK_F5)
            ui.key(ord("P"), "p")
            await ui.wait("Port Forwards", "Loopback reverse")
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("Loopback reverse", "started")
            streams = []
            try:
                for data in [b"one", b"two"]:
                    r,w = await asyncio.open_connection("127.0.0.1", reverse_port)
                    streams.append((r,w))
                    w.write(data)
                    await w.drain()
                    assert await asyncio.wait_for(r.readexactly(len(data)), 5) == data
                ui.key(vk.VK_ESCAPE, "\x1b")
                ui.key(vk.VK_RETURN, "\r")
                await ui.wait("Loopback reverse", "stopped")
                for r,_ in streams:
                    assert await asyncio.wait_for(r.read(1), 5) == b""
            finally:
                for _,w in streams:
                    w.close()
            ui.key(vk.VK_ESCAPE, "\x1b")
            ui.key(ord("Q"), "q")
            await devices(ui)
    print("Remote forwarding: two concurrent channels returned their own bytes; stop closed both streams.")


async def run(exe, root, files_only=False, forwards_only=False):
    config = root / "devices.toml"
    old, new = [asyncssh.generate_private_key("ssh-ed25519") for _ in range(2)]
    client_key = asyncssh.generate_private_key("ssh-ed25519")
    identity = root / "id"
    identity.write_bytes(client_key.export_private_key("openssh"))
    client = client_key.convert_to_public()
    remote = root / "sftp"
    remote.mkdir()
    (remote / "REMOTE.txt").touch()
    fp = old.get_fingerprint("sha256")
    async with server(old, 0, client, remote) as (port, auth):
        if forwards_only:
            await forward_start(exe, root, identity, old, port, auth)
            await reverse_forward(exe, root, identity, old, client, remote)
            return
        config.write_text(profile(port, identity, "Trust fixture", public_line(old) if files_only else None) +
                          "[settings]\ndefault_port = 22\nconnection_timeout = 10\n", encoding="utf-8")
        if files_only:
            await files_switch(exe, root, config, identity, new, client)
            return
        before = config.read_bytes()
        async with tui(exe, config) as ui:
            # Files goes through the same verifier before SFTP.
            ui.key(vk.VK_F2)
            await ui.wait("Choose Connection")
            view = await trust_prompt(ui, port, fp)
            for label in (PROMPT, fp, "Trust and save"):
                line = next(row for row in view.splitlines() if label in row)
                print("  " + line.strip().replace(fp, "SHA256:<ephemeral test fingerprint>"))
            await asyncio.sleep(11)  # Outlast the 10-second SSH connect timeout.
            await ui.wait(PROMPT, fp)
            assert not auth and config.read_bytes() == before
            ui.key(vk.VK_RETURN, "\r")  # Pressing Enter on the prompt must never grant trust.
            ui.key(ord("N"), "n")
            await ui.wait("Choose Connection", absent=[PROMPT])
            ui.key(vk.VK_F4)
            await devices(ui)
            assert not auth and config.read_bytes() == before
            await trust_prompt(ui, port, fp)
            ui.key(vk.VK_ESCAPE, "\x1b")
            await devices(ui)
            assert not auth and config.read_bytes() == before
            print("First SSH/SFTP contact: displayed fingerprint matches AsyncSSH; Enter/n and Esc saved nothing, zero auth attempts.")

            await trust_prompt(ui, port, fp)
            # A Windows reader denying write/delete sharing makes both overwrite and atomic replace fail.
            lock = win32file.CreateFile(str(config), vk.GENERIC_READ, vk.FILE_SHARE_READ, None, vk.OPEN_EXISTING, 0, None)
            try:
                ui.key(ord("Y"), "y")
                await ui.wait("Error", absent=[PROMPT, "verified-shell-ready"])
                assert not auth and config.read_bytes() == before
            finally:
                lock.Close()
            ui.key(vk.VK_ESCAPE, "\x1b")
            await devices(ui)
            await trust_prompt(ui, port, fp)
            assert not auth
            print("Persistence failure: error visible, zero auth attempts, unchanged file; same process prompts again.")
            ui.key(ord("Y"), "y")
            await ui.wait("verified-shell-ready", absent=[PROMPT])
            assert auth and stored_pin(config) == public_line(old)
            ui.key(vk.VK_F8)
            await devices(ui)

        async with tui(exe, config) as ui:
            count = len(auth)
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("verified-shell-ready", absent=[PROMPT])
            ui.text("restarted")
            await ui.wait("verified-reply:restarted")
            assert len(auth) > count and stored_pin(config) == public_line(old)
            ui.key(vk.VK_F8)
            await devices(ui)
        print("Explicit y saved the matching public key; restarted TUI connected and exchanged data without another prompt.")

        fresh = root / "fresh.toml"
        async with tui(exe, fresh, "No saved SSH devices") as ui:
            count = len(auth)
            ui.key(ord("N"), "n")
            await ui.wait("New SSH Connection")
            # 'id' is the ephemeral private key in the process's isolated current directory.
            for value in ("127.0.0.1", str(port), "fixture", "", "id"):
                for ch in value:
                    ui.key(win32api.VkKeyScan(ch) & 255, ch)
                ui.key(vk.VK_TAB, "\t")
            for ch in "newfixture":
                ui.key(ord(ch.upper()), ch)
            await trust_prompt(ui, port, fp)
            ui.key(vk.VK_ESCAPE, "\x1b")
            await ui.wait("New SSH Connection", "127.0.0.1", absent=[PROMPT])
            assert len(auth) == count and not fresh.exists()
            await trust_prompt(ui, port, fp)
            ui.key(ord("Y"), "y")
            await ui.wait("verified-shell-ready")
            records = tomllib.loads(fresh.read_text("utf-8"))["connections"]
            assert len(records) == 1 and records[0]["display_name"] == "newfixture" and records[0]["public_key"] == public_line(old)
            ui.key(vk.VK_F8)
            await ui.wait("Connection List", "newfixture")
        print("New device form: Esc preserved the entered fields without creating a file; y created exactly one pinned device.")

        await files_switch(exe, root, config, identity, new, client)
        await forward_start(exe, root, identity, old, port, auth)
        await reverse_forward(exe, root, identity, old, client, remote)

    # New server key, same host:port and same on-disk profile. No approval path.
    async with server(new, port, client, remote) as (_, auth):
        async with tui(exe, config) as ui:
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("Host key changed", fp, new.get_fingerprint("sha256"),
                          absent=[PROMPT, "verified-shell-ready"])
            ui.key(ord("Y"), "y")
            await ui.wait("Host key changed", absent=[PROMPT, "verified-shell-ready"])
            ui.key(vk.VK_ESCAPE, "\x1b")
            await devices(ui)
            assert not auth and stored_pin(config) == public_line(old)
        print("Rotated server key on the same endpoint: blocked; y could not override; original pin retained, zero auth attempts.")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    with TemporaryDirectory(prefix="rrw-hostkey-") as tmp:
        # Selector sockets avoid Windows Proactor shutdown callbacks on deliberately rejected handshakes.
        with asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner:
            runner.run(run(Path(sys.argv[1]).resolve(strict=True), Path(tmp),
                           files_only="--files-only" in sys.argv[2:], forwards_only="--forwards-only" in sys.argv[2:]))
