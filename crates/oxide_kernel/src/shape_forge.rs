use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use dashmap::DashMap;

use crate::{kernel_debug, kernel_trace};

/// 隐藏类（shape）的整数标识符。
pub type ShapeId = u32;
/// 属性名在字符串 intern 表（[`crate::string_forge::PermInterner`]）中的 id。
pub type StringIndex = u32;

/// 空 shape 的保留 id：表示"无属性"的根节点，构造对象时以此为初始 shape。
pub const EMPTY_SHAPE_ID: ShapeId = 1;
const EMPTY_SENTINEL: StringIndex = u32::MAX;

/// 单个隐藏类节点：记录新增的属性名、父节点与根到自身的深度。
///
/// shape 构成一棵树，从根（[`EMPTY_SHAPE_ID`]）出发每添加一个属性产生一个子节点；
/// `depth` 即该节点的属性总数。
#[derive(Debug, Clone)]
pub struct Shape {
    pub id: ShapeId,
    pub property_name: StringIndex,
    pub parent: Option<ShapeId>,
    /// 从根到本 shape 的非哨兵属性数。
    pub depth: u32,
}

/// 共享隐藏类存储。
///
/// 并发模型：
/// - `shapes` 为 append-mostly，受 `RwLock` 保护。
/// - `transitions` 是以 `(parent_shape, property_name)` 为键的分片 `DashMap`。
/// - `positions` 缓存 `(shape_id, prop_name) → slot`，供 O(1) 重复查询。
/// - 分片数固定，保证跨机器的争用行为可复现，不依赖 DashMap 的 CPU 相关默认值。
pub struct ShapeForge {
    shapes: RwLock<Vec<Option<Arc<Shape>>>>,
    transitions: DashMap<u64, ShapeId>,
    positions: DashMap<u64, u32>,
    next_id: AtomicU32,
    overflow_map: RwLock<HashMap<u64, ShapeId>>,
    overflow_active: AtomicBool,
}

impl ShapeForge {
    /// 创建空 forge，并预置 [`EMPTY_SHAPE_ID`] 根节点。
    pub fn new() -> Self {
        let forge = Self {
            shapes: RwLock::new(Vec::with_capacity(256)),
            transitions: DashMap::with_capacity_and_shard_amount(256, 16),
            positions: DashMap::with_capacity_and_shard_amount(256, 16),
            next_id: AtomicU32::new(2),
            overflow_map: RwLock::new(HashMap::new()),
            overflow_active: AtomicBool::new(false),
        };
        {
            let mut shapes = forge.shapes.write().unwrap();
            let empty = Arc::new(Shape {
                id: EMPTY_SHAPE_ID,
                property_name: EMPTY_SENTINEL,
                parent: None,
                depth: 0,
            });
            debug_assert_eq!(shapes.len(), 0);
            shapes.push(Some(empty));
        }
        forge
    }

    /// 把 `(parent_shape, prop_name)` 打包为单个 u64 key，用作 transition / position 缓存的键。
    pub fn pack_key(parent_id: ShapeId, prop_name: StringIndex) -> u64 {
        ((parent_id as u64) << 32) | (prop_name as u64)
    }

