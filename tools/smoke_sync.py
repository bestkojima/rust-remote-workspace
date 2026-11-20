# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312; sys_platform == 'win32'", "psutil==7.1.3"]
# ///
"""Drive F3 through a real Windows console, rclone and a disposable SFTP server."""
import argparse
import json
import asyncio
from pathlib import Path
import sys
import tomllib
from tempfile import TemporaryDirectory
import asyncssh
import win32con as vk
import win32console as con
import win32api
from smoke_host_key import profile, public_line, server, tui
from smoke_files import goto
from measure_workspace import measure_working_set

def key(ui, ch):
    ui.key(ord(ch.upper()), ch)

def enter(ui):
    ui.key(vk.VK_RETURN, "\r")

def esc(ui):
    ui.key(vk.VK_ESCAPE, "\x1b")

def tab(ui):
    ui.key(vk.VK_TAB, "\t")

def type_text(ui, s):
    for ch in s:
        code=win32api.VkKeyScan(ch)
        ui.key(code & 255 if code!=-1 else 0xE7,ch)

def select_text(ui):
    events=[]
    for down in [True, False]:
        e=con.PyINPUT_RECORDType(con.KEY_EVENT)
        e.KeyDown,e.RepeatCount,e.VirtualKeyCode,e.Char,e.ControlKeyState=down,1,ord("A"),"\x01",vk.LEFT_CTRL_PRESSED
        events.append(e)
    ui.input.WriteConsoleInput(events)

async def tool(ui, rclone):
    key(ui,"r"); await ui.wait("Sync tool","rclone executable")
    select_text(ui); type_text(ui,str(rclone)); enter(ui)
    await ui.wait("Folder sync",absent=["Sync tool"])
    cfg=Path(ui.proc.args[2])
    assert tomllib.loads(cfg.read_text("utf-8"))["settings"]["rclone_path"]==str(rclone),repr(tomllib.loads(cfg.read_text("utf-8"))["settings"]["rclone_path"])

async def scope(ui):
    enter(ui)
    await ui.wait("Authorize remote directory","Exclusive access for this run")

async def exclusive(ui):
    await scope(ui)
    key(ui,"e")

