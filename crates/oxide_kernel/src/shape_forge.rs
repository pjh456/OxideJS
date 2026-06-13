use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use dashmap::DashMap;

use crate::kernel_debug;

pub type ShapeId = u32;
pub type StringIndex = u32;

pub const EMPTY_SHAPE_ID: ShapeId = 1;
const EMPTY_SENTINEL: StringIndex = u32::MAX;

#[derive(Debug, Clone)]
pub struct Shape {
    pub id: ShapeId,
    pub property_name: StringIndex,
    pub parent: Option<ShapeId>,
}

pub struct ShapeForge {
    shapes: RwLock<Vec<Option<Arc<Shape>>>>,
    transitions: DashMap<u64, ShapeId>,
    next_id: AtomicU32,
}

impl ShapeForge {
    pub fn new() -> Self {
        let forge = Self {
            shapes: RwLock::new(Vec::with_capacity(256)),
            transitions: DashMap::with_capacity(256),
            next_id: AtomicU32::new(2),
        };
        {
            let mut shapes = forge.shapes.write().unwrap();
            let empty = Arc::new(Shape {
                id: EMPTY_SHAPE_ID,
                property_name: EMPTY_SENTINEL,
                parent: None,
            });
            debug_assert_eq!(shapes.len(), 0);
            shapes.push(Some(empty));
        }
        forge
    }

    pub fn pack_key(parent_id: ShapeId, prop_name: StringIndex) -> u64 {
        ((parent_id as u64) << 32) | (prop_name as u64)
    }

    fn compute_depth(shape_id: ShapeId, shapes: &[Option<Arc<Shape>>]) -> u32 {
        if shape_id == 0 {
            return 0;
        }
        let mut count = 0u32;
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match shapes.get((id - 1) as usize).and_then(|s| s.clone()) {
                Some(s) => {
                    if s.property_name != EMPTY_SENTINEL {
                        count += 1;
                    }
                    cursor = s.parent;
                }
                None => break,
            }
        }
        count
    }

    pub fn make_shape(&self, parent_id: ShapeId, prop_name: StringIndex) -> ShapeId {
        let key = Self::pack_key(parent_id, prop_name);

        if let Some(entry) = self.transitions.get(&key) {
            kernel_debug!("ShapeForge transition cached parent={} prop={} -> id={}", parent_id, prop_name, *entry);
            return *entry;
        }

        let new_id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let shape = Arc::new(Shape {
            id: new_id,
            property_name: prop_name,
            parent: Some(parent_id),
        });

        {
            let mut shapes = self.shapes.write().unwrap();
            while shapes.len() < new_id as usize {
                shapes.push(None);
            }
            shapes[(new_id - 1) as usize] = Some(shape);
        }

        let entry = self.transitions.entry(key).or_insert(new_id);
        let id = *entry.value();
        if id == new_id {
            kernel_debug!("ShapeForge transition parent={} prop={} -> id={}", parent_id, prop_name, new_id);
        } else {
            kernel_debug!("ShapeForge transition cached parent={} prop={} -> id={}", parent_id, prop_name, id);
        }
        id
    }

    pub fn get_shape(&self, id: ShapeId) -> Option<Arc<Shape>> {
        let shapes = self.shapes.read().unwrap();
        shapes.get((id - 1) as usize).and_then(|s| s.clone())
    }

    pub fn lookup_position(&self, shape_id: ShapeId, prop_name: StringIndex) -> Option<u32> {
        let shapes = self.shapes.read().unwrap();
        let total_depth = Self::compute_depth(shape_id, &shapes);
        let mut prop_steps: u32 = 0;
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match shapes.get((id - 1) as usize).and_then(|s| s.clone()) {
                Some(s) => {
                    if s.property_name != EMPTY_SENTINEL {
                        if s.property_name == prop_name {
                            return total_depth.checked_sub(prop_steps + 1);
                        }
                        prop_steps += 1;
                    }
                    cursor = s.parent;
                }
                None => return None,
            }
        }
        None
    }

    pub fn has_property(&self, shape_id: ShapeId, prop_name: StringIndex) -> bool {
        let shapes = self.shapes.read().unwrap();
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match shapes.get((id - 1) as usize).and_then(|s| s.clone()) {
                Some(s) => {
                    if s.property_name == prop_name && s.property_name != EMPTY_SENTINEL {
                        return true;
                    }
                    cursor = s.parent;
                }
                None => return false,
            }
        }
        false
    }

    pub fn shape_prop_count(&self, shape_id: ShapeId) -> u32 {
        let shapes = self.shapes.read().unwrap();
        let mut count = 0u32;
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match shapes.get((id - 1) as usize).and_then(|s| s.clone()) {
                Some(s) => {
                    if s.property_name != EMPTY_SENTINEL {
                        count += 1;
                    }
                    cursor = s.parent;
                }
                None => break,
            }
        }
        count
    }
}

