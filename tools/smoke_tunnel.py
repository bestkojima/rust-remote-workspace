# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312", "psutil==7.1.3"]
# ///
"""Drive a real Windows workspace through two distinct SSH servers and rathole 0.5.0."""
import argparse
import asyncio
import base64
from contextlib import contextmanager
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
from tempfile import TemporaryDirectory
import time
import tomllib

import asyncssh
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey
import psutil
from ntsecuritycon import FILE_ALL_ACCESS
import win32api
import win32con as vk
import win32security as sec

from smoke_host_key import PROMPT, profile, public_line, tui

LO = "127.0.0.1"


def private_dir(path):
    """Set a user-only inheritable DACL before any key/token is written."""
    token = sec.OpenProcessToken(win32api.GetCurrentProcess(), vk.TOKEN_QUERY)
    user, _ = sec.GetTokenInformation(token, sec.TokenUser)
    acl = sec.ACL()
    acl.AddAccessAllowedAceEx(sec.ACL_REVISION, vk.OBJECT_INHERIT_ACE | vk.CONTAINER_INHERIT_ACE,
                               FILE_ALL_ACCESS, user)
    sec.SetNamedSecurityInfo(str(path), sec.SE_FILE_OBJECT,
                             sec.DACL_SECURITY_INFORMATION | sec.PROTECTED_DACL_SECURITY_INFORMATION,
                             None, None, acl, None)


def unused_ports(count):
    sockets = []
    try:
        for _ in range(count):
            sock = socket.socket()
            sockets.append(sock)
            sock.bind((LO, 0))
        return [s.getsockname()[1] for s in sockets]
    finally:
        for sock in sockets:
            sock.close()


def listeners(port):
    return [(conn.laddr.ip, conn.pid) for conn in psutil.net_connections("tcp4")
            if conn.status == psutil.CONN_LISTEN and conn.laddr.port == port]


async def until(check, label, timeout=20):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        await asyncio.sleep(.1)
    raise AssertionError(f"timed out: {label}")


class Endpoint:
    def __init__(self, port, host_key, client_key, allow_forward=None, sftp_root=None):
        self.port, self.key, self.login = port, host_key, client_key.convert_to_public()
        self.permit = allow_forward
        self.sftp_root = sftp_root
        self.auth, self.destinations, self.live = [], [], set()
        self.listener = None
        self.connections = 0

    async def start(self):
        endpoint = self

        class Recorder(asyncssh.SSHServer):
            def connection_made(self, conn):
                self.conn = conn
                endpoint.live.add(conn)
                endpoint.connections += 1

            def connection_lost(self, exc):
                endpoint.live.discard(self.conn)

            def begin_auth(self, username):
                endpoint.auth.append(username)
                return True

            def public_key_auth_supported(self):
                return True

            def validate_public_key(self, username, key):
                return username == "fixture" and key == endpoint.login

            def connection_requested(self, dest_host, dest_port, orig_host, orig_port):
                endpoint.destinations.append((dest_host, dest_port))
                return endpoint.permit is not None and (dest_host, dest_port) == (LO, endpoint.permit)

        async def echo(proc):
            proc.stdout.write("device-shell-ready\n")
            async for line in proc.stdin:
                proc.stdout.write("device-reply:" + line.strip() + "\n")

        options = {} if self.permit else {"process_factory": echo}
        if self.sftp_root is not None:
            options["sftp_factory"] = lambda chan: asyncssh.SFTPServer(chan, chroot=os.fsencode(self.sftp_root))
        self.listener = await asyncssh.listen(LO, self.port, server_factory=Recorder, config=None,
                                              server_host_keys=[self.key], **options)

    async def stop(self):
        listener, self.listener = self.listener, None
        if listener:
            listener.close()
        connections = list(self.live)
        for conn in connections:
            conn.abort()
        await asyncio.gather(*(conn.wait_closed() for conn in connections), return_exceptions=True)
        if listener:
            await asyncio.wait_for(listener.wait_closed(), 5)


