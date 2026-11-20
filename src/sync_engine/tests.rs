use super::*;
use crate::config::manager::ConfigManager;
use std::fs as disk;

#[tokio::test]
#[ignore = "requires isolated AsyncSSH fixture: uv run tools/smoke_sync_engine.py <rclone>"]
async fn live_rclone_sftp() {
    let base = PathBuf::from(std::env::var_os("RRW_SYNC_FIXTURE").unwrap());
    let conf = ConfigManager::with_path(base.join("devices.toml")).unwrap();
    let conn = &conf.connections()[0];
    let rclone = PathBuf::from(std::env::var_os("RRW_TEST_RCLONE").unwrap());
    let local = base.join("local");
    let remote = base.join("sftp").join("data");
    let spec = SyncSpec {
        local_root: local.clone(),
        remote_root: "/data".into(),
        direction: Direction::Upload,
        excludes: vec!["build".into()],
    };
    let (tx, _rx) = mpsc::channel(8);
    macro_rules! scan {
        ($s:expr) => {
            preview_scoped(
                &rclone,
                conn,
                $s.clone(),
                Some(RemoteScope::confirm(conn, &$s, ScopeMode::ExclusiveRun).unwrap()),
                CancellationToken::new(),
                tx.clone(),
            )
            .await
            .unwrap()
        };
    }
    macro_rules! apply {
        ($p:expr,$a:expr) => {
            execute($p, $a, CancellationToken::new(), tx.clone()).await
        };
    }
    let mut wrong = conn.clone();
    wrong.public_key = Some(disk::read_to_string(base.join("different.pub")).unwrap());
    let bad = Rclone::new(&rclone, &wrong, &CancellationToken::new())
        .await
        .unwrap();
    disk::write(local.join("pin-probe"), "must not be transferred").unwrap();
    assert!(
        bad.copy(
            local.to_str().unwrap(),
            "device:/data",
            &["pin-probe"],
            false,
            &CancellationToken::new(),
            None
        )
        .await
        .is_err()
    );
    assert!(!remote.join("pin-probe").exists());
    disk::remove_file(local.join("pin-probe")).unwrap();
    let mut untrusted = conn.clone();
    untrusted.public_key = None;
    assert!(matches!(
        preview(
            &rclone,
            &untrusted,
            spec.clone(),
            CancellationToken::new(),
            tx.clone()
        )
        .await,
        Err(AppError::UnknownHostKey { .. })
    ));
    let mut invalid = spec.clone();
    invalid.remote_root = "/does-not-exist".into();
    assert!(
        preview_scoped(
            &rclone,
            conn,
            invalid.clone(),
            Some(RemoteScope::confirm(conn, &invalid, ScopeMode::ExclusiveRun).unwrap()),
            CancellationToken::new(),
            tx.clone()
        )
        .await
        .is_err()
    );

    disk::create_dir(local.join("子 目录")).unwrap();
    disk::create_dir(local.join("build")).unwrap();
    disk::write(local.join("build/skip.txt"), "skip").unwrap();
    disk::write(local.join("子 目录/alpha.txt"), "original").unwrap();
    disk::write(remote.join("sentinel"), "target only").unwrap();
    let p = scan!(spec);
    assert!(p.skipped >= 2);
    assert_eq!(p.entries.len(), 1);
    assert_eq!(p.entries[0].path, "子 目录/alpha.txt");
    drop(p);
    assert!(!remote.join("子 目录").exists());
    assert_eq!(disk::read_dir(&remote).unwrap().count(), 1);
    let p = scan!(spec);
    disk::write(local.join("after-preview"), "late").unwrap();
    assert_eq!(apply!(p, BTreeSet::new()).unwrap().copied, 1);
    assert_eq!(
        disk::read(remote.join("子 目录/alpha.txt")).unwrap(),
        b"original"
    );
    assert!(!remote.join("after-preview").exists());
    disk::remove_file(local.join("after-preview")).unwrap();
    assert!(scan!(spec).entries.is_empty());
    disk::write(local.join("子 目录/alpha.txt"), "modified").unwrap();
    let p = scan!(spec);
    assert_eq!(p.entries[0].kind, ChangeKind::Conflict);
    assert!(!p.entries[0].can_overwrite);
    assert_eq!(
        apply!(p, BTreeSet::from(["子 目录/alpha.txt".into()]))
            .unwrap()
            .copied,
        0
    );
    assert_eq!(
        disk::read(remote.join("子 目录/alpha.txt")).unwrap(),
        b"original"
    );
    disk::create_dir(remote.join("link-out")).unwrap();
    disk::write(remote.join("link-out/escape"), "would escape").unwrap();
    let down = SyncSpec {
        direction: Direction::Download,
        ..spec.clone()
    };
    let p = scan!(down);
    disk::write(local.join("子 目录/alpha.txt"), "raced!!!").unwrap();
    #[cfg(windows)]
    assert!(
        apply!(p, BTreeSet::from(["子 目录/alpha.txt".into()]))
            .unwrap_err()
            .to_string()
            .contains("changed since preview")
    );
    assert_eq!(
        disk::read(local.join("子 目录/alpha.txt")).unwrap(),
        b"raced!!!"
    );
    let p = scan!(down);
    let a = apply!(p, BTreeSet::from(["子 目录/alpha.txt".into()])).unwrap();
    #[cfg(windows)]
    {
        assert_eq!(a.copied, 2);
        assert_eq!(
            disk::read(local.join("子 目录/alpha.txt")).unwrap(),
            b"original"
        );
    }
    assert_eq!(disk::read(local.join("sentinel")).unwrap(), b"target only");
    assert!(!base.join("outside/escape").exists());
    disk::write(local.join("new"), "new").unwrap();
    let p = scan!(spec);
    disk::write(remote.join("new"), "competing writer").unwrap();
    assert!(
        apply!(p, BTreeSet::new())
            .unwrap_err()
            .to_string()
            .contains("changed since preview")
    );
    assert_eq!(disk::read(remote.join("new")).unwrap(), b"competing writer");

    disk::write(local.join("big"), vec![3; 4 * 1024 * 1024]).unwrap();
    let p = scan!(spec);
    let t = CancellationToken::new();
    let c = t.clone();
    disk::write(base.join("stall"), "").unwrap();
    let job = tokio::spawn(execute(p, BTreeSet::new(), t, tx.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !base.join("copy-started").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    c.cancel();
    disk::remove_file(base.join("stall")).unwrap();
    let err = job.await.unwrap().unwrap_err().to_string();
    assert!(err.contains("Cancel"), "{err}");
    assert!(!remote.join("big").exists());
    assert!(!disk::read_dir(&remote).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rrw-sync-")
    }));
    let done = b"small file already published";
    disk::write(local.join("00-done"), done).unwrap();
    let p = scan!(spec);
    disk::write(base.join("observe-upload"), "").unwrap();
    disk::write(base.join("disconnect-upload"), "").unwrap();
    let partial = apply!(p, BTreeSet::new()).unwrap_err().to_string();
    assert!(partial.contains("Published 1 file"), "{partial}");
    assert!(!remote.join("big").exists());
    assert_eq!(disk::read(remote.join("00-done")).unwrap(), done);
    disk::remove_file(base.join("disconnect-upload")).unwrap();
    let p = scan!(spec);
    assert!(!p.entries.iter().any(|e| e.path == "00-done"));
    assert_eq!(apply!(p, BTreeSet::new()).unwrap().copied, 1);
    assert_eq!(
        disk::read(remote.join("big")).unwrap(),
        vec![3; 4 * 1024 * 1024]
    );
    check_offsets(&base, "upload", done.len());

    // Downloads have separate read traffic for preview/content comparison.
    // The SFTP fixture records rclone payload reads only.
    let inbound = base.join("downloaded");
    let source = base.join("sftp/download-source");
    disk::create_dir(&inbound).unwrap();
    disk::create_dir(&source).unwrap();
    disk::write(source.join("00-done"), done).unwrap();
    disk::write(source.join("zz-big"), vec![7; 4 * 1024 * 1024]).unwrap();
    let download = SyncSpec {
        local_root: inbound.clone(),
        remote_root: "/download-source".into(),
        direction: Direction::Download,
        excludes: vec![],
    };
    let p = scan!(download);
    disk::write(base.join("observe-download"), "").unwrap();
    disk::write(base.join("disconnect-download"), "").unwrap();
    let partial = apply!(p, BTreeSet::new()).unwrap_err().to_string();
    assert!(partial.contains("Published 1 file"), "{partial}");
    assert_eq!(disk::read(inbound.join("00-done")).unwrap(), done);
    assert!(!inbound.join("zz-big").exists());
    disk::remove_file(base.join("disconnect-download")).unwrap();
    let p = scan!(download);
    assert_eq!(
        p.entries
            .iter()
            .map(|e| e.path.as_str())
            .collect::<Vec<_>>(),
        ["zz-big"]
    );
    assert_eq!(apply!(p, BTreeSet::new()).unwrap().copied, 1);
    assert_eq!(
        disk::read(inbound.join("zz-big")).unwrap(),
        vec![7; 4 * 1024 * 1024]
    );
    check_offsets(&base, "download", done.len());
}

