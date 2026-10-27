//! The compile-and-run probe for Photon's substrate on ferros: tokio's multi-thread runtime, kete over the kernel's vault scope, fgtw and vsf linked in. Spawn: `ferros-bridge spawn stdprobe.bin --name stdprobe --entropy --store photon`.

fn main() {
    let t0 = std::time::Instant::now();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_time().build().unwrap();
    let n = rt.block_on(async {
        let mut set = tokio::task::JoinSet::new();
        for i in 0..8u64 {
            set.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10 * i)).await;
                i * i
            });
        }
        let mut sum = 0;
        while let Some(r) = set.join_next().await {
            sum += r.unwrap();
        }
        sum
    });
    println!("tokio: sum of squares A#{n} (expect 140) in A#{} ms", t0.elapsed().as_millis());
    println!("blake3 of 'ferros': {}", &blake3::hash(b"ferros").to_hex()[..16]);

    let t1 = std::time::Instant::now();
    let app = kete::App { id: "stdprobe", dir: "stdprobe" };
    let store = match kete::FlatStorage::open_device_shared(app, [7u8; 32]) {
        Ok(s) => s,
        Err(e) => {
            println!("kete: open failed: {e}");
            return;
        }
    };
    println!("kete: device vault open in A#{} ms", t1.elapsed().as_millis());
    let big: Vec<u8> = (0..10_000u32).map(|i| (i * 7 % 251) as u8).collect();
    store.write_device("probe.small", b"hello vault").unwrap();
    store.write_device("probe.big", &big).unwrap();
    let small = store.read_device("probe.small").unwrap();
    let back = store.read_device("probe.big").unwrap();
    println!("kete: small {:?}, big A#{} bytes {}", small.as_deref().map(|b| String::from_utf8_lossy(b).into_owned()), back.as_ref().map_or(0, |b| b.len()), if back.as_deref() == Some(&big[..]) { "exact" } else { "WRONG" });
    let addrs = store.live_addrs().unwrap();
    println!("kete: A#{} live addrs", addrs.len());
    store.delete_device("probe.big").unwrap();
    store.delete_device("probe.small").unwrap();
    println!("kete: after delete A#{} live addrs, small {:?}; A#{} ms; exit 0", store.live_addrs().unwrap().len(), store.read_device("probe.small").unwrap().map(|b| b.len()), t1.elapsed().as_millis());
}
