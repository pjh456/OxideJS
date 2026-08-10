use std::time::Instant;

use oxide_types::mem::{Epoch, P};

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
    if cfg!(debug_assertions) {
        println!("1M allocations took {}ms (debug build - skipping timing assertion)", elapsed.as_millis());
    } else {
        assert!(
            elapsed.as_millis() < 200,
            "1M allocations took {}ms, expected <200ms",
            elapsed.as_millis()
        );
    }
}

#[test]
fn persistent_new_and_deref() {
    let p = P::new(42i32);
    assert_eq!(*p, 42);
}

#[test]
fn persistent_survives_epoch_reset() {
    let mut epoch = Epoch::new();
    let p = P::new(100i32);

    epoch.reset();

    assert_eq!(*p, 100);
}

#[test]
fn persistent_clone_shares_data() {
    let a = P::new(42i32);
    let b = a.clone();

    assert_eq!(*a, 42);
    assert_eq!(*b, 42);

    let a_ptr = &*a as *const i32;
    let b_ptr = &*b as *const i32;
    assert_eq!(a_ptr, b_ptr, "cloned P<T> should point to same data");
}

#[test]
fn persistent_custom_type() {
    #[derive(Debug, PartialEq)]
    struct Data {
        name: String,
        value: i32,
    }

    let p = P::new(Data {
        name: "test".to_string(),
        value: 42,
    });

    assert_eq!(p.name, "test");
    assert_eq!(p.value, 42);
}

#[test]
fn persistent_debug_format() {
    let p = P::new(42i32);
    assert_eq!(format!("{:?}", p), "P(42)");
}

#[test]
fn persistent_display_format() {
    let p = P::new(42i32);
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
fn epoch_alloc_with_closure() {
    let epoch = Epoch::new();
    let ptr = epoch.alloc_with(|| 99i32);
    assert_eq!(unsafe { *ptr }, 99);
}
