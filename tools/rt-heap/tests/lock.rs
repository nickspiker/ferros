//! lock.rs over the Linux futex: sixteen threads (more than the cores, so holders are preempted while others wait) each add to a counter that only the lock protects; the sum must be exact.

use rt_heap::lock::Lock;

#[test]
fn sixteen_threads_one_counter() {
    static COUNT: Lock<u64> = Lock::new(0);
    const EACH: u64 = 1 << 18;
    std::thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| {
                for _ in 0..EACH {
                    let mut c = COUNT.lock();
                    // A read, a pause, a write: lost updates would show if two held it at once.
                    let v = *c;
                    core::hint::black_box(&v);
                    *c = v + 1;
                }
            });
        }
    });
    assert_eq!(*COUNT.lock(), 16 * EACH);
    assert!(COUNT.try_lock().is_some());
    let held = COUNT.lock();
    assert!(COUNT.try_lock().is_none());
    drop(held);
}
