use std::sync::Arc;

use dashmap::DashMap;

use crate::shape_forge::ShapeId;

#[derive(Debug, Clone)]
pub struct IcTemplate {
    pub shape_id: ShapeId,
    pub offset: u8,
    pub generation: u32,
}

pub struct IcForge {
    templates: DashMap<ShapeId, Arc<IcTemplate>>,
}

impl IcForge {
    pub fn new() -> Self {
        Self {
            templates: DashMap::new(),
        }
    }

    pub fn get_template(&self, shape_id: ShapeId) -> Option<Arc<IcTemplate>> {
        self.templates.get(&shape_id).map(|r| Arc::clone(&*r))
    }

    pub fn upsert(&self, shape_id: ShapeId, template: IcTemplate) {
        self.templates.insert(shape_id, Arc::new(template));
    }

    pub fn upsert_if_better(&self, shape_id: ShapeId, template: IcTemplate) {
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
}

impl Default for IcForge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_nonexistent() {
        let forge = IcForge::new();
        assert!(forge.get_template(999).is_none());
    }

    #[test]
    fn test_upsert_and_get() {
        let forge = IcForge::new();
        let t = IcTemplate {
            shape_id: 1,
            offset: 3,
            generation: 10,
        };
        forge.upsert(1, t);
        let got = forge.get_template(1).unwrap();
        assert_eq!(got.shape_id, 1);
        assert_eq!(got.offset, 3);
        assert_eq!(got.generation, 10);
    }

    #[test]
    fn test_upsert_if_better_high_gen_wins() {
        let forge = IcForge::new();
        forge.upsert_if_better(
            1,
            IcTemplate {
                shape_id: 1,
                offset: 3,
                generation: 10,
            },
        );
        forge.upsert_if_better(
            1,
            IcTemplate {
                shape_id: 1,
                offset: 7,
                generation: 5,
            },
        );
        let got = forge.get_template(1).unwrap();
        assert_eq!(got.generation, 10);
    }

    #[test]
    fn test_upsert_if_better_low_replaced() {
        let forge = IcForge::new();
        forge.upsert_if_better(
            1,
            IcTemplate {
                shape_id: 1,
                offset: 3,
                generation: 5,
            },
        );
        forge.upsert_if_better(
            1,
            IcTemplate {
                shape_id: 1,
                offset: 7,
                generation: 10,
            },
        );
        let got = forge.get_template(1).unwrap();
        assert_eq!(got.generation, 10);
        assert_eq!(got.offset, 7);
    }
}
