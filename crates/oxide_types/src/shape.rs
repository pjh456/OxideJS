use std::sync::{Arc, Mutex, OnceLock};

use hashbrown::HashMap;

pub type ShapeId = u32;
pub type StringIndex = u32;

pub const EMPTY_SHAPE_ID: ShapeId = 1;
const EMPTY_SENTINEL: StringIndex = u32::MAX;

#[derive(Debug, Clone)]
pub struct Shape {
    pub id: ShapeId,
    pub property_name: StringIndex,
    pub property_offset: u8,
    pub parent: Option<ShapeId>,
}

struct ShapeStore {
    shapes: Vec<Option<Arc<Shape>>>,
    transitions: HashMap<(ShapeId, StringIndex), ShapeId>,
    next_id: ShapeId,
}

impl ShapeStore {
    fn new() -> Self {
        let mut store = Self {
            shapes: Vec::with_capacity(256),
            transitions: HashMap::with_capacity(256),
            next_id: 1,
        };
        store.create_empty_shape();
        store
    }

    fn create_empty_shape(&mut self) {
        let empty = Arc::new(Shape {
            id: EMPTY_SHAPE_ID,
            property_name: EMPTY_SENTINEL,
            property_offset: 0,
            parent: None,
        });
        debug_assert_eq!(self.shapes.len(), 0);
        self.shapes.push(Some(empty));
        self.next_id = 2;
    }

    fn get_shape(&self, id: ShapeId) -> Option<Arc<Shape>> {
        self.shapes.get((id - 1) as usize)?.clone()
    }

    fn make_shape(&mut self, parent_id: ShapeId, prop_name: StringIndex) -> ShapeId {
        let key = (parent_id, prop_name);
        if let Some(&existing) = self.transitions.get(&key) {
            return existing;
        }

        let prop_offset = self.compute_offset(parent_id);
        let new_id = self.next_id;
        self.next_id += 1;

        let shape = Arc::new(Shape {
            id: new_id,
            property_name: prop_name,
            property_offset: prop_offset,
            parent: Some(parent_id),
        });

        while self.shapes.len() < new_id as usize {
            self.shapes.push(None);
        }
        self.shapes[(new_id - 1) as usize] = Some(shape);
        self.transitions.insert(key, new_id);

        new_id
    }

    fn compute_offset(&self, shape_id: ShapeId) -> u8 {
        let mut count = 0u8;
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match self.get_shape(id) {
                Some(s) => {
                    if s.property_name != EMPTY_SENTINEL {
                        count += 1;
                    }
                    cursor = s.parent;
                }
                None => break,
            }
        }
        count.min(31)
    }

    fn lookup_offset(&self, shape_id: ShapeId, prop_name: StringIndex) -> Option<u8> {
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match self.get_shape(id) {
                Some(s) => {
                    if s.property_name == prop_name && s.property_name != EMPTY_SENTINEL {
                        return Some(s.property_offset);
                    }
                    cursor = s.parent;
                }
                None => return None,
            }
        }
        None
    }

    fn shape_prop_count(&self, shape_id: ShapeId) -> u8 {
        let mut count = 0u8;
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match self.get_shape(id) {
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

static SHAPE_STORE: OnceLock<Mutex<ShapeStore>> = OnceLock::new();

fn store() -> &'static Mutex<ShapeStore> {
    SHAPE_STORE.get_or_init(|| Mutex::new(ShapeStore::new()))
}

pub fn get_shape(id: ShapeId) -> Option<Arc<Shape>> {
    store().lock().unwrap().get_shape(id)
}

pub fn make_shape(parent_id: ShapeId, prop_name: StringIndex) -> ShapeId {
    store().lock().unwrap().make_shape(parent_id, prop_name)
}

pub fn lookup_offset(shape_id: ShapeId, prop_name: StringIndex) -> Option<u8> {
    store().lock().unwrap().lookup_offset(shape_id, prop_name)
}

pub fn shape_prop_count(shape_id: ShapeId) -> u8 {
    store().lock().unwrap().shape_prop_count(shape_id)
}

pub fn init_shape_store() {
    let _ = store();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_shape_exists() {
        let s = get_shape(EMPTY_SHAPE_ID);
        assert!(s.is_some());
        let s = s.unwrap();
        assert_eq!(s.id, EMPTY_SHAPE_ID);
        assert!(s.parent.is_none());
    }

    #[test]
    fn make_shape_creates_new_id() {
        let key: StringIndex = 1_000_000;
        let s1 = make_shape(EMPTY_SHAPE_ID, key);
        assert!(s1 > EMPTY_SHAPE_ID);
        let shape = get_shape(s1).unwrap();
        assert_eq!(shape.property_name, key);
        assert_eq!(shape.parent, Some(EMPTY_SHAPE_ID));
    }

    #[test]
    fn hash_cons_returns_same_id() {
        let key: StringIndex = 1_000_001;
        let a = make_shape(EMPTY_SHAPE_ID, key);
        let b = make_shape(EMPTY_SHAPE_ID, key);
        assert_eq!(a, b);
    }

    #[test]
    fn different_props_different_ids() {
        let a = make_shape(EMPTY_SHAPE_ID, 1_000_002);
        let b = make_shape(EMPTY_SHAPE_ID, 1_000_003);
        assert_ne!(a, b);
    }

    #[test]
    fn chain_of_three() {
        let s1 = make_shape(EMPTY_SHAPE_ID, 1_000_010);
        let s2 = make_shape(s1, 1_000_020);
        let s3 = make_shape(s2, 1_000_030);

        assert_eq!(shape_prop_count(s3), 3);

        assert_eq!(lookup_offset(s3, 1_000_030), Some(2));
        assert_eq!(lookup_offset(s3, 1_000_020), Some(1));
        assert_eq!(lookup_offset(s3, 1_000_010), Some(0));
        assert_eq!(lookup_offset(s3, 99), None);
    }

    #[test]
    fn two_branches_share_ancestor() {
        let base = make_shape(EMPTY_SHAPE_ID, 1_000_040);
        let branch_a = make_shape(base, 1_000_050);
        let branch_b = make_shape(base, 1_000_060);
        assert_ne!(branch_a, branch_b);
        let a_shape = get_shape(branch_a).unwrap();
        let b_shape = get_shape(branch_b).unwrap();
        assert_eq!(a_shape.parent, Some(base));
        assert_eq!(b_shape.parent, Some(base));
    }

    #[test]
    fn edge_same_structure_different_names() {
        let s1 = make_shape(EMPTY_SHAPE_ID, 1_000_070);
        let s2 = make_shape(s1, 1_000_080);
        let s3 = make_shape(EMPTY_SHAPE_ID, 1_000_090);
        let s4 = make_shape(s3, 1_000_100);
        assert_ne!(s1, s3);
        assert_ne!(s2, s4);
        assert_eq!(lookup_offset(s2, 1_000_070), Some(0));
        assert_eq!(lookup_offset(s4, 1_000_090), Some(0));
    }
}
