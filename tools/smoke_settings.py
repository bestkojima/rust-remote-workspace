# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312"]
# ///
"""Edit defaults on a real Windows console; save an offline device and verify the next launch."""
import asyncio
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import tomllib

import win32api
import win32con as vk
import win32console
from smoke_host_key import tui


def ctrl(ui, ch):
    events = []
    for down in (True, False):
        k = win32console.PyINPUT_RECORDType(win32console.KEY_EVENT)
        k.KeyDown, k.RepeatCount = down, 1
        k.VirtualKeyCode, k.Char, k.ControlKeyState = ord(ch.upper()), chr(ord(ch.lower())-96), vk.LEFT_CTRL_PRESSED
        events.append(k)
    ui.input.WriteConsoleInput(events)


def write(ui, text):
    for c in text:
        ui.key(win32api.VkKeyScan(c) & 255, c)


async def settings(ui):
    ui.key(vk.VK_F5)
    await ui.wait("Workspace settings")
    ui.key(ord("E"), "e")
    await ui.wait("Edit workspace defaults")


async def run(exe, root):
    cfg = root / "devices.toml"
    tool = str(root / "custom tools" / "rclone.exe")
    async with tui(exe, cfg, "No saved SSH devices") as ui:
        await settings(ui)
        ctrl(ui,"a"); write(ui,"2227")
        ui.key(vk.VK_TAB); ctrl(ui,"a"); write(ui,"5001")
        ui.key(vk.VK_RETURN)
        await ui.wait("between 1 and 5000")
        assert not cfg.exists()
        ctrl(ui,"a"); write(ui,"1234")
        ui.key(vk.VK_TAB); ctrl(ui,"a"); write(ui,tool)
        ui.key(vk.VK_TAB); ui.key(vk.VK_RIGHT); ui.key(vk.VK_RIGHT); ui.key(vk.VK_RETURN)
        await ui.wait("Workspace settings","2227","1234 lines","debug",absent=["Edit workspace defaults"])
        assert tomllib.loads(cfg.read_text("utf8"))["settings"]["rclone_path"] == tool
        assert not (root / "termirs.log").exists()
        ui.key(ord("E"), "e")
        ctrl(ui,"a"); write(ui,"777")
        ui.key(vk.VK_ESCAPE)
        await ui.wait("Workspace settings","2227",absent=["777"])
        ui.key(vk.VK_F4); ui.key(ord("N"),"n")
        await ui.wait("New SSH Connection","2227")
        write(ui,"127.0.0.1")
        ui.key(vk.VK_TAB); ui.key(vk.VK_TAB)
        write(ui,"fixture")
        ctrl(ui,"s")
        await ui.wait("Connection List","127.0.0.1",absent=["New SSH Connection"])
    device, = tomllib.loads(cfg.read_text("utf8"))["connections"]
    assert device["port"] == 2227 and not device.get("public_key")
    logdir = root / "second"
    logdir.mkdir()
    async with tui(exe,cfg,"127.0.0.1",cwd=logdir) as ui:
        await settings(ui)
        await ui.wait("1234", "2227", "custom tools", "debug")
        for _ in range(3):
            ui.key(vk.VK_TAB)
        ui.key(vk.VK_RIGHT)
        ui.key(vk.VK_RETURN)
        await ui.wait("Workspace settings","off",absent=["Edit workspace defaults"])
    assert "Logging enabled at level: debug" in (logdir/"termirs.log").read_text("utf8")
    finaldir=root/"third"
    finaldir.mkdir()
    async with tui(exe,cfg,"127.0.0.1",cwd=finaldir) as ui:
        ui.key(vk.VK_F5)
        await ui.wait("Workspace settings","off","2227")
    assert not (finaldir/"termirs.log").exists()
    print("Defaults: invalid history blocked; saves/cancellation and custom tool survived restart. Offline device used 2227; next-launch debug/off applied.")


if __name__ == "__main__":
    os.environ.pop("RUST_LOG",None)
    with TemporaryDirectory(prefix="rrw-defaults-") as root:
        asyncio.run(run(Path(sys.argv[1]).resolve(strict=True),Path(root)))