async def exercise_open_files(ui,local_root,remote_root,rclone,*,remote_base="/"):
    """Start with Files open; ends at the F3 profile list. All writes stay in new fixture folders."""
    local,remote=Path(local_root)/"pair source",Path(remote_root)/"pair target"
    local.mkdir();remote.mkdir()
    (local/"aa conflict.txt").write_text("source differs",encoding="utf8")
    (remote/"aa conflict.txt").write_text("keep remote",encoding="utf8")
    (local/"bb selected.txt").write_bytes(b"chosen\n")
    (remote/"zz sentinel").touch()
    (local/"target").mkdir()
    (local/"target"/"hidden").touch()
    (local/"notes.omit").touch()
    await goto(ui,local)
    tab(ui)
    await goto(ui,remote_base.rstrip("/")+"/pair target")
    ui.key(vk.VK_F7);await ui.wait("Sync profile","pair source","pair target")
    # Save a user exclusion, keeping suggested directory excludes.
    for _ in range(5):tab(ui)
    type_text(ui,"; *.omit");enter(ui)
    await ui.wait("Folder sync","pair source","pair target")
    await tool(ui,rclone)
    print("Upload fixture: 4 source files (14 + 7 + 0 + 0 bytes), 2 target files (11 + 0 bytes), 2 exclusions.")
    async with measure_working_set(ui.proc.pid, "Sync scan"):
        await exclusive(ui); await ui.wait("Copy preview","aa conflict.txt","bb selected.txt","KEEP","NEW")
    key(ui," "); await ui.wait("Selected: 1", "Replacements: 0", "Original kept")
    enter(ui); await ui.wait("Confirm copy","Files: 1","Replacements: 0","Bytes: 7","pair source","pair target")
    assert not (remote/"bb selected.txt").exists() and not (remote/"target").exists()
    esc(ui);await ui.wait("Copy preview")
    esc(ui);await ui.wait("Folder sync")
    assert not (remote/"bb selected.txt").exists()
    # Frozen manifest: a new file created after scan is not silently included.
    await exclusive(ui);await ui.wait("Copy preview","bb selected.txt")
    (local/"new after scan").touch()
    enter(ui);await ui.wait("Confirm copy")
    async with measure_working_set(ui.proc.pid,"Sync copy"):
        key(ui,"y")
        await ui.wait("Sync result","Finished","1 copied")
    assert (remote/"bb selected.txt").read_bytes()==(local/"bb selected.txt").read_bytes()
    assert (remote/"aa conflict.txt").read_text()=="keep remote"
    assert (remote/"zz sentinel").exists() and not (remote/"notes.omit").exists() and not (remote/"new after scan").exists()
    assert not (remote/"target").exists()
    enter(ui);await ui.wait("Folder sync")
    (local/"new after scan").unlink()
    await exclusive(ui);await ui.wait("Copy preview","1 unchanged","Selected: 0")
    esc(ui);await ui.wait("Folder sync")
    # Reverse the direction and select a specific replacement into Windows.
    key(ui,"e");await ui.wait("Sync profile")
    for _ in range(4):tab(ui)
    ui.key(vk.VK_RIGHT);enter(ui)
    await ui.wait("Folder sync","Download")
    await exclusive(ui);await ui.wait("Copy preview","aa conflict.txt","zz sentinel","KEEP")
    key(ui," ");await ui.wait("REPLACE","Replacements: 1")
    enter(ui);await ui.wait("Confirm copy","Replacements: 1","pair source")
    # New target version invalidates the selected overwrite.
    (local/"aa conflict.txt").write_text("changed after review",encoding="utf8")
    key(ui,"y");await ui.wait("Sync result","Failed")
    assert (local/"aa conflict.txt").read_text()=="changed after review"
    enter(ui);await ui.wait("Folder sync")
    await exclusive(ui);await ui.wait("Copy preview","aa conflict.txt")
    key(ui," ");enter(ui);await ui.wait("Confirm copy","Replacements: 1")
    key(ui,"y");await ui.wait("Sync result","Finished")
    assert (local/"aa conflict.txt").read_text()=="keep remote" and (local/"zz sentinel").exists()
    enter(ui);await ui.wait("Folder sync")
    key(ui,"h");await ui.wait("Run history","Finished","Failed","Preview");esc(ui)
    await ui.wait("Folder sync")
    print("Sync UI: picked real folders, saved filter, reviewed each path, skipped remote conflict, rejected stale local overwrite, approved a fresh replacement.")

