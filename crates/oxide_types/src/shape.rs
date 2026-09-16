//! 隐藏类（hidden class / shape）存储。
//!
//! 形状表是对象属性布局的可共享描述：每次往对象添加一个命名属性就沿链增长一个
//! shape。形状按 `(parent_id, prop_name)` 哈希一致化（hash-cons），使结构相同的
//! 对象共享同一 shape，从而支持属性位置缓存（inline cache）。
//!
//! 使用范围：本模块仅 `EMPTY_SHAPE_ID` 常量被外部引用（如 `oxide_runtime_api`）；
//! `SHAPE_STORE` 全局单例与 `get_shape` 等自由函数目前仅本模块测试使用，运行时
//! 跨 VM 共享的形状存储是 `oxide_kernel::shape_forge::ShapeForge`。

use std::sync::{Arc, Mutex, OnceLock};

use hashbrown::HashMap;

/// 形状标识符，作为对象 header 中 24 位属性存储。
pub type ShapeId = u32;
/// 属性名字符串索引（进入字符串表的下标）。
pub type StringIndex = u32;

/// 空形状（无任何属性）的固定 ID。
///
/// 所有对象至少从空形状出发，`ShapeStore` 构造时即预置。
pub const EMPTY_SHAPE_ID: ShapeId = 1;
const EMPTY_SENTINEL: StringIndex = u32::MAX;

/// 单个形状：一次属性添加对应的节点。
///
/// `property_name` 为本节点新增的属性名（空形状为 `EMPTY_SENTINEL`），
/// `parent` 指向前一个形状，形成从当前形状到空形状的属性链。
#[derive(Debug, Clone)]
pub struct Shape {
    pub id: ShapeId,
    pub property_name: StringIndex,
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

        let new_id = self.next_id;
        self.next_id += 1;

        let shape = Arc::new(Shape {
            id: new_id,
            property_name: prop_name,
            parent: Some(parent_id),
        });

        while self.shapes.len() < new_id as usize {
            self.shapes.push(None);
        }
        self.shapes[(new_id - 1) as usize] = Some(shape);
        self.transitions.insert(key, new_id);

        new_id
    }

    fn lookup_position(&self, shape_id: ShapeId, prop_name: StringIndex) -> Option<u32> {
        let mut depth: u32 = 0;
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match self.get_shape(id) {
                Some(s) => {
                    cursor = s.parent;
                    if s.property_name != EMPTY_SENTINEL {
                        depth += 1;
                    }
                }
                None => break,
            }
        }
        let total_depth = depth;
        let mut step: u32 = 0;
        cursor = Some(shape_id);
        while let Some(id) = cursor {
            let s = self.get_shape(id)?;
            if s.property_name == prop_name && s.property_name != EMPTY_SENTINEL {
                return Some(total_depth - step - 1);
            }
            cursor = s.parent;
            step += 1;
        }
        None
    }

    fn has_property(&self, shape_id: ShapeId, prop_name: StringIndex) -> bool {
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            match self.get_shape(id) {
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

    fn shape_prop_count(&self, shape_id: ShapeId) -> u32 {
        let mut count = 0u32;
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

/// 按 ID 获取形状，不存在则返回 `None`。
pub fn get_shape(id: ShapeId) -> Option<Arc<Shape>> {
    store().lock().unwrap().get_shape(id)
}

/// 获取或创建 `(parent_id, prop_name)` 对应的形状 ID（hash-cons）。
///
/// 同一 `(parent, prop_name)` 键返回同一 ID；首次出现时分配新 ID 并沿链挂接。
pub fn make_shape(parent_id: ShapeId, prop_name: StringIndex) -> ShapeId {
    store().lock().unwrap().make_shape(parent_id, prop_name)
}

/// 在形状的属性链中查找 `prop_name` 的位置（从父向子的深度编号）。
///
/// 位置即该属性在 dense 属性向量中的下标：深度 0 表示链中最老的属性。
pub fn lookup_position(shape_id: ShapeId, prop_name: StringIndex) -> Option<u32> {
    store().lock().unwrap().lookup_position(shape_id, prop_name)
}

/// 判断形状链中是否包含 `prop_name`。
pub fn has_property(shape_id: ShapeId, prop_name: StringIndex) -> bool {
    store().lock().unwrap().has_property(shape_id, prop_name)
}

/// 返回形状链的属性数量（空形状计 0）。
pub fn shape_prop_count(shape_id: ShapeId) -> u32 {
    store().lock().unwrap().shape_prop_count(shape_id)
}

#[cfg(test)]
mod tests;
