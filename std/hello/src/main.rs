//! The first std program on ferros. Spawned as any payload: `ferros-bridge spawn stdhello.bin --name stdhello --entropy`; everything it prints is the console's.

use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

thread_local! {
    // A non-zero const initialiser lives in .tdata: the value proves the TLS image landed where the linker said.
    static MARK: Cell<u64> = const { Cell::new(0x5EED_1234_ABCD_0001) };
    static HITS: Cell<u32> = const { Cell::new(0) };
}

fn main() {
    let t0 = Instant::now();
    println!("hello from std on ferros");
    println!("tls mark G#{:X} (expect 5EED1234ABCD0001)", MARK.with(|m| m.get()));
    MARK.with(|m| m.set(7));

    // Threads: each sums its own range, bumps its own thread-local, and reports the main thread's mark untouched.
    let shared = Arc::new((Mutex::new(0u64), Condvar::new()));
    let mut handles = Vec::new();
    for i in 0..4u64 {
        let shared = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            let sum: u64 = (i * 1000..(i + 1) * 1000).sum();
            HITS.with(|h| h.set(h.get() + 1));
            let mark = MARK.with(|m| m.get());
            let (lock, cv) = &*shared;
            *lock.lock().unwrap() += sum;
            cv.notify_all();
            (i, sum, mark, HITS.with(|h| h.get()))
        }));
    }
    for h in handles {
        let (i, sum, mark, hits) = h.join().unwrap();
        println!("thread {i}: sum A#{sum}, its tls mark G#{mark:X}, hits A#{hits}");
    }
    let (lock, cv) = &*shared;
    let total = cv.wait_while(lock.lock().unwrap(), |t| *t == 0).unwrap();
    println!("total A#{} (expect 7998000), main's mark now A#{}", *total, MARK.with(|m| m.get()));

    // Detached threads: their stacks must come back through the reaper.
    for _ in 0..8 {
        thread::spawn(|| HITS.with(|h| h.set(1)));
    }
    thread::sleep(Duration::from_millis(50));

    // A word count over a Vec: nine distinct words, a linear scan is the lookup.
    let mut words: Vec<(&str, u32)> = Vec::new();
    for w in "the quick brown fox jumps over the lazy dog the end".split(' ') {
        match words.iter_mut().find(|(k, _)| *k == w) {
            Some((_, n)) => *n += 1,
            None => words.push((w, 1)),
        }
    }
    println!("words: 'the' A#{} times, A#{} distinct", words.iter().find(|(k, _)| *k == "the").map_or(0, |(_, n)| *n), words.len());

    let before = Instant::now();
    thread::sleep(Duration::from_millis(100));
    let slept = before.elapsed();
    println!("slept A#{} ms (asked 100)", slept.as_millis());
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => println!("utc A#{} s since 1970", d.as_secs()),
        Err(_) => println!("utc before 1970?"),
    }
    println!("done in A#{} us; exit 0", t0.elapsed().as_micros());
}