async def run(exe,rclone,root):
    config=root/"devices.toml"
    local,remote=root/"local",root/"remote";local.mkdir();remote.mkdir()
    host=asyncssh.generate_private_key("ssh-ed25519")
    client=asyncssh.generate_private_key("ssh-ed25519")
    identity=root/"test.key";identity.write_bytes(client.export_private_key("openssh"))
    pub=client.convert_to_public()
    opened=[]
    probe=root/"rclone-probe.cmd"
    probe.write_text('@echo tool-started>"%~dp0unexpected-tool"\n',encoding="ascii")
    async with server(host,0,pub,remote,on_sftp_open=lambda:opened.append(1)) as (port,auth):
        config.write_text(profile(port,identity,"Trust fixture")+"[settings]\ndefault_port = 22\nconnection_timeout = 10\nrclone_path="+json.dumps(str(probe))+"\n",encoding="utf8")
        async with tui(exe,config) as ui:
            # First contact from Sync, not Terminal/Files. An offline profile is sufficient.
            ui.key(vk.VK_F3);key(ui,"n");await ui.wait("Sync profile")
            type_text(ui,"draft");tab(ui);tab(ui);type_text(ui,str(local));tab(ui);type_text(ui,"/");enter(ui)
            await ui.wait("Folder sync","draft")
            before=config.read_bytes()
            enter(ui);await ui.wait("Verify SSH host key",host.get_fingerprint("sha256"));assert not auth
            esc(ui);await ui.wait("Folder sync",absent=["Verify SSH host key"]);assert not auth and config.read_bytes()==before
            enter(ui);await ui.wait("Verify SSH host key");key(ui,"y");await ui.wait("Host key saved");esc(ui)
            before=config.read_bytes()
            await scope(ui);enter(ui);await ui.wait("Authorize remote directory","Save administrator-verified jail")
            esc(ui);await ui.wait("Folder sync","draft")
            assert config.read_bytes()==before and not opened and not (root/"unexpected-tool").exists()
            await scope(ui);key(ui,"x");await ui.wait("Folder sync",absent=["Authorize remote directory"])
            assert not opened and not (root/"unexpected-tool").exists()
            await tool(ui,rclone)
            await scope(ui);key(ui,"j");await ui.wait("Copy preview")
            esc(ui);await ui.wait("Saved jail grant")
            assert tomllib.loads(config.read_text("utf8"))["sync_profiles"][0]["saved_jail"]["mode"]=="JailedRoot"
        async with tui(exe,config) as ui:
            ui.key(vk.VK_F3);await ui.wait("Saved jail grant");enter(ui);await ui.wait("Copy preview")
            esc(ui);key(ui,"u");await ui.wait("Folder sync",absent=["Saved jail grant"])
            assert "saved_jail" not in tomllib.loads(config.read_text("utf8"))["sync_profiles"][0]
            await scope(ui);key(ui,"j");await ui.wait("Copy preview");esc(ui)
            # A different remote directory, even under the same jail, gets no inherited grant.
            key(ui,"e");await ui.wait("Sync profile")
            for _ in range(3):tab(ui)
            select_text(ui);type_text(ui,"/different");enter(ui)
            await ui.wait("Folder sync","/different",absent=["Saved jail grant"])
            before=len(opened);await scope(ui)
            key(ui,"j");await ui.wait("Authorize remote directory",absent=["Save administrator-verified jail"])
            esc(ui);await ui.wait("Folder sync")
            assert len(opened)==before and "saved_jail" not in tomllib.loads(config.read_text("utf8"))["sync_profiles"][0]
            key(ui,"d");await ui.wait("Remove sync profile");esc(ui)
            key(ui,"d");key(ui,"y");await ui.wait("No folder pairs")
            ui.key(vk.VK_F2);enter(ui);await ui.wait("SFTP File Transfer")
            await exercise_open_files(ui,local,remote,rclone)
        saved=tomllib.loads(config.read_text(encoding="utf8"))
        p,=saved["sync_profiles"]
        assert p["direction"]=="Download" and p["remote_root"]=="/pair target" and "*.omit" in p["excludes"]
        assert p["deletion"]=="KeepTarget" and p["mode"]=="ManualCopy" and 1<=len(p["history"])<=12
        assert saved["settings"]["rclone_path"]==str(rclone) and "saved_jail" not in p
        async with tui(exe,config) as ui:
            ui.key(vk.VK_F3);await ui.wait("Folder sync","pair source","Download")
            key(ui,"h");await ui.wait("Run history","Finished");esc(ui)
            # Save a second independent pair without touching either directory.
            key(ui,"n");await ui.wait("Sync profile");type_text(ui,"Invalid root")
            tab(ui);tab(ui);type_text(ui,str(local/"missing directory"));tab(ui);type_text(ui,"/");enter(ui)
            await ui.wait("Folder sync","Invalid root","pair source")
            await exclusive(ui);await ui.wait("Sync result","Failed","No files written")
            enter(ui);key(ui,"d");await ui.wait("Remove sync profile","Invalid root")
            key(ui,"y");await ui.wait("Folder sync","pair source",absent=["Invalid root"])
            # Pending scan cancellation is allowed, no approved plan leaks into a retry.
            await exclusive(ui);key(ui,"x");await ui.wait("Sync result","Cancelled")
            enter(ui);await ui.wait("Folder sync");ui.key(vk.VK_F4);await ui.wait("Connection List")
    async with server(asyncssh.generate_private_key("ssh-ed25519"),port,pub,remote) as (_,auth):
        async with tui(exe,config) as ui:
            ui.key(vk.VK_F3);enter(ui);await ui.wait("Host key changed",host.get_fingerprint("sha256"))
            assert not auth
            esc(ui);ui.key(vk.VK_F4);await ui.wait("Connection List")
    print("Sync trust, scope cancellation (no SFTP/tool), per-run exclusive consent, persistent jail binding, restart, revocation and profile history: passed.")

if __name__=="__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("exe",type=Path);p.add_argument("rclone",type=Path)
    a=p.parse_args()
    with TemporaryDirectory(prefix="rrw-sync-ui-") as tmp, asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner:
        runner.run(run(a.exe.resolve(strict=True),a.rclone.resolve(strict=True),Path(tmp)))
