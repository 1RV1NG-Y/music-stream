//! Phone and PC in one process over real QUIC: the phone serves a library from a folder, the
//! PC browses and reads it through `Remote` (PLAN.md §5–§6).

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use musicstream_core::{
    Node, NodeConfig,
    library::{Library, Root},
    remote::{Remote, read_all},
};

async fn open(dir: &Path, name: &str, platform: &str) -> Node {
    Node::open(NodeConfig { data_dir: dir.to_owned(), device_name: name.into(), platform: platform.into(), port: 0, mdns: false })
        .await
        .unwrap()
}

async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !cond() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A file lofty accepts as FLAC: headers with tags, then `len` bytes of pseudo-random payload.
fn write_flac(path: &Path, title: &str, len: usize, seed: u8) {
    let mut f = b"fLaC".to_vec();
    let mut si = vec![0x10, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0];
    si.extend_from_slice(&((44100u64 << 44) | (1u64 << 41) | (15u64 << 36) | 441000).to_be_bytes());
    si.extend_from_slice(&[0; 16]);
    f.push(0x00);
    f.extend_from_slice(&(si.len() as u32).to_be_bytes()[1..]);
    f.extend_from_slice(&si);
    let mut vc = Vec::new();
    vc.extend_from_slice(&4u32.to_le_bytes());
    vc.extend_from_slice(b"test");
    let comments = [format!("TITLE={title}"), "ALBUM=Test".into()];
    vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for c in &comments {
        vc.extend_from_slice(&(c.len() as u32).to_le_bytes());
        vc.extend_from_slice(c.as_bytes());
    }
    f.push(0x84);
    f.extend_from_slice(&(vc.len() as u32).to_be_bytes()[1..]);
    f.extend_from_slice(&vc);
    let mut x: u32 = seed as u32 + 1;
    f.extend((0..len).map(|_| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x as u8
    }));
    std::fs::write(path, f).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pc_browses_and_streams_the_phones_library() {
    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let music = dirs[3].path();
    std::fs::create_dir(music.join("Album")).unwrap();
    write_flac(&music.join("Album/01.flac"), "One", 3_000_000, 1);
    write_flac(&music.join("Album/02.flac"), "Two", 700_000, 2);

    let phone = open(dirs[0].path(), "Phone", "android").await;
    let library = Arc::new(Library::open(dirs[0].path().join("music"), vec![Root { volume: "external_primary".into(), path: music.to_owned() }]).unwrap());
    phone.serve_library(library.clone());
    let pc = open(dirs[1].path(), "PC", "linux").await;
    pc.use_code(&phone.create_invite().unwrap().ticket).await.unwrap();
    let remote = Remote::open(pc.clone(), dirs[2].path().to_owned()).unwrap();
    wait_for("index arrives", || remote.index().tracks.len() == 2).await;
    assert_eq!(remote.status().source.as_deref(), Some("Phone"));

    let index = remote.index();
    let one = index.tracks.iter().find(|t| t.title == "One").unwrap().clone();
    let source_bytes = std::fs::read(music.join("Album/01.flac")).unwrap();

    // Random reads, then everything, match the file; seeks work before anything is cached.
    let no = Arc::new(AtomicBool::new(false));
    let (r1, r2) = {
        let remote = remote.clone();
        let one = one.clone();
        let no = no.clone();
        tokio::task::spawn_blocking(move || {
            let mut a = remote.open_track(&one, no.clone()).unwrap();
            let mut b = remote.open_track(&one, no).unwrap();
            let mut chunk = vec![0; 5000];
            a.seek(SeekFrom::Start(2_500_000)).unwrap();
            a.read_exact(&mut chunk).unwrap();
            let r1 = chunk.clone();
            b.seek(SeekFrom::Start(300_000)).unwrap();
            b.read_exact(&mut chunk).unwrap();
            a.seek(SeekFrom::Start(0)).unwrap();
            let all = read_all(&mut a).unwrap();
            assert_eq!(all.len() as u64, one.size());
            (r1, (chunk, all))
        })
        .await
        .unwrap()
    };
    assert_eq!(r1, source_bytes[2_500_000..2_505_000]);
    assert_eq!(r2.0, source_bytes[300_000..305_000]);
    assert!(r2.1 == source_bytes, "the whole track matches");
    wait_for("track cached", || remote.cached().contains(&(one.id, one.rev))).await;

    // With the phone gone, the cached track still plays.
    phone.shutdown().await;
    let cached = {
        let remote = remote.clone();
        let one = one.clone();
        tokio::task::spawn_blocking(move || read_all(&mut remote.open_track(&one, no).unwrap()).unwrap()).await.unwrap()
    };
    assert!(cached == source_bytes);
    pc.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_file_is_refused_and_a_stalled_read_can_be_interrupted() {
    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let music = dirs[3].path();
    let path = music.join("a.flac");
    write_flac(&path, "A", 2_000_000, 3);
    let phone = open(dirs[0].path(), "Phone", "android").await;
    phone.serve_library(Arc::new(Library::open(dirs[0].path().join("music"), vec![Root { volume: "v".into(), path: music.to_owned() }]).unwrap()));
    let pc = open(dirs[1].path(), "PC", "linux").await;
    pc.use_code(&phone.create_invite().unwrap().ticket).await.unwrap();
    let remote = Remote::open(pc.clone(), dirs[2].path().to_owned()).unwrap();
    wait_for("index arrives", || remote.index().tracks.len() == 1).await;
    let old = remote.index().tracks[0].clone();

    // Replaced on the phone before the PC reads it: the read fails, the index catches up.
    std::thread::sleep(Duration::from_millis(20));
    write_flac(&path, "A", 2_000_000, 4);
    let err = {
        let remote = remote.clone();
        let old = old.clone();
        tokio::task::spawn_blocking(move || read_all(&mut remote.open_track(&old, Arc::new(AtomicBool::new(false))).unwrap()).unwrap_err()).await.unwrap()
    };
    assert!(format!("{err:#}").contains("changed"), "{err:#}");
    wait_for("new rev in the index", || remote.index().tracks[0].rev != old.rev).await;

    // Phone gone and nothing cached: a read waits, and the player's interrupt releases it.
    phone.shutdown().await;
    let fresh = remote.index().tracks[0].clone();
    let interrupt = Arc::new(AtomicBool::new(false));
    let waiting = {
        let remote = remote.clone();
        let interrupt = interrupt.clone();
        tokio::task::spawn_blocking(move || {
            let mut s = remote.open_track(&fresh, interrupt).unwrap();
            s.read(&mut [0; 10]).unwrap_err().to_string()
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!waiting.is_finished(), "the read waits for the phone");
    interrupt.store(true, std::sync::atomic::Ordering::Release);
    let kind = tokio::time::timeout(Duration::from_millis(500), waiting).await.expect("released within 500 ms").unwrap();
    assert_eq!(kind, musicstream_core::remote::INTERRUPTED);
    pc.shutdown().await;
}