class Rathole:
    def __init__(self, binary, root, device_port, control, service):
        self.binary, self.root = binary, root
        self.server, self.client = root / "vps.toml", root / "agent.toml"
        self.process = None
        token = secrets.token_urlsafe(32)
        key = X25519PrivateKey.generate()
        noise = lambda raw: base64.b64encode(raw).decode("ascii")
        self.server.write_text(f"""[server]
bind_addr = "{LO}:{control}"
[server.transport]
type = "noise"
[server.transport.noise]
local_private_key = "{noise(key.private_bytes_raw())}"
[server.services.ssh]
bind_addr = "{LO}:{service}"
token = "{token}"
""", encoding="utf-8")
        self.client.write_text(f"""[client]
remote_addr = "{LO}:{control}"
retry_interval = 1
[client.transport]
type = "noise"
[client.transport.noise]
remote_public_key = "{noise(key.public_key().public_bytes_raw())}"
[client.services.ssh]
local_addr = "{LO}:{device_port}"
token = "{token}"
""", encoding="utf-8")

    def start(self):
        self.process = subprocess.Popen([self.binary, "--server", self.server], cwd=self.root,
                                        creationflags=subprocess.CREATE_NO_WINDOW,
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def stop(self):
        if self.process is not None:
            self.process.terminate()
            self.process.wait(5)
            self.process = None


@contextmanager
def device_agent(rathole, exe):
    command = ([exe, "tunnel-agent", "--config", rathole.client, "--rathole", rathole.binary,
                "--stop-on-stdin-close"] if exe else [rathole.binary, "--client", rathole.client])
    proc = subprocess.Popen(command, cwd=rathole.root, stdin=subprocess.PIPE,
                            stderr=subprocess.PIPE if exe else subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)
    try:
        yield proc
    finally:
        if exe:
            proc.stdin.close()
            try:
                rc = proc.wait(12)
            except subprocess.TimeoutExpired:
                # Only fixture processes created by this agent may be stopped.
                for child in psutil.Process(proc.pid).children():
                    child.kill()
                proc.kill()
                proc.wait(5)
                raise
            if rc:
                raise AssertionError(f"tunnel-agent exited {rc}")
        else:
            proc.terminate()
            proc.wait(5)


async def fixture_probe(gateway, device, service, gateway_identity, dev_identity):
    opts = lambda endpoint, identity: dict(username="fixture", client_keys=[identity], config=None,
                                            known_hosts=([endpoint.key.convert_to_public()], [], []))
    async with asyncssh.connect(LO, gateway.port, **opts(gateway, gateway_identity)) as jump:
        try:
            await jump.open_connection(LO, device.port)
        except asyncssh.ChannelOpenError:
            pass
        else:
            raise AssertionError("gateway accepted a destination outside the selected service")
        async with asyncssh.connect(LO, service, tunnel=jump, **opts(device, dev_identity)) as remote:
            async with remote.create_process() as shell:
                assert (await shell.stdout.readline()).strip() == "device-shell-ready"
                shell.stdin.write("fixture-only\n")
                assert (await shell.stdout.readline()).strip() == "device-reply:fixture-only"
                shell.stdin.write_eof()
    print("Fixture: the gateway rejected the raw device port; Noise rathole service returned the device's SSH reply.")


async def open_manager(ui):
    ui.key(vk.VK_F5)
    await ui.wait("Workspace settings")
    ui.key(ord("T"), "t")
    await ui.wait("VPS tunnels")


async def open_device_files(ui):
    ui.key(vk.VK_F2)
    ui.key(ord("/"), "/")
    ui.text("dev")
    ui.key(vk.VK_RETURN, "\r")
    await ui.wait("R3DEV.txt", "R3HOME.txt")


async def workspace(exe, root, gw, dev, rathole, gw_identity, dev_identity, local, service, args):
    config = root / "workspace.toml"
    def pins():
        return {r["id"]: r.get("public_key") for r in tomllib.loads(config.read_text("utf-8"))["connections"]}
    conf = ""
    for label, port, key in [("vps", gw.port, gw_identity), ("dev", local, dev_identity)]:
        identity = root / (label + "-identity")
        identity.write_bytes(key.export_private_key("openssh"))
        conf += profile(port, identity, label)
    config.write_text(conf + "[settings]\ndefault_port=22\nconnection_timeout=10\n", encoding="utf-8")

    local_files = root / "local-files"
    local_files.mkdir()
    async with tui(exe, config, "vps", cwd=local_files) as ui:
        await open_manager(ui)
        ui.key(ord("N"), "n")
        await ui.wait("New VPS tunnel", "VPS SSH profile", "Device SSH profile")
        ui.key(vk.VK_TAB, "\t")
        ui.key(vk.VK_TAB, "\t")
        ui.text(str(service))
        await ui.wait("VPS tunnels", "Stopped", f"VPS 127.0.0.1:{service}")
        route = tomllib.loads(config.read_text("utf-8"))["tunnels"]
        assert route == [{"gateway_id": "vps", "device_id": "dev", "service_port": service}]
        assert not gw.auth and not dev.auth and not listeners(local)
        ui.key(vk.VK_RETURN, "\r")
        await ui.wait(PROMPT, gw.key.get_fingerprint("sha256"))
        ui.key(vk.VK_ESCAPE, "\x1b")
        await ui.wait("VPS tunnels", "Blocked", absent=[PROMPT])
        assert pins() == {"vps": None, "dev": None} and not gw.auth and not dev.auth
        ui.key(vk.VK_RETURN, "\r")
        await ui.wait(PROMPT, gw.key.get_fingerprint("sha256"))
        ui.key(ord("Y"), "y")
        await ui.wait("Host key saved", "Repeat the action")
        assert not gw.auth and not listeners(local)
        assert pins() == {"vps": public_line(gw.key), "dev": None}
        ui.key(vk.VK_ESCAPE, "\x1b")
        ui.key(vk.VK_RETURN, "\r")
        await ui.wait("Forward ready", absent=[PROMPT])
        assert listeners(local) == [(LO, ui.proc.pid)] and gw.auth and not dev.auth
        ui.key(ord("O"), "o")
        await ui.wait(PROMPT, dev.key.get_fingerprint("sha256"))
        assert not dev.auth and gw.destinations == [(LO, service)]
        ui.key(ord("Y"), "y")
        await ui.wait("device-shell-ready")
        ui.text("viavps")
        await ui.wait("device-reply:viavps")
        assert pins() == {"vps": public_line(gw.key), "dev": public_line(dev.key)}
        assert dev.auth and all(dest == (LO, service) for dest in gw.destinations)
        ui.key(vk.VK_F8)
        await open_manager(ui)
        print("TUI: saved a VPS route, verified two independent fingerprints, exchanged shell data through the permitted gateway channel.")

        if dev.sftp_root is not None:
            (dev.sftp_root / "R3DEV.txt").touch()
            (local_files / "R3HOME.txt").touch()
            previous = len(gw.destinations)
            if args.files:
                from smoke_files import exercise_open_files
                await open_device_files(ui)
                await exercise_open_files(ui, local_files, dev.sftp_root)
                ui.key(ord("Q"), "q")
                await open_manager(ui)
                print("Files: two-pane write/read/rename/delete completed through the restricted rathole route.")
            if args.rclone:
                from smoke_sync import exercise_open_files
                await open_device_files(ui)
                await exercise_open_files(ui, local_files, dev.sftp_root, args.rclone)
                await open_manager(ui)
                print("Sync: reviewed and copied directories through the same pinned device route.")
            assert len(gw.destinations) > previous and set(gw.destinations) == {(LO, service)}

        # Drop and restore the VPS while the reverse service itself is healthy.
        count = len(gw.auth)
        await gw.stop()
        await ui.wait("VPS offline", "Retry")
        assert not listeners(local)
        await gw.start()
        await ui.wait("Forward ready", absent=[PROMPT])
        assert len(gw.auth) > count and listeners(local) == [(LO, ui.proc.pid)]
        # Drop the reverse service: the gateway still authenticates, but it cannot reach the device.
        rathole.stop()
        count = len(dev.auth)
        ui.key(ord("O"), "o")
        await ui.wait("Error")
        assert len(dev.auth) == count and listeners(local) == [(LO, ui.proc.pid)]
        ui.key(vk.VK_ESCAPE, "\x1b")
        await open_manager(ui)
        await ui.wait("Device service unavailable")
        rathole.start()
        await until(lambda: listeners(service) == [(LO, rathole.process.pid)], "automatic rathole reconnection")
        ui.key(ord("O"), "o")
        await ui.wait("device-shell-ready")
        ui.text("backonline")
        await ui.wait("device-reply:backonline")
        ui.key(vk.VK_F8)
        await open_manager(ui)
        print("Recovery: loss of VPS closed home listener, same-key VPS reconnected automatically; reverse service outage recovered with the same device agent.")

        # Changed device key cannot be accepted even with a healthy, pinned gateway.
        saved_dev = dev.key
        await dev.stop()
        dev.key = asyncssh.generate_private_key("ssh-ed25519")
        await dev.start()
        count = len(dev.auth)
        ui.key(ord("O"), "o")
        await ui.wait("Host key changed", saved_dev.get_fingerprint("sha256"), dev.key.get_fingerprint("sha256"))
        assert len(dev.auth) == count and pins()["dev"] == public_line(saved_dev)
        ui.key(vk.VK_ESCAPE, "\x1b")
        await open_manager(ui)
        ui.key(ord("X"), "x")
        await ui.wait("Stopped")
        assert not listeners(local)
        # Rotate VPS at the same address. Do not use a changed gateway to contact the device.
        saved_gw = gw.key
        await gw.stop()
        gw.key = asyncssh.generate_private_key("ssh-ed25519")
        await gw.start()
        count = len(gw.auth)
        ui.key(vk.VK_RETURN, "\r")
        await ui.wait("Host key changed", saved_gw.get_fingerprint("sha256"), gw.key.get_fingerprint("sha256"))
        assert len(gw.auth) == count and not listeners(local) and pins()["vps"] == public_line(saved_gw)
        ui.key(vk.VK_ESCAPE, "\x1b")
        ui.key(ord("X"), "x")
        await ui.wait("Stopped")
        assert not listeners(local)
        print("Identity: changed device and gateway keys were both blocked before user auth; both original pins survived.")
        # Stop while the gateway is unreachable. Restoring it must not resurrect this route.
        await gw.stop()
        gw.key = saved_gw
        ui.key(vk.VK_RETURN, "\r")
        await ui.wait("VPS offline", "Retry")
        ui.key(ord("X"), "x")
        await ui.wait("Stopped")
        await gw.start()
        await asyncio.sleep(2.2)  # Exceed the first retry delay.
        assert not listeners(local) and len(gw.auth) == count
        ui.key(vk.VK_ESCAPE, "\x1b")
        await ui.wait("Workspace settings")



async def run(args, root):
    private_dir(root)
    dev_port, gw_port, control, service, local = unused_ports(5)
    make_key = lambda: asyncssh.generate_private_key("ssh-ed25519")
    gw_identity, dev_identity = make_key(), make_key()
    gateway = Endpoint(gw_port, make_key(), gw_identity, allow_forward=service)
    remote_files = root / "remote-files" if args.files or args.rclone else None
    if remote_files is not None:
        remote_files.mkdir()
    device = Endpoint(dev_port, make_key(), dev_identity, sftp_root=remote_files)
    rathole = Rathole(args.rathole, root, dev_port, control, service)
    try:
        await device.start()
        await gateway.start()
        rathole.start()
        await until(lambda: listeners(control), "rathole control listener")
        with device_agent(rathole, None if args.fixture_only else args.exe) as agent:
            def service_ready():
                if agent.poll() is not None:
                    raise AssertionError(f"agent exited early ({agent.returncode})")
                return listeners(service)
            assert await until(service_ready, "client registered its service") == [(LO, rathole.process.pid)]
            if args.fixture_only:
                await fixture_probe(gateway, device, service, gw_identity, dev_identity)
            else:
                children = [p for p in psutil.Process(agent.pid).children() if Path(p.exe()).samefile(args.rathole)]
                assert len(children) == 1, f"expected one rathole child; saw {[(p.pid, p.name()) for p in psutil.Process(agent.pid).children()]}"
                await workspace(args.exe, root, gateway, device, rathole, gw_identity, dev_identity, local, service, args)
        if not args.fixture_only:
            assert not any(child.is_running() for child in children)
            # Upstream v0.5.0 accepts on the VPS until the control task detects
            # closure; a listening socket alone does not prove the device is online.
            before = device.connections
            since = time.monotonic()
            probes = 0
            while listeners(service) and time.monotonic() - since < 40:
                try:
                    reader, writer = await asyncio.wait_for(asyncio.open_connection(LO, service), 2)
                except (OSError, TimeoutError):
                    continue
                probes += 1
                try:
                    try:
                        banner = await asyncio.wait_for(reader.readline(), 1)
                    except (OSError, TimeoutError):
                        banner = b""
                    assert not banner, "stopped device still served an SSH banner"
                finally:
                    writer.close()
                    try:
                        await writer.wait_closed()
                    except OSError:
                        pass
                await asyncio.sleep(.2)
            assert not listeners(service) and device.connections == before
            assert not listeners(local)
            print(f"Manual stop: no managed child or home listener; {probes} post-stop probes reached no device. "
                  f"VPS service listener closed after {time.monotonic() - since:.1f}s with probes.")
    finally:
        rathole.stop()
        await gateway.stop()
        await device.stop()


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("exe", type=Path, help="rust-remote.exe candidate (unused by --fixture-only)")
    cli.add_argument("--rathole", type=Path, required=True, help="official rathole 0.5.0 executable")
    cli.add_argument("--fixture-only", action="store_true", help="exercise the harness without the workspace")
    cli.add_argument("--files", action="store_true", help="also exercise the Files workspace through the same route")
    cli.add_argument("--rclone", type=Path, help="exercise Sync through the same route with this pinned rclone binary")
    args = cli.parse_args()
    args.exe, args.rathole = args.exe.resolve(), args.rathole.resolve(strict=True)
    if args.rclone:
        args.rclone = args.rclone.resolve(strict=True)
    if "Build Version:       0.5.0" not in subprocess.check_output([args.rathole, "--version"], text=True):
        cli.error("this fixture requires rathole 0.5.0")
    with TemporaryDirectory(prefix="rrw-tunnel-") as tmp:
        with asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner:
            runner.run(run(args, Path(tmp)))