    /// 取（或创建）`parent_id` 加属性 `prop_name` 对应的 shape id，实现哈希一致性。
    ///
    /// 相同 `(parent, prop_name)` 组合返回同一 id；超过 24 位上限后转入 overflow map 兜底。
    pub fn make_shape(&self, parent_id: ShapeId, prop_name: StringIndex) -> ShapeId {
        let key = Self::pack_key(parent_id, prop_name);

        kernel_trace!("ShapeForge lookup: parent={} prop={}", parent_id, prop_name);

        if let Some(entry) = self.transitions.get(&key) {
            kernel_debug!("ShapeForge transition cached parent={} prop={} -> id={}", parent_id, prop_name, *entry);
            return *entry;
        }

        if self.overflow_active.load(Ordering::Relaxed) {
            if let Some(id) = self.overflow_map.read().unwrap().get(&key) {
                kernel_debug!("ShapeForge overflow cached parent={} prop={} -> id={}", parent_id, prop_name, *id);
                return *id;
            }
        }

        let new_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        debug_assert!(new_id <= 0x00FF_FFFF, "shape_id {} exceeds 24-bit limit (16M)", new_id,);

        if new_id > 0x00FF_FFFF {
            self.overflow_active.store(true, Ordering::Relaxed);
            let mut map = self.overflow_map.write().unwrap();
            if let Some(id) = map.get(&key) {
                return *id;
            }
            map.insert(key, new_id);
            kernel_debug!("ShapeForge overflow parent={} prop={} -> id={}", parent_id, prop_name, new_id);
            return new_id;
        }

        // 从父节点计算深度（O(1)——只读缓存值）。
        let parent_depth = {
            let shapes = self.shapes.read().unwrap();
            shapes
                .get((parent_id - 1) as usize)
                .and_then(|s| s.as_ref())
                .map(|s| s.depth)
                .unwrap_or(0)
        };

        let shape = Arc::new(Shape {
            id: new_id,
            property_name: prop_name,
            parent: Some(parent_id),
            depth: parent_depth + 1,
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

    /// 按 id 取出 shape 节点的 `Arc` 副本；id 不存在时返回 `None`。
    pub fn get_shape(&self, id: ShapeId) -> Option<Arc<Shape>> {
        let shapes = self.shapes.read().unwrap();
        shapes.get((id - 1) as usize).and_then(|s| s.clone())
    }

    /// 当前登记在册的 shape 节点数（含根节点）。
    pub fn len(&self) -> usize {
        self.shapes.read().unwrap().len()
    }

    /// 是否尚无任何 shape（正常情况下始终为 false，因根节点恒存在）。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 清空所有非根 shape、transition 与 position 缓存，id 计数器复位。
    ///
    /// 用于 session 重建：丢弃一次性对象产生的形状，只保留跨 session 共享的根节点。
    pub fn clear_transient(&self) {
        let mut shapes = self.shapes.write().unwrap();
        if shapes.len() > 1 {
            shapes.truncate(1);
        }
        self.transitions.clear();
        self.positions.clear();
        self.next_id.store(2, Ordering::Relaxed);
    }

    /// 查询属性在给定 shape 对应对象存储中的槽位下标；沿父链上溯查找并缓存结果。
    pub fn lookup_position(&self, shape_id: ShapeId, prop_name: StringIndex) -> Option<u32> {
        let cache_key = Self::pack_key(shape_id, prop_name);
        if let Some(pos) = self.positions.get(&cache_key) {
            return Some(*pos);
        }

        let shapes = self.shapes.read().unwrap();
        let root = shapes.get((shape_id - 1) as usize)?.as_ref()?;
        let total_depth = root.depth;
        let mut prop_steps: u32 = 0;
        let mut cursor = Some(shape_id);
        while let Some(id) = cursor {
            let s = shapes.get((id - 1) as usize).and_then(|s| s.clone())?;
            if s.property_name != EMPTY_SENTINEL {
                if s.property_name == prop_name {
                    let pos = total_depth.checked_sub(prop_steps + 1)?;
                    drop(shapes);
                    self.positions.insert(cache_key, pos);
                    return Some(pos);
                }
                prop_steps += 1;
            }
            cursor = s.parent;
        }
        None
    }

    /// 判断 shape 或其祖先链上是否存在指定属性。
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

    /// 返回 shape 的属性数量（即根到该节点的深度）；未知 id 返回 0。
    pub fn shape_prop_count(&self, shape_id: ShapeId) -> u32 {
        let shapes = self.shapes.read().unwrap();
        shapes
            .get((shape_id - 1) as usize)
            .and_then(|s| s.as_ref())
            .map(|s| s.depth)
            .unwrap_or(0)
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
        assert_eq!(s.depth, 0);
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
        assert_eq!(shape.depth, 1);
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
    fn lookup_position_cached_second_call() {
        let forge = ShapeForge::new();
        let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_010);
        let s2 = forge.make_shape(s1, 1_000_020);
        let s3 = forge.make_shape(s2, 1_000_030);

        // 首次调用填充缓存。
        assert_eq!(forge.lookup_position(s3, 1_000_020), Some(1));
        // 二次调用命中缓存（验证无回归）。
        assert_eq!(forge.lookup_position(s3, 1_000_020), Some(1));
        assert_eq!(forge.lookup_position(s3, 1_000_030), Some(2));
        assert_eq!(forge.lookup_position(s3, 1_000_030), Some(2));
    }

    #[test]
    fn clear_transient_clears_positions() {
        let forge = ShapeForge::new();
        let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_010);
        assert_eq!(forge.lookup_position(s1, 1_000_010), Some(0));
        forge.clear_transient();
        // 清理后只剩 EMPTY_SHAPE，其余全部移除。
        assert_eq!(forge.shapes.read().unwrap().len(), 1);
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

    #[test]
    fn private_band_keys_are_shape_properties() {
        let forge = ShapeForge::new();
        let private_key = oxide_types::private_key::make_private_name_id(12);
        let shape = forge.make_shape(EMPTY_SHAPE_ID, private_key);
        assert!(oxide_types::private_key::is_private_name_key(private_key));
        assert!(forge.has_property(shape, private_key));
        assert_eq!(forge.lookup_position(shape, private_key), Some(0));
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "exceeds 24-bit"))]
    fn overflow_debug_assert_fires() {
        let forge = ShapeForge::new();
        forge.next_id.store(0x0100_0000, Ordering::Relaxed);
        let _ = forge.make_shape(EMPTY_SHAPE_ID, 1_000_400);
    }

    #[test]
    fn overflow_fallback_works() {
        let forge = ShapeForge::new();
        let key = ShapeForge::pack_key(EMPTY_SHAPE_ID, 1_000_500);
        forge.overflow_active.store(true, Ordering::Relaxed);
        forge.overflow_map.write().unwrap().insert(key, 0xDEAD);
        let id = forge.make_shape(EMPTY_SHAPE_ID, 1_000_500);
        assert_eq!(id, 0xDEAD);
    }

    #[test]
    fn overflow_concurrent_same_key() {
        let forge = Arc::new(ShapeForge::new());
        let key: StringIndex = 1_000_600;
        forge.overflow_active.store(true, Ordering::Relaxed);
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
        assert!(id1 > 0);
    }
}
