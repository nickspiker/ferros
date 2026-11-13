//! The compile-and-run probe for Photon's substrate on ferros: tokio's multi-thread runtime, kete over the kernel's vault scope, fgtw and vsf linked in. Spawn: `ferros-bridge spawn stdprobe.bin --name stdprobe --entropy --store test` (the kernel has no `photon` scope now).

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

    // The house KEMs round-trip: ML-KEM-768 through PQClean's C, built freestanding.
    {
        use pqcrypto_traits::kem::SharedSecret;
        let t = std::time::Instant::now();
        let (pk, sk) = pqcrypto_mlkem::mlkem768::keypair();
        let (ss_a, ct) = pqcrypto_mlkem::mlkem768::encapsulate(&pk);
        let ss_b = pqcrypto_mlkem::mlkem768::decapsulate(&ct, &sk);
        println!("mlkem768: shared secrets {} in A#{} us", if ss_a.as_bytes() == ss_b.as_bytes() { "agree" } else { "DIFFER" }, t.elapsed().as_micros());
    }

    // The heap, end to end: a pattern written through 96 MiB and read back, so a bad region of the big arc pool shows here rather than as a mystery elsewhere (STD-TARGET.md).
    {
        let t = std::time::Instant::now();
        let n = 96usize << 20;
        let mut v: Vec<u64> = Vec::with_capacity(n / 8);
        for i in 0..n / 8 {
            v.push((i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA5A5_5A5A_0F0F_F0F0);
        }
        let bad = v.iter().enumerate().filter(|&(i, &x)| x != (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA5A5_5A5A_0F0F_F0F0).count();
        let at = v.as_ptr() as usize;
        println!("memtest: 96 MiB at G#{at:X}, A#{bad} bad words, A#{} ms", t.elapsed().as_millis());
    }

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
    // A map cell's size (Mahere's base cell on the Pro, 2026-11-12), written and read in bulk as Mahere keeps its cells: the vault's own line says how many transfers the read took.
    let cell: Vec<u8> = (0..1_654_438u32).map(|i| (i * 13 % 251) as u8).collect();
    let t = std::time::Instant::now();
    store.write_device_bulk(&[("probe.cell".to_string(), cell.clone())]).unwrap();
    let wrote = t.elapsed().as_millis();
    let t = std::time::Instant::now();
    let back = store.read_device_bulk("probe.cell").unwrap();
    println!("kete: cell A#{} bytes written in A#{wrote} ms, read in A#{} ms {}", cell.len(), t.elapsed().as_millis(), if back.as_deref() == Some(&cell[..]) { "exact" } else { "WRONG" });
    store.delete_device("probe.cell").unwrap();
    // Values the size of a kist's capsule (2026-11-13: Mahere's is 53 MB, Photon's 91 MB), written and read whole thru the vault, each compared by BLAKE3: the vault process takes them a transfer at a time and must hold no value whole.
    for mib in [32usize, 96] {
        let n = mib << 20;
        let value: Vec<u8> = (0..n).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let want = blake3::hash(&value);
        let t = std::time::Instant::now();
        let wrote = store.write_device_bulk(&[("probe.capsule".to_string(), value)]);
        let wrote_ms = t.elapsed().as_millis();
        let t = std::time::Instant::now();
        let back = store.read_device_bulk("probe.capsule");
        let read_ms = t.elapsed().as_millis();
        let verdict = match (&wrote, &back) {
            (Err(e), _) => format!("write FAILED: {e}"),
            (_, Err(e)) => format!("read FAILED: {e}"),
            (_, Ok(None)) => "read ABSENT".to_string(),
            (_, Ok(Some(b))) if b.len() == n && blake3::hash(b) == want => "exact".to_string(),
            (_, Ok(Some(b))) => format!("WRONG (A#{} bytes back)", b.len()),
        };
        println!("kete: A#{mib} MiB value written in A#{wrote_ms} ms, read in A#{read_ms} ms: {verdict}");
        drop(back);
        if let Err(e) = store.delete_device("probe.capsule") {
            println!("kete: delete of the A#{mib} MiB value FAILED: {e}");
        }
    }
    let addrs = store.live_addrs().unwrap();
    println!("kete: A#{} live addrs", addrs.len());
    store.delete_device("probe.big").unwrap();
    store.delete_device("probe.small").unwrap();
    println!("kete: after delete A#{} live addrs, small {:?}; A#{} ms; exit 0", store.live_addrs().unwrap().len(), store.read_device("probe.small").unwrap().map(|b| b.len()), t1.elapsed().as_millis());
}
