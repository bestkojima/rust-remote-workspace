# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["asyncssh==2.24.1", "pywin32==312; sys_platform == 'win32'"]
# ///
"""Native Windows Files controls against disposable local and chroot SFTP roots."""
import asyncio
from pathlib import Path
import os
import sys
from tempfile import TemporaryDirectory

import asyncssh
import ntsecuritycon
import win32api
import win32con as vk
import win32security as sec
from smoke_host_key import profile, public_line, server, tui


def press(ui, ch):
    ui.key(0xE7, ch)


def enter_text(ui, value):
    for ch in value:
        # VK_PACKET supplies Unicode (not an arbitrary virtual-key number).
        ui.key(0xE7, ch)
    ui.key(vk.VK_RETURN, "\r")


async def dialog(ui, key, title, value):
    press(ui, key)
    await ui.wait(title)
    enter_text(ui, value)
    await ui.wait("SFTP File Transfer", absent=[title, "Updating directory"])


async def goto(ui, path):
    await dialog(ui, "g", "Go to directory", str(path))


async def choose(ui, name):
    press(ui, "/")
    enter_text(ui, name)
    await ui.wait(name)


async def copy_to_other(ui):
    press(ui, "c")
    await ui.wait("[COPY MODE]")
    ui.key(vk.VK_TAB, "\t")
    press(ui, "v")
    await ui.wait("SFTP Transfers", "Completed", "Press Enter or Esc")
    ui.key(vk.VK_RETURN, "\r")
    await ui.wait("SFTP File Transfer", absent=["SFTP Transfers"])


async def exercise_open_files(ui, local_root, remote_root):
    """From an open Files browser (left local, right SSH), verify both directions."""
    local = Path(local_root) / "f4 local"
    remote = Path(remote_root) / "f4 remote"
    local.mkdir(); remote.mkdir()
    (local / "download").mkdir()
    name = "数据 sample.txt"
    renamed = "renamed 文件.txt"
    source = ("可靠的往返传输\n"*30).encode()
    (local / name).write_bytes(source)
    (remote / name).write_bytes(b"prior destination")
    await goto(ui, local)
    await ui.wait("数据", absent=["Error"])
    await choose(ui, name)
    await ui.wait(f"{len(source)} B")
    ui.key(vk.VK_ESCAPE)
    ui.key(vk.VK_TAB, "\t")
    await goto(ui, "/f4 remote")
    # A failed path lookup must leave the currently browsed directory intact.
    await goto(ui, "/no-such-f4-directory")
    await ui.wait("Error")
    ui.key(vk.VK_ESCAPE)
    await ui.wait("/f4 remote", absent=["Error"])
    await dialog(ui, "m", "Create directory", "inbox")
    assert (remote / "inbox").is_dir()
    ui.key(vk.VK_RIGHT)
    await ui.wait("/f4 remote/inbox")
    ui.key(vk.VK_LEFT)
    await ui.wait("inbox", absent=["/f4 remote/inbox"])
    ui.key(vk.VK_TAB, "\t")
    await choose(ui, name)
    await copy_to_other(ui)
    assert (remote / name).read_bytes() == source
    await choose(ui, name)
    await dialog(ui, "R", "Rename entry", renamed)
    assert not (remote / name).exists()
    assert (remote / renamed).read_bytes() == source
    ui.key(vk.VK_TAB, "\t")
    await goto(ui, local / "download")
    ui.key(vk.VK_TAB, "\t")
    await choose(ui, renamed)
    await copy_to_other(ui)
    assert (local / "download" / renamed).read_bytes() == source
    ui.key(vk.VK_TAB, "\t")
    await choose(ui, renamed)
    press(ui, "d")
    await ui.wait("Delete File", renamed)
    ui.key(vk.VK_ESCAPE)
    await ui.wait("SFTP File Transfer", absent=["Delete File"])
    assert (remote / renamed).read_bytes() == source
    press(ui, "d")
    await ui.wait("Delete File")
    press(ui, "y")
    await ui.wait("SFTP File Transfer", absent=["Delete File", "Updating directory"])
    assert not (remote / renamed).exists()
    assert (local / name).read_bytes() == source
    print("Files: absolute path, invalid path recovery, metadata, mkdir, enter/parent, Unicode upload, rename, download and delete/cancel passed.")


