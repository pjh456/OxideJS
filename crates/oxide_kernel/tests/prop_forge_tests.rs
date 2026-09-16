use oxide_kernel::prop_forge::{PropForge, PropTemplate};

#[test]
fn test_get_nonexistent() {
    let forge = PropForge::new();
    assert!(forge.get_template(999).is_none());
}

#[test]
fn test_upsert_and_get() {
    let forge = PropForge::new();
    let t = PropTemplate {
        shape_id: 1,
        prop_name: 11,
        position: 3,
        generation: 10,
    };
    forge.upsert(1, t);
    let got = forge.get_template(1).unwrap();
    assert_eq!(got.shape_id, 1);
    assert_eq!(got.prop_name, 11);
    assert_eq!(got.position, 3);
    assert_eq!(got.generation, 10);
}

#[test]
fn test_upsert_if_better_high_gen_wins() {
    let forge = PropForge::new();
    forge.upsert_if_better(
        1,
        PropTemplate {
            shape_id: 1,
            prop_name: 11,
            position: 3,
            generation: 10,
        },
    );
    forge.upsert_if_better(
        1,
        PropTemplate {
            shape_id: 1,
            prop_name: 12,
            position: 7,
            generation: 5,
        },
    );
    let got = forge.get_template(1).unwrap();
    assert_eq!(got.generation, 10);
}

#[test]
fn test_upsert_if_better_low_replaced() {
    let forge = PropForge::new();
    forge.upsert_if_better(
        1,
        PropTemplate {
            shape_id: 1,
            prop_name: 11,
            position: 3,
            generation: 5,
        },
    );
    forge.upsert_if_better(
        1,
        PropTemplate {
            shape_id: 1,
            prop_name: 12,
            position: 7,
            generation: 10,
        },
    );
    let got = forge.get_template(1).unwrap();
    assert_eq!(got.generation, 10);
    assert_eq!(got.position, 7);
}
