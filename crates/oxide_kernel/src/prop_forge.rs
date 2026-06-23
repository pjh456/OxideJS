use std::sync::Arc;

use dashmap::DashMap;

use crate::shape_forge::ShapeId;

#[derive(Debug, Clone)]
pub struct PropTemplate {
    pub shape_id: ShapeId,
    pub prop_name: u32,
    pub position: u32,
    pub generation: u32,
}

pub struct PropForge {
    templates: DashMap<ShapeId, Arc<PropTemplate>>,
}

impl PropForge {
    pub fn new() -> Self {
        Self { templates: DashMap::new() }
    }

    pub fn get_template(&self, shape_id: ShapeId) -> Option<Arc<PropTemplate>> {
        self.templates.get(&shape_id).map(|r| Arc::clone(&*r))
    }

    pub fn upsert(&self, shape_id: ShapeId, template: PropTemplate) {
        self.templates.insert(shape_id, Arc::new(template));
    }

    pub fn upsert_if_better(&self, shape_id: ShapeId, template: PropTemplate) {
        use dashmap::mapref::entry::Entry;

        match self.templates.entry(shape_id) {
            Entry::Occupied(mut e) => {
                if e.get().generation < template.generation {
                    e.insert(Arc::new(template));
                }
            }
            Entry::Vacant(e) => {
                e.insert(Arc::new(template));
            }
        }
    }

    pub fn clear(&self) {
        self.templates.clear();
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

impl Default for PropForge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
