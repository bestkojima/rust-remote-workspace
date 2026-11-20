# /// script
# requires-python = ">=3.12"
# dependencies = ["asyncssh==2.24.1"]
# ///
"""Run the engine against an isolated SFTP chroot and the specified pinned rclone."""
import asyncio, json, os, sys, tempfile, subprocess
from pathlib import Path
import asyncssh

async def run(exe):
    with tempfile.TemporaryDirectory(prefix="rrw-sync-test-") as d:
        base=Path(d)
        (base/"sftp/data").mkdir(parents=True)
        (base/"local").mkdir()
        outside=base/"outside";outside.mkdir()
        (outside/"secret.txt").write_text("outside fixture",encoding="utf-8")
        if os.name=="nt":
            import _winapi
            _winapi.CreateJunction(str(outside),str(base/"local/link-out"))
        else:
            (base/"local/link-out").symlink_to(outside,target_is_directory=True)
        host=asyncssh.generate_private_key("ssh-ed25519")
        key=asyncssh.generate_private_key("ssh-ed25519")
        (base/"different.pub").write_bytes(asyncssh.generate_private_key("ssh-ed25519").export_public_key())
        identity=base/"identity"
        identity.write_bytes(key.export_private_key("openssh"))
        class Server(asyncssh.SSHServer):
            def public_key_auth_supported(self): return True
            def validate_public_key(self,u,k): return u=="fixture" and k==key.convert_to_public()
        half=2*1024*1024
        transferred={"upload":0,"download":0}
        class SFTP(asyncssh.SFTPServer):
            def __init__(self,chan):
                super().__init__(chan,chroot=str(base/"sftp"));self.chan=chan
            def cut(self,direction,large):
                # Disconnect before the next packet after 2 MiB accepted/served.
                if large and (base/f"disconnect-{direction}").exists() and transferred[direction]>=half:
                    self.chan.get_connection().abort()
                    raise asyncssh.SFTPFailure("test disconnect after partial transfer")
            def record(self,direction,large,off,n):
                if not n or not (base/f"observe-{direction}").exists(): return
                phase="first" if (base/f"disconnect-{direction}").exists() else "retry"
                if phase=="first" and large: transferred[direction]+=n
                with (base/f"{direction}.writes-or-reads").open("a",encoding="ascii") as trace:
                    print(phase,"big" if large else "done",off,n,file=trace)
            async def write(self,f,off,data):
                if (base/"stall").exists():
                    (base/"copy-started").touch()
                    while (base/"stall").exists(): await asyncio.sleep(.025)
                # Uploads use a per-file staging name; distinguish the two
                # fixture files by their known contents, never log payloads.
                large=data.startswith(b"\x03")
                self.cut("upload",large)
                n=super().write(f,off,data)
                self.record("upload",large,off,n)
                return n
            def read(self,f,off,size):
                # Do not count russh's independent version/preview reads as
                # rclone downloads. Only the payload stream answers the question.
                rclone="rclone" in self.chan.get_connection().get_extra_info("client_version","").lower()
                large=Path(os.fsdecode(f.name)).name=="zz-big"
                if rclone: self.cut("download",large)
                data=super().read(f,off,size)
                if rclone: self.record("download",large,off,len(data))
                return data
        srv=await asyncssh.create_server(Server,"127.0.0.1",0,server_host_keys=[host],sftp_factory=SFTP,encoding=None)
        try:
            (base/"devices.toml").write_text(f"""
[[connections]]
id="device"
display_name="fixture"
host="127.0.0.1"
port={srv.get_port()}
username="fixture"
auth_method={{public_key={{private_key_path={json.dumps(str(identity))}}}}}
public_key={json.dumps(host.export_public_key().decode().strip())}
created_at="2026-01-01T00:00:00Z"
[settings]
default_port=22
connection_timeout=10
""",encoding="utf-8")
            env=dict(os.environ,RRW_SYNC_FIXTURE=str(base),RRW_TEST_RCLONE=exe)
            proc=await asyncio.to_thread(subprocess.run,["cargo","test","--locked","--lib","sync_engine::tests::live_rclone_sftp","--","--ignored","--exact","--nocapture"],env=env)
            return proc.returncode
        finally:
            srv.close();await srv.wait_closed()

if __name__=="__main__":
    with asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner: raise SystemExit(runner.run(run(str(Path(sys.argv[1]).resolve()))))
