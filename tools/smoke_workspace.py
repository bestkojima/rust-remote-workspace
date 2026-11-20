# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312; sys_platform == 'win32'"]
# ///
"""Windows: uv run tools/smoke_workspace.py target/debug/rust-remote.exe"""
import asyncio
from contextlib import asynccontextmanager
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
import time

import asyncssh
import pywintypes
import win32api
import win32con as vk
import win32console as console
import win32file
import win32gui
import win32process


def console_handle(name):
    handle = win32file.CreateFile(
        name, vk.GENERIC_READ | vk.GENERIC_WRITE, vk.FILE_SHARE_READ | vk.FILE_SHARE_WRITE,
        None, vk.OPEN_EXISTING, 0, None,
    )
    return console.PyConsoleScreenBufferType(handle.Detach())


def cpu_time(handle):
    times = win32process.GetProcessTimes(handle)
    return (times["KernelTime"] + times["UserTime"]) * 1e-7


def working_set(handle):
    return win32process.GetProcessMemoryInfo(handle)["WorkingSetSize"] / (1024 ** 2)


class Keyboard:
    """Read the child's active screen, including alternate buffers. Send native key events."""
    def __init__(self, proc):
        self.proc = proc
        try:
            console.FreeConsole()
        except pywintypes.error:
            pass  # The invoking shell may already be detached.
        # CREATE_NEW_CONSOLE returns before the console host is attachable.
        for attempt in range(50):
            try:
                console.AttachConsole(proc.pid)
                break
            except pywintypes.error as exc:
                if exc.winerror != 6 or proc.poll() is not None or attempt == 49:
                    raise
                time.sleep(.01)
        self.input = console_handle("CONIN$")

    def screen(self):
        buf = console_handle("CONOUT$")
        info = buf.GetConsoleScreenBufferInfo()
        rect = info["Window"]
        # A wide glyph occupies two cells but returns a single character.
        return "\n".join(buf.ReadConsoleOutputCharacter(
            rect.Right - rect.Left + 1, console.PyCOORDType(rect.Left, y)
        ).rstrip() for y in range(rect.Top, rect.Bottom + 1))

    async def wait(self, *need, absent=()):
        end = time.monotonic() + 18
        while time.monotonic() < end:
            if self.proc.poll() is not None:
                raise AssertionError(f"TUI exited with {self.proc.returncode} waiting for {need}")
            view = self.screen()
            if all(word in view for word in need) and all(word not in view for word in absent):
                return view
            await asyncio.sleep(.05)
        raise AssertionError(f"Expected {need}, absent {absent}; last screen:\n{view.rstrip()}")

    def key(self, keycode, ch="\0"):
        records = []
        for down in (True, False):
            event = console.PyINPUT_RECORDType(console.KEY_EVENT)
            event.KeyDown, event.RepeatCount = down, 1
            event.VirtualKeyCode, event.Char = keycode, ch
            records.append(event)
        self.input.WriteConsoleInput(records)

    def text(self, value):
        for ch in value:
            self.key(ord(ch.upper()), ch)
        self.key(vk.VK_RETURN, "\r")


@asynccontextmanager
async def fixture(root):
    local, remote = root / "local", root / "remote"
    local.mkdir()
    remote.mkdir()
    (local / "LOCAL.txt").write_bytes(bytes(range(256)) * 4096)
    (remote / "REMOTE.txt").touch()
    host = asyncssh.generate_private_key("ssh-ed25519")
    client = asyncssh.generate_private_key("ssh-ed25519")
    identity = root / "client.key"
    identity.write_bytes(client.export_private_key("openssh"))
    shells = {}

    class Server(asyncssh.SSHServer):
        def public_key_auth_supported(self):
            return True

        def validate_public_key(self, username, key):
            return username == "fixture" and key == client.convert_to_public()

    async def echo(proc):
        # Input is only echoed. No OS shell or command execution.
        number = len(shells) + 1
        shells[number] = proc
        proc.stdout.write(f"screen-{number}-ready\n")
        while True:
            try:
                line = await proc.stdin.readline()
            except asyncssh.TerminalSizeChanged:
                continue
            if not line:
                break
            proc.stdout.write(f"reply-{number}-{line.strip()}\n")

    async with asyncssh.listen(
        "127.0.0.1", 0, config=None, server_factory=Server, server_host_keys=[host],
        process_factory=echo, sftp_factory=lambda ch: asyncssh.SFTPServer(ch, chroot=os.fsencode(remote)),
    ) as server:
        config = root / "devices.toml"
        pin = host.export_public_key("openssh").decode().strip()
        config.write_text(f"""[[connections]]
id = "loopback"
display_name = "Echo fixture"
host = "127.0.0.1"
port = {server.get_port()}
username = "fixture"
created_at = "2026-01-01T00:00:00Z"
public_key = {json.dumps(pin)}
auth_method = {{ public_key = {{ private_key_path = {json.dumps(str(identity))} }} }}
[settings]
default_port = 22
connection_timeout = 10
""", encoding="utf-8")
        yield local, config, shells