impl Default for ShapeForge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn empty_shape_exists() {
        let forge = ShapeForge::new();
        let s = forge.get_shape(EMPTY_SHAPE_ID);
        assert!(s.is_some());
        let s = s.unwrap();
        assert_eq!(s.id, EMPTY_SHAPE_ID);
        assert!(s.parent.is_none());
    }

    #[test]
    fn make_shape_creates_new_id() {
        let forge = ShapeForge::new();
        let key: StringIndex = 1_000_000;
        let s1 = forge.make_shape(EMPTY_SHAPE_ID, key);
        assert!(s1 > EMPTY_SHAPE_ID);
        let shape = forge.get_shape(s1).unwrap();
        assert_eq!(shape.property_name, key);
        assert_eq!(shape.parent, Some(EMPTY_SHAPE_ID));
    }

    #[test]
    fn hash_cons_returns_same_id() {
        let forge = ShapeForge::new();
        let key: StringIndex = 1_000_001;
        let a = forge.make_shape(EMPTY_SHAPE_ID, key);
        let b = forge.make_shape(EMPTY_SHAPE_ID, key);
        assert_eq!(a, b);
    }

    #[test]
    fn different_props_different_ids() {
        let forge = ShapeForge::new();
        let a = forge.make_shape(EMPTY_SHAPE_ID, 1_000_002);
        let b = forge.make_shape(EMPTY_SHAPE_ID, 1_000_003);
        assert_ne!(a, b);
    }

    #[test]
    fn chain_of_three() {
        let forge = ShapeForge::new();
        let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_010);
        let s2 = forge.make_shape(s1, 1_000_020);
        let s3 = forge.make_shape(s2, 1_000_030);

        assert_eq!(forge.shape_prop_count(s3), 3);

        assert_eq!(forge.lookup_position(s3, 1_000_030), Some(2));
        assert_eq!(forge.lookup_position(s3, 1_000_020), Some(1));
        assert_eq!(forge.lookup_position(s3, 1_000_010), Some(0));
        assert_eq!(forge.lookup_position(s3, 99), None);
    }

    #[test]
    fn two_branches_share_ancestor() {
        let forge = ShapeForge::new();
        let base = forge.make_shape(EMPTY_SHAPE_ID, 1_000_040);
        let branch_a = forge.make_shape(base, 1_000_050);
        let branch_b = forge.make_shape(base, 1_000_060);
        assert_ne!(branch_a, branch_b);
        let a_shape = forge.get_shape(branch_a).unwrap();
        let b_shape = forge.get_shape(branch_b).unwrap();
        assert_eq!(a_shape.parent, Some(base));
        assert_eq!(b_shape.parent, Some(base));
    }

    #[test]
    fn edge_same_structure_different_names() {
        let forge = ShapeForge::new();
        let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_070);
        let s2 = forge.make_shape(s1, 1_000_080);
        let s3 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_090);
        let s4 = forge.make_shape(s3, 1_000_100);
        assert_ne!(s1, s3);
        assert_ne!(s2, s4);
        assert_eq!(forge.lookup_position(s2, 1_000_070), Some(0));
        assert_eq!(forge.lookup_position(s4, 1_000_090), Some(0));
    }

    #[test]
    fn concurrent_make_same_key() {
        let forge = Arc::new(ShapeForge::new());
        let key: StringIndex = 1_000_200;
        let barrier = Arc::new(Barrier::new(2));

        let f1 = Arc::clone(&forge);
        let b1 = Arc::clone(&barrier);
        let h1 = std::thread::spawn(move || {
            b1.wait();
            f1.make_shape(EMPTY_SHAPE_ID, key)
        });

        let f2 = Arc::clone(&forge);
        let b2 = Arc::clone(&barrier);
        let h2 = std::thread::spawn(move || {
            b2.wait();
            f2.make_shape(EMPTY_SHAPE_ID, key)
        });

        let id1 = h1.join().unwrap();
        let id2 = h2.join().unwrap();
        assert_eq!(id1, id2);
        assert!(id1 > EMPTY_SHAPE_ID);
    }

    #[test]
    fn has_property_works() {
        let forge = ShapeForge::new();
        let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_300);
        let s2 = forge.make_shape(s1, 1_000_310);
        assert!(forge.has_property(s2, 1_000_300));
        assert!(forge.has_property(s2, 1_000_310));
        assert!(!forge.has_property(s2, 99));
    }
}
