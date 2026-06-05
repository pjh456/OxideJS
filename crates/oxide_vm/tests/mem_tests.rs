use std::time::Instant;

use oxide_types::mem::{Epoch, PersistentHeap};

#[test]
fn epoch_alloc_and_read() {
    let epoch = Epoch::new();
    let ptr = epoch.alloc(42i32);
    assert_eq!(unsafe { *ptr }, 42);
}

#[test]
fn epoch_alloc_and_write() {
    let epoch = Epoch::new();
    let ptr = epoch.alloc(0i32);
    unsafe {
        *ptr = 99;
        assert_eq!(*ptr, 99);
    }
}

#[test]
fn epoch_reset_increments_id() {
    let mut epoch = Epoch::new();
    assert_eq!(epoch.current_id(), 0);
    epoch.reset();
    assert_eq!(epoch.current_id(), 1);
    epoch.reset();
    assert_eq!(epoch.current_id(), 2);
}

#[test]
fn epoch_benchmark_1m_allocations() {
    let epoch = Epoch::new();
    let start = Instant::now();

    for i in 0..1_000_000u64 {
        let ptr = epoch.alloc(i);
        unsafe {
            assert_eq!(*ptr, i);
        }
    }

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 100,
        "1M allocations took {}ms, expected <100ms",
        elapsed.as_millis()
    );
}

#[test]
fn persistent_promote_and_deref() {
    let heap = PersistentHeap::new();
    let p = heap.promote(42i32);
    assert_eq!(*p, 42);
}

#[test]
fn persistent_promote_survives_epoch_reset() {
    let heap = PersistentHeap::new();
    let mut epoch = Epoch::new();
    let p = heap.promote(100i32);

    epoch.reset();

    assert_eq!(*p, 100);
}

#[test]
fn persistent_clone_shares_data() {
    let heap = PersistentHeap::new();
    let a = heap.promote(42i32);
    let b = a.clone();

    assert_eq!(*a, 42);
    assert_eq!(*b, 42);

    let a_ptr = &*a as *const i32;
    let b_ptr = &*b as *const i32;
    assert_eq!(a_ptr, b_ptr, "cloned P<T> should point to same data");
}

#[test]
fn persistent_promote_custom_type() {
    #[derive(Debug, PartialEq)]
    struct Data {
        name: String,
        value: i32,
    }

    let heap = PersistentHeap::new();
    let p = heap.promote(Data {
        name: "test".to_string(),
        value: 42,
    });

    assert_eq!(p.name, "test");
    assert_eq!(p.value, 42);
}

#[test]
fn persistent_debug_format() {
    let heap = PersistentHeap::new();
    let p = heap.promote(42i32);
    assert_eq!(format!("{:?}", p), "P(42)");
}

#[test]
fn persistent_display_format() {
    let heap = PersistentHeap::new();
    let p = heap.promote(42i32);
    assert_eq!(format!("{}", p), "42");
}

#[test]
fn epoch_default_creates_valid() {
    let epoch = Epoch::default();
    assert_eq!(epoch.current_id(), 0);
    let ptr = epoch.alloc(7i32);
    assert_eq!(unsafe { *ptr }, 7);
}

#[test]
fn persistent_heap_default() {
    let heap = PersistentHeap;
    let p = heap.promote("hello");
    assert_eq!(*p, "hello");
}

#[test]
fn epoch_alloc_with_closure() {
    let epoch = Epoch::new();
    let ptr = epoch.alloc_with(|| 99i32);
    assert_eq!(unsafe { *ptr }, 99);
}