async def exercise(exe, root):
    async with fixture(root) as (local, config, shells):
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0
        started = time.perf_counter()
        # The test runner may disable color. Enable normal color for this test console only.
        env = os.environ.copy()
        env.pop("NO_COLOR", None)
        env["TERM"] = "xterm-256color"
        proc = subprocess.Popen([exe, "--config", config], cwd=local, env=env,
                                creationflags=subprocess.CREATE_NEW_CONSOLE, startupinfo=startup)
        try:
            ui = Keyboard(proc)
            view = await ui.wait("Echo fixture", "F4 Devices")
            first_paint = 1000 * (time.perf_counter() - started)
            print("Workspace:", " | ".join(view.splitlines()[:2]))
            handle = win32api.OpenProcess(vk.PROCESS_QUERY_INFORMATION | vk.PROCESS_VM_READ, False, proc.pid)
            idle_start, cpu_start = time.perf_counter(), cpu_time(handle)
            await asyncio.sleep(5)
            elapsed, cpu = time.perf_counter() - idle_start, cpu_time(handle) - cpu_start
            print(f"Startup to first visible device list: {first_paint:.0f} ms (50 ms screen polling)")
            print(f"Idle: {working_set(handle):.1f} MiB working set; CPU {cpu*100/elapsed:.2f}% of one core over {elapsed:.2f} s")
            for i, marker in enumerate(("alpha", "bravo", "charlie"), start=1):
                ui.key(vk.VK_RETURN, "\r")
                await ui.wait(f"screen-{i}-ready")
                ui.text(marker)
                await ui.wait(f"reply-{i}-{marker}", absent=[f"screen-{n}-ready" for n in range(1, i)])
                if i in (1, 3):
                    print(f"{i} open SSH: {working_set(handle):.1f} MiB working set")
                if i < 3:
                    ui.key(vk.VK_F12)
                    await ui.wait(f"SSH {i} open")
            # All three sockets coexist. Deliver new data to parked B while C is active.
            shells[2].stdout.write("background-bravo\n")
            ui.key(vk.VK_F6)
            await ui.wait("reply-1-alpha", absent=["bravo", "charlie"])
            ui.key(vk.VK_F6)
            await ui.wait("reply-2-bravo", "background-bravo", absent=["alpha", "charlie"])
            ui.key(vk.VK_F6)
            view = await ui.wait("reply-3-charlie", absent=["alpha", "bravo"])
            print("Terminal:", view.splitlines()[0])
            print("Three concurrent SSH PTYs: independent screen contents, input routing and parked output.")
            shells[3].stdout.write("\x1b[91mRED-SAMPLE\x1b[0mPLAIN-SAMPLE\n")
            view = await ui.wait("RED-SAMPLEPLAIN-SAMPLE")
            output = console_handle("CONOUT$")
            rect = output.GetConsoleScreenBufferInfo()["Window"]
            row, line = next((y, text) for y, text in enumerate(view.splitlines()) if "RED-SAMPLE" in text)
            def color(word):
                pos = console.PyCOORDType(rect.Left + line.index(word), rect.Top + row)
                return output.ReadConsoleOutputAttribute(1, pos)[0] & 15
            red, plain = color("RED-SAMPLE"), color("PLAIN-SAMPLE")
            assert red != plain and red & 7 == console.FOREGROUND_RED, (red, plain)

            original = shells[3].get_terminal_size()[:2]
            # Resize the actual conhost window, not the inactive primary console buffer.
            hwnd = console.GetConsoleWindow()
            x, y, right, bottom = win32gui.GetWindowRect(hwnd)
            win32gui.SetWindowPos(hwnd, None, 0, 0, right - x - 80, bottom - y - 40,
                                  vk.SWP_NOMOVE | vk.SWP_NOZORDER)
            actual = output.GetConsoleScreenBufferInfo()["Window"]
            expected = (actual.Right-actual.Left+1, actual.Bottom-actual.Top)  # one row is the tab strip
            assert expected[0] < original[0] and expected[1] < original[1], (original, expected)
            async with asyncio.timeout(5):
                while shells[3].get_terminal_size()[:2] != expected:
                    await asyncio.sleep(.05)
            print(f"ANSI red/reset console attributes: {red}/{plain}. Foreground PTY resized {original} -> {expected}.")
            win32gui.SetWindowPos(hwnd, None, 0, 0, right - x, bottom - y, vk.SWP_NOMOVE | vk.SWP_NOZORDER)
            async with asyncio.timeout(5):
                while shells[3].get_terminal_size()[:2] != original:
                    await asyncio.sleep(.05)
            # Drop B's transport on the server while C is the foreground tab.
            shells[2].channel.get_connection().abort()
            ui.text("survives")
            await ui.wait("reply-3-survives")
            ui.key(vk.VK_F6)
            await ui.wait("reply-1-alpha")
            ui.text("alive")
            await ui.wait("reply-1-alive", absent=["bravo", "charlie"])
            ui.key(vk.VK_F12)
            await ui.wait("SSH 2 open")
            print("Lost background B: C and A both answered fresh input; workspace counted 2 open.")
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("screen-4-ready", absent=["reply-2-bravo", "background-bravo"])
            ui.text("fresh")
            await ui.wait("reply-4-fresh")
            ui.key(vk.VK_F8)
            await ui.wait("reply-1-alive")
            ui.key(vk.VK_F12)
            await ui.wait("SSH 2 open")
            ui.key(vk.VK_F1)
            await ui.wait("reply-1-alive")
            ui.key(vk.VK_F8)
            await ui.wait("reply-3-survives")
            print("Reconnected same device: a fresh fourth shell; closing it preserved A and C.")
            ui.key(vk.VK_F12)
            await ui.wait("SSH 1 open")
            ui.key(vk.VK_F2)
            await ui.wait("Choose Connection")
            samples = []
            finished = asyncio.Event()
            async def sample_files_memory():
                while not finished.is_set():
                    samples.append(working_set(handle))
                    await asyncio.sleep(.02)
            sampler = asyncio.create_task(sample_files_memory())
            ui.key(vk.VK_RETURN, "\r")
            view = await ui.wait("SFTP File Transfer", "LOCAL.txt", "REMOTE.txt")
            local_col = next(row.index("LOCAL.txt") for row in view.splitlines() if "LOCAL.txt" in row)
            remote_col = next(row.index("REMOTE.txt") for row in view.splitlines() if "REMOTE.txt" in row)
            assert local_col < remote_col
            print(f"Files: LOCAL.txt at column {local_col + 1}, REMOTE.txt at {remote_col + 1}.")
            ui.key(vk.VK_DOWN)  # first row is ../
            ui.key(ord("C"), "c")
            # Never paste a directory; check the selected *header*, not a row name.
            await ui.wait("[COPY MODE] LOCAL.txt")
            ui.key(vk.VK_TAB, "\t")
            ui.key(ord("V"), "v")
            await ui.wait("SFTP Transfers", "Completed", "Press Enter or Esc")
            assert (root / "remote" / "LOCAL.txt").read_bytes() == (local / "LOCAL.txt").read_bytes()
            finished.set()
            await sampler
            print(f"1 MiB upload matched byte-for-byte. Files open through upload: max sampled working set "
                  f"{max(samples):.1f} MiB ({len(samples)} observations, target 20 ms interval).")
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("SFTP File Transfer", absent=["SFTP Transfers"])
            ui.key(ord("Q"), "q")
            await ui.wait("SSH 1 open")
            ui.key(ord("Q"), "q")
            assert await asyncio.to_thread(proc.wait, 8) == 0
        finally:
            if proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=5)


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    exe = Path(sys.argv[1]).resolve(strict=True)
    with TemporaryDirectory(prefix="rrw-smoke-") as tmp:
        asyncio.run(exercise(exe, Path(tmp)))