fn check_offsets(base: &Path, direction: &str, done_len: usize) {
    let raw = disk::read_to_string(base.join(format!("{direction}.writes-or-reads"))).unwrap();
    let mut chunks = BTreeMap::<(&str, &str), Vec<(usize, usize)>>::new();
    for row in raw.lines() {
        let mut i = row.split_ascii_whitespace();
        chunks
            .entry((i.next().unwrap(), i.next().unwrap()))
            .or_default()
            .push((
                i.next().unwrap().parse().unwrap(),
                i.next().unwrap().parse().unwrap(),
            ));
    }
    let ranges = |mut v: Vec<(usize, usize)>| {
        v.sort_unstable();
        let mut end = 0;
        for (offset, len) in v {
            assert_eq!(offset, end, "overlap or hole in {direction} SFTP trace");
            end += len;
        }
        end
    };
    assert_eq!(ranges(chunks.remove(&("first", "done")).unwrap()), done_len);
    assert!(
        chunks.remove(&("retry", "done")).is_none(),
        "already published file was retransferred"
    );
    assert_eq!(
        ranges(chunks.remove(&("first", "big")).unwrap()),
        2 * 1024 * 1024
    );
    let retry = chunks.remove(&("retry", "big")).unwrap();
    assert_eq!(
        retry[0].0, 0,
        "retry's first SFTP request did not start at zero"
    );
    assert_eq!(ranges(retry), 4 * 1024 * 1024);
    assert!(chunks.is_empty());
    println!(
        "{direction}: first 0..2097152; retry 0..4194304; completed file {done_len} bytes on first transfer, 0 bytes retransferred"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_sync_staging_and_published_file_keep_private_permissions() {
    use std::process::Command;
    let work = tempfile::tempdir().unwrap();
    let stage = private_dir::PrivateDir::new_in(work.path()).unwrap();
    let payload = stage.path().join("payload");
    disk::write(&payload, "new contents").unwrap();
    let dest = work.path().join("restricted");
    disk::write(&dest, "old").unwrap();
    let acl = |p: &Path, setup: &str| {
        let code = format!(
            "$ErrorActionPreference='Stop'; $p=$env:RRW_ACL_PATH; {setup}; $a=if ([IO.Directory]::Exists($p)) {{[IO.Directory]::GetAccessControl($p)}} else {{[IO.File]::GetAccessControl($p)}}; $a.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)"
        );
        let r = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &code])
            .env("RRW_ACL_PATH", p)
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        String::from_utf8_lossy(&r.stdout).trim().to_string()
    };
    let private = acl(stage.path(), "");
    assert!(
        private.starts_with("D:P") && private.contains(";;;OW)") && private.contains(";;;SY)"),
        "{private}"
    );
    assert_eq!(private.matches("(A;").count(), 2, "{private}");
    let child = acl(&payload, "");
    assert!(
        !child.contains(";;;WD)") && !child.contains(";;;BU)"),
        "{child}"
    );
    let original = acl(
        &dest,
        "$a=[IO.File]::GetAccessControl($p); $a.SetAccessRuleProtection($true,$false); $id=[Security.Principal.WindowsIdentity]::GetCurrent().User; $a.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($id,'FullControl','Allow')); [IO.File]::SetAccessControl($p,$a)",
    );
    let c = CancellationToken::new();
    let v = fs::hash_reader(tokio::fs::File::open(&dest).await.unwrap(), &c)
        .await
        .unwrap();
    {
        let _writer = disk::OpenOptions::new().write(true).open(&dest).unwrap();
        assert!(
            publish::publish_local(&payload, &dest, Some(&v), &c)
                .await
                .is_err()
        );
        assert_eq!(disk::read(&dest).unwrap(), b"old");
    }
    publish::publish_local(&payload, &dest, Some(&v), &c)
        .await
        .unwrap();
    assert_eq!(disk::read(&dest).unwrap(), b"new contents");
    let after = acl(&dest, "");
    assert!(after.starts_with("D:P") && original.starts_with("D:P"));
    // AI is Windows bookkeeping about prior automatic inheritance; the protected DACL and every ACE must match.
    assert_eq!(
        after.split_once('(').unwrap().1,
        original.split_once('(').unwrap().1
    );
}