async def run(exe, root):
    local, remote = root / "local", root / "remote"
    local.mkdir(); remote.mkdir()
    host = asyncssh.generate_private_key("ssh-ed25519")
    client = asyncssh.generate_private_key("ssh-ed25519")
    identity = root / "client.key"
    identity.write_bytes(client.export_private_key())
    peer, peer_key = root / "peer", asyncssh.generate_private_key("ssh-ed25519")
    (peer / "peer-tree" / "nested").mkdir(parents=True)
    (peer / "peer-tree" / "nested" / "peer.txt").write_bytes(b"only on second machine")
    slow_read = asyncio.Event()
    release_reads = asyncio.Event()


    class Server(asyncssh.SSHServer):
        def public_key_auth_supported(self): return True
        def validate_public_key(self, user, key): return user == "fixture" and key == client.convert_to_public()

    class ControlledSFTP(asyncssh.SFTPServer):
        async def read(self, file_obj, offset, size):
            if os.fsdecode(file_obj.name).endswith("slow.bin"):
                slow_read.set()
                await release_reads.wait()
            return super().read(file_obj, offset, size)

    async def shell(proc):
        proc.stdout.write("file-fixture-shell\n")
        async for line in proc.stdin:
            proc.stdout.write(f"echo:{line.strip()}\n")

    async with (
        asyncssh.listen("127.0.0.1", 0, config=None, server_factory=Server, server_host_keys=[host],
                        process_factory=shell, sftp_factory=lambda ch: ControlledSFTP(ch, chroot=os.fsencode(remote))) as listener,
        server(peer_key, 0, client.convert_to_public(), peer) as (peer_port,_),
    ):
        config = root / "profile.toml"
        config.write_text(profile(listener.get_port(), identity, "Files fixture", public_line(host)) + "\n" +
                          profile(peer_port, identity, "Peer fixture", public_line(peer_key)) +
                          "\n[settings]\ndefault_port=22\nconnection_timeout=10\n", encoding="utf-8")
        async with tui(exe, config, "Files fixture", cwd=local) as ui:
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("file-fixture-shell")
            ui.key(vk.VK_F12)
            ui.key(vk.VK_F2)
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("SFTP File Transfer", "Empty directory")
            ui.key(vk.VK_TAB, "\t")
            for ch in ("c", "d", "R", "o"):
                press(ui, ch)
            await ui.wait("Empty directory", absent=["Delete File", "Error", "Rename entry"])
            ui.key(vk.VK_TAB, "\t")
            await exercise_open_files(ui, local, remote)
            # Hold the server's reads after the destination sibling is created.
            (remote / "f4 remote" / "slow.bin").write_bytes(b"x" * (1024 * 1024))
            await goto(ui, "/f4 remote")
            dest = local / "f4 local" / "download"
            protected = dest / "slow.bin"
            protected.write_bytes(b"existing destination")
            user = sec.GetTokenInformation(sec.OpenProcessToken(win32api.GetCurrentProcess(), vk.TOKEN_QUERY), sec.TokenUser)[0]
            dacl = sec.ACL()
            dacl.AddAccessAllowedAceEx(sec.ACL_REVISION, 0, ntsecuritycon.FILE_ALL_ACCESS, user)
            sec.SetNamedSecurityInfo(str(protected), sec.SE_FILE_OBJECT,
                                     sec.DACL_SECURITY_INFORMATION | sec.PROTECTED_DACL_SECURITY_INFORMATION,
                                     None, None, dacl, None)
            def rights():
                sd = sec.GetNamedSecurityInfo(str(protected),sec.SE_FILE_OBJECT,sec.DACL_SECURITY_INFORMATION)
                acl = sd.GetSecurityDescriptorDacl()
                # Compare effective ACEs and protection, not inheritance-history bits.
                aces = [(flags,mask,sec.ConvertSidToStringSid(sid))
                        for flags,mask,sid in (acl.GetAce(i) for i in range(acl.GetAceCount()))]
                return bool(sd.GetSecurityDescriptorControl()[0] & sec.SE_DACL_PROTECTED), aces
            before_acl = rights()
            before_names = set(dest.iterdir())
            await choose(ui, "slow.bin")
            press(ui, "c"); await ui.wait("[COPY MODE]")
            ui.key(vk.VK_TAB, "\t")
            press(ui, "v")
            await asyncio.wait_for(slow_read.wait(), 15)
            await ui.wait("SFTP Transfers", "Cancel transfer")
            ui.key(vk.VK_ESCAPE)
            await ui.wait("Cancelled", "Press Enter or Esc")
            release_reads.set()
            assert (dest / "slow.bin").read_bytes() == b"existing destination"
            assert set(dest.iterdir()) == before_names
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("SFTP File Transfer", absent=["SFTP Transfers"])
            ui.key(vk.VK_TAB, "\t")
            await choose(ui, "slow.bin")
            await copy_to_other(ui)
            assert (dest / "slow.bin").read_bytes() == (remote / "f4 remote" / "slow.bin").read_bytes()
            after_acl = rights()
            assert after_acl == before_acl, (before_acl, after_acl)
            assert set(dest.iterdir()) == before_names, set(dest.iterdir()) ^ before_names
            # A queued source disappears before paste: preserve the existing
            # remote file, report the failure, and remove the staging sibling.
            vanished = dest / "vanished.txt"
            vanished.write_bytes(b"source")
            sentinel = remote / "f4 remote" / vanished.name
            sentinel.write_bytes(b"keep this")
            await goto(ui, dest)
            await choose(ui, vanished.name)
            press(ui, "c"); await ui.wait("[COPY MODE]")
            vanished.unlink()
            ui.key(vk.VK_TAB, "\t")
            press(ui, "v")
            await ui.wait("Error", "FAILED")
            assert sentinel.read_bytes() == b"keep this"
            assert not list(sentinel.parent.glob(".rrw-part-*"))
            ui.key(vk.VK_ESCAPE)
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("SFTP File Transfer", absent=["SFTP Transfers"])
            # Same path, different identity. Cancel keeps the selection; a
            # successful switch of its origin must discard it.
            (remote / "collision.txt").write_bytes(b"from first server")
            (peer / "collision.txt").write_bytes(b"from other server")
            await goto(ui,"/")
            await choose(ui,"collision.txt")
            press(ui,"c"); await ui.wait("[COPY MODE] collision.txt")
            press(ui,"s"); await ui.wait("Select Right Pane Source")
            ui.key(vk.VK_ESCAPE)
            await ui.wait("[COPY MODE] collision.txt",absent=["Select Right Pane Source"])
            press(ui,"s"); await ui.wait("Select Right Pane Source")
            ui.key(vk.VK_DOWN);ui.key(vk.VK_RETURN,"\r")
            await ui.wait("peer-tree",absent=["[COPY MODE]", "Select Right Pane Source"])
            ui.key(vk.VK_TAB,"\t");press(ui,"v")
            await ui.wait("No files selected")
            assert not (dest / "collision.txt").exists()
            ui.key(vk.VK_ESCAPE);ui.key(vk.VK_TAB,"\t")
            press(ui,"s");await ui.wait("Select Right Pane Source")
            ui.key(vk.VK_UP);ui.key(vk.VK_RETURN,"\r")
            await ui.wait("f4 remote",absent=["Select Right Pane Source","peer-tree"])
            await choose(ui,"collision.txt");press(ui,"c");await ui.wait("[COPY MODE] collision.txt")
            # Switching just the destination keeps the original SSH source.
            ui.key(vk.VK_TAB, "\t")
            press(ui, "s")
            await ui.wait("Select Left Pane Source", "Peer fixture")
            ui.key(vk.VK_DOWN)
            ui.key(vk.VK_RETURN, "\r")
            await ui.wait("peer-tree", "[COPY MODE]", absent=["Select Left Pane Source"])
            press(ui,"v")
            await ui.wait("SFTP Transfers","Completed","Press Enter or Esc")
            ui.key(vk.VK_RETURN,"\r");await ui.wait("SFTP File Transfer",absent=["SFTP Transfers"])
            assert (peer/"collision.txt").read_bytes() == b"from first server"
            ui.key(vk.VK_TAB,"\t");await goto(ui,"/f4 remote");ui.key(vk.VK_TAB,"\t")
            await choose(ui, "peer-tree")
            await copy_to_other(ui)
            assert (remote / "f4 remote" / "peer-tree" / "nested" / "peer.txt").read_bytes() == b"only on second machine"
            await choose(ui, "slow.bin")
            await copy_to_other(ui)
            assert (peer / "slow.bin").read_bytes() == (remote / "f4 remote" / "slow.bin").read_bytes()
            press(ui, "q")
            ui.key(vk.VK_F1)
            enter_text(ui, "still-open")
            await ui.wait("echo:still-open")
            ui.key(vk.VK_F12)
            await ui.wait("F4 Devices")
            print("Cancelled in-flight read retained the destination; 1 MiB retry preserved the private Windows DACL. A failed upload left no partials. Two SFTP peers exchanged a directory and a file; the parked SSH shell still responded.")
    release_reads.set()


if __name__ == "__main__":
    with TemporaryDirectory(prefix="rrw-f4-") as tmp:
        asyncio.run(run(Path(sys.argv[1]).resolve(), Path(tmp)))

