use std::sync::Arc;

use dashmap::DashMap;

use crate::shape_forge::ShapeId;

/// 属性布局模板：记录某个 shape 下属性 `prop_name` 应落在对象存储中的 `position`。
///
/// `generation` 用于并发更新时的版本裁决（新代覆盖旧代）。
#[derive(Debug, Clone)]
pub struct PropTemplate {
    pub shape_id: ShapeId,
    pub prop_name: u32,
    pub position: u32,
    pub generation: u32,
}

/// 属性模板缓存：`shape_id → 属性布局` 的共享映射。
///
/// 供 IC（inline cache）与属性写入路径查询，避免每次写属性都回溯 shape 父链算槽位。
pub struct PropForge {
    templates: DashMap<ShapeId, Arc<PropTemplate>>,
}

impl PropForge {
    /// 创建空缓存。
    pub fn new() -> Self {
        Self { templates: DashMap::new() }
    }

    /// 取指定 shape 的属性模板；未缓存时返回 `None`。
    pub fn get_template(&self, shape_id: ShapeId) -> Option<Arc<PropTemplate>> {
        self.templates.get(&shape_id).map(|r| Arc::clone(&*r))
    }

    /// 无条件写入（覆盖）该 shape 的模板。
    pub fn upsert(&self, shape_id: ShapeId, template: PropTemplate) {
        self.templates.insert(shape_id, Arc::new(template));
    }

    /// 仅当新模板 `generation` 更高时覆盖已有模板；空缺则直接写入。
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

    /// 清空全部模板缓存。
    pub fn clear(&self) {
        self.templates.clear();
    }

    /// 缓存的模板数量。
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// 缓存是否为空。
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

impl Default for PropForge {
    fn default() -> Self {
        Self::new()
    }
}