#[tokio::test]
async fn remote_auth_is_bound_to_account_key_and_root_and_transient_is_one_use() {
    use crate::config::manager::AuthMethod;
    use russh::keys::ssh_key::{PublicKey, public::Ed25519PublicKey};
    let key = |n| {
        PublicKey::from(Ed25519PublicKey([n; 32]))
            .to_openssh()
            .unwrap()
    };
    let mut c = Connection::new("127.0.0.1".into(), 1, "user".into(), AuthMethod::None);
    c.public_key = Some(key(1));
    let spec = SyncSpec {
        local_root: PathBuf::from("never-read"),
        remote_root: "/subtree".into(),
        direction: Direction::Download,
        excludes: vec![],
    };
    assert!(RemoteScope::confirm(&c, &spec, ScopeMode::JailedRoot).is_err());
    let lease = RemoteScope::confirm(&c, &spec, ScopeMode::ExclusiveRun).unwrap();
    assert!(lease.matches(&c, &spec));
    let disk: RemoteScope = toml::from_str(&toml::to_string(&lease).unwrap()).unwrap();
    assert!(!disk.matches(&c, &spec));
    let (tx, _rx) = mpsc::channel(1);
    let denied = preview(
        Path::new("must-not-run"),
        &c,
        spec.clone(),
        CancellationToken::new(),
        tx,
    )
    .await
    .unwrap_err();
    assert!(
        denied.to_string().contains("authorization required"),
        "{denied}"
    );
    let copy = lease.clone();
    lease.claim(&c, &spec).unwrap();
    assert!(copy.claim(&c, &spec).is_err());
    let mut jailed = spec.clone();
    jailed.remote_root = "/".into();
    let attestation = RemoteScope::confirm(&c, &jailed, ScopeMode::JailedRoot).unwrap();
    let saved: RemoteScope = toml::from_str(&toml::to_string(&attestation).unwrap()).unwrap();
    assert!(saved.matches(&c, &jailed));
    assert!(!saved.matches(&c, &spec));
    for mut different in [c.clone(), c.clone(), c.clone(), c.clone(), c.clone()]
        .into_iter()
        .enumerate()
    {
        match different.0 {
            0 => different.1.id.push('2'),
            1 => different.1.host = "elsewhere".into(),
            2 => different.1.port = 2,
            3 => different.1.username = "other".into(),
            _ => different.1.public_key = Some(key(2)),
        }
        assert!(!saved.matches(&different.1, &jailed));
    }
}
