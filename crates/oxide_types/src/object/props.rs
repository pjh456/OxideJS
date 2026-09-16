//! 属性存储区与属性元数据读写 API：命名属性区（`hash_props`）、数组元素区与元素
//! 元数据区、命名属性元数据区三区，均为懒分配堆外裸指针（生命周期由 `ensure_*` /
//! `release_raw_heap` / GC 改写维护），dense 封顶 `MAX_DENSE_PROPS`，下标经
//! `PropIndex` 统一。

use super::{JsObject, PropAttributes, PropIndex, PropMetaEntry, MAX_DENSE_PROPS};
use crate::value::JsValue;

impl JsObject {
    /// 返回对象属性数：数组对象为数组元素数（`array_prop_count`），
    /// 普通对象为命名属性区长度（未分配时 0）。
    pub fn prop_count(&self) -> u32 {
        if self.is_array() {
            return self.array_prop_count;
        }
        if self.hash_props.is_null() {
            0
        } else {
            // SAFETY: hash_props 要么为空，要么在 ensure_hash_props/new_array 中
            // 由 Box<Vec<JsValue>> 创建并归本对象所有。
            let vec = unsafe { &*(self.hash_props as *const Vec<JsValue>) };
            vec.len() as u32
        }
    }

    /// 数组逻辑长度：`a.length` 读取语义（含超出 dense 上限的覆盖值）。
    #[inline]
    pub fn logical_len(&self) -> u32 {
        if self.array_len_override != 0 {
            self.array_len_override
        } else {
            self.array_prop_count
        }
    }

    /// 设置数组逻辑长度覆盖（仅在 length > `MAX_DENSE_PROPS` 时使用）。
    pub fn set_array_len_override(&mut self, len: u32) {
        self.array_len_override = len;
    }

    /// 清除数组逻辑长度覆盖，恢复以 `array_prop_count` 为 length。
    pub fn clear_array_len_override(&mut self) {
        self.array_len_override = 0;
    }

    /// 数组 length 属性值（`a.length` 读取结果）。
    pub fn logical_len_value(&self) -> JsValue {
        let l = self.logical_len();
        if l <= i32::MAX as u32 {
            JsValue::int(l as i32)
        } else {
            JsValue::float(l as f64)
        }
    }

    /// 设置数组元素数 / 命名属性向量长度。截断或补 undefined 扩展。
    /// 数组对象只调整独立元素区（`array_elements` + `array_prop_count`），命名属性区
    /// （`hash_props`）零搬移——push 为摊销 O(1) 的 `Vec::push`。
    pub fn set_prop_count(&mut self, count: impl PropIndex) {
        let target = count.to_u32() as usize;
        if self.is_array() {
            let old = self.array_prop_count as usize;
            let vec = self.ensure_array_elements();
            if target > old {
                vec.extend((old..target).map(|_| JsValue::undefined()));
            } else if target < old {
                vec.truncate(target);
            }
            if let Some(meta) = self.array_elements_meta_vec_mut() {
                if target > old {
                    meta.extend((old..target).map(|_| None));
                } else if target < old {
                    meta.truncate(target);
                }
            }
            self.array_prop_count = target as u32;
        } else {
            let vec = self.ensure_hash_props();
            if target < vec.len() {
                vec.truncate(target);
            } else {
                while vec.len() < target {
                    vec.push(JsValue::undefined());
                }
            }
            if let Some(meta) = self.prop_meta_vec_mut() {
                if target < meta.len() {
                    meta.truncate(target);
                } else {
                    while meta.len() < target {
                        meta.push(None);
                    }
                }
            }
        }
    }

    /// 是否已分配任何属性元数据向量（命名属性区或数组元素区）。
    pub fn has_prop_meta(&self) -> bool {
        !self.prop_meta.is_null() || !self.array_elements_meta.is_null()
    }

    /// 设置数组元素数（数组对象高频入口，与 `set_prop_count` 行为一致）。
    ///
    /// # 边界与前提
    /// - 参数为新元素数；截断或补 `undefined` 扩展。
    /// - 数组对象只调整独立元素区（`array_elements` 与 `array_prop_count`），
    ///   命名属性区（`hash_props`）零搬移。
    ///
    /// # 副作用
    /// - 修改 `array_prop_count` 与元素区；元素区扩缩时同步扩缩元素元数据区。
    #[inline]
    pub fn set_prop_count_fast(&mut self, count: impl PropIndex) {
        self.set_prop_count(count);
    }

    /// 确保属性元数据向量已分配并返回可变引用。
    ///
    /// 初始长度与当前命名属性数对齐，全部初始化为 `None`（无元数据）。
    pub fn ensure_prop_meta(&mut self) -> &mut Vec<Option<PropMetaEntry>> {
        if self.prop_meta.is_null() {
            let len = self.prop_vec_len();
            let vec = Box::new(vec![None::<PropMetaEntry>; len]);
            self.prop_meta = Box::into_raw(vec) as *mut u8;
        }
        // SAFETY: prop_meta 在 ensure_prop_meta 中由
        // Box<Vec<Option<PropMetaEntry>>> 创建，本对象持有期间始终有效。
        unsafe { &mut *(self.prop_meta as *mut Vec<Option<PropMetaEntry>>) }
    }

    /// 只读访问属性元数据向量（未分配时返回 `None`）。
    pub fn prop_meta_vec(&self) -> Option<&Vec<Option<PropMetaEntry>>> {
        if self.prop_meta.is_null() {
            None
        } else {
            // SAFETY: prop_meta 在 ensure_prop_meta 中由
            // Box<Vec<Option<PropMetaEntry>>> 创建，本对象持有期间始终有效。
            unsafe { Some(&*(self.prop_meta as *const Vec<Option<PropMetaEntry>>)) }
        }
    }

    pub(crate) fn prop_meta_vec_mut(&mut self) -> Option<&mut Vec<Option<PropMetaEntry>>> {
        if self.prop_meta.is_null() {
            None
        } else {
            // SAFETY: prop_meta 在 ensure_prop_meta 中由
            // Box<Vec<Option<PropMetaEntry>>> 创建，本对象持有期间始终有效。
            unsafe { Some(&mut *(self.prop_meta as *mut Vec<Option<PropMetaEntry>>)) }
        }
    }

    /// 读取绝对存储下标 position 处的元数据；无元数据或越界返回 `None`。
    pub fn prop_meta_at(&self, position: impl PropIndex) -> Option<PropMetaEntry> {
        let pos = position.to_u32() as usize;
        if self.is_array() {
            let count = self.array_prop_count as usize;
            if pos < count {
                return self.array_elements_meta_vec().and_then(|vec| vec.get(pos).copied().flatten());
            }
            return self.prop_meta_vec().and_then(|vec| vec.get(pos - count).copied().flatten());
        }
        self.prop_meta_vec().and_then(|vec| vec.get(pos).copied().flatten())
    }

    /// 设置指定下标属性的数据属性描述符。
    pub fn set_data_meta(&mut self, position: impl PropIndex, attributes: PropAttributes) {
        self.set_meta_at(position, PropMetaEntry::data(attributes));
    }

    /// 设置指定下标属性的访问器描述符（getter/setter）。
    pub fn set_accessor_meta(
        &mut self, position: impl PropIndex, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) {
        self.set_meta_at(position, PropMetaEntry::accessor(get, set, attributes));
    }

    /// 标记私有方法槽为不可写：复用 `hole` 位（私有键在普通属性/枚举路径不可见，
    /// 与数组删除标记无冲突）。GET_PRIVATE 仍按数据槽取值，SET_PRIVATE 遇此标记抛错。
    pub fn set_private_method_meta(&mut self, position: impl PropIndex) {
        self.set_meta_at(
            position,
            PropMetaEntry {
                hole: true,
                ..PropMetaEntry::data(PropAttributes::DEFAULT_DATA)
            },
        );
    }

    /// 判断指定下标的属性是否为访问器属性。
    pub fn is_accessor_meta(&self, position: impl PropIndex) -> bool {
        self.prop_meta_at(position).is_some_and(|entry| entry.is_accessor)
    }

    fn set_meta_at(&mut self, position: impl PropIndex, entry: PropMetaEntry) {
        let pos = position.to_u32() as usize;
        if self.is_array() {
            let count = self.array_prop_count as usize;
            if pos < count {
                let meta = self.ensure_array_elements_meta();
                while meta.len() <= pos {
                    meta.push(None);
                }
                meta[pos] = Some(entry);
                return;
            }
            // 命名属性区：值与长度由 set_prop_storage/push_prop 保证。
            let np = pos - count;
            let meta = self.ensure_prop_meta();
            while meta.len() <= np {
                meta.push(None);
            }
            meta[np] = Some(entry);
            return;
        }
        let prop_len = self.prop_vec_len();
        if pos >= prop_len {
            self.set_prop_count(pos + 1);
        }
        let meta = self.ensure_prop_meta();
        while meta.len() <= pos {
            meta.push(None);
        }
        meta[pos] = Some(entry);
    }

    /// 把数组元素槽标记为 hole（删除语义）：值置 undefined 并写入 hole 标记，
    /// `array_prop_count` 与元素区大小不变（length 保持不变）。
    pub fn mark_hole_at(&mut self, position: impl PropIndex) {
        let pos = position.to_u32() as usize;
        if self.is_array() {
            let count = self.array_prop_count as usize;
            if pos < count {
                let vec = self.ensure_array_elements();
                if pos >= vec.len() {
                    vec.push(JsValue::undefined());
                }
                vec[pos] = JsValue::undefined();
                let meta = self.ensure_array_elements_meta();
                while meta.len() <= pos {
                    meta.push(None);
                }
                meta[pos] = Some(PropMetaEntry::hole());
            }
            return;
        }
        if pos >= self.prop_vec_len() {
            self.set_prop_count(pos + 1);
        }
        self.set_prop_at(pos, JsValue::undefined());
        let meta = self.ensure_prop_meta();
        while meta.len() <= pos {
            meta.push(None);
        }
        meta[pos] = Some(PropMetaEntry::hole());
    }

    /// 若绝对下标是 hole 标记则清除（元素被重新写入时恢复为存在）。
    fn clear_hole_marker(&mut self, pos: usize) {
        if self.is_array() {
            let count = self.array_prop_count as usize;
            if pos < count {
                if let Some(meta) = self.array_elements_meta_vec_mut() {
                    if meta.get(pos).is_some_and(|entry| entry.is_some_and(|e| e.is_hole())) {
                        meta[pos] = None;
                    }
                }
                return;
            }
            if let Some(meta) = self.prop_meta_vec_mut() {
                let np = pos - count;
                if meta.get(np).is_some_and(|entry| entry.is_some_and(|e| e.is_hole())) {
                    meta[np] = None;
                }
            }
            return;
        }
        if let Some(meta) = self.prop_meta_vec_mut() {
            if meta.get(pos).is_some_and(|entry| entry.is_some_and(|e| e.is_hole())) {
                meta[pos] = None;
            }
        }
    }

    /// 清空全部命名属性、数组元素区与元数据，`array_prop_count` 归零。
    ///
    /// 供 shape 链重建（如 delete 重排属性表）使用：清空后以 `push_prop` /
    /// `set_prop_count` 按新形状重填。不清除形状 ID，调用方自行处理。
    pub fn clear_props(&mut self) {
        if !self.array_elements.is_null() {
            // SAFETY: array_elements 在 ensure_array_elements/new_array 中由 Box<Vec<JsValue>> 创建。
            let vec = unsafe { &mut *(self.array_elements as *mut Vec<JsValue>) };
            vec.clear();
        }
        if !self.array_elements_meta.is_null() {
            // SAFETY: array_elements_meta 在 ensure_array_elements_meta 中创建。
            let meta = unsafe { &mut *(self.array_elements_meta as *mut Vec<Option<PropMetaEntry>>) };
            meta.clear();
        }
        if !self.hash_props.is_null() {
            // SAFETY: hash_props 在 ensure_hash_props/new_array 中由 Box<Vec<JsValue>> 创建。
            let vec = unsafe { &mut *(self.hash_props as *mut Vec<JsValue>) };
            vec.clear();
        }
        if !self.prop_meta.is_null() {
            // SAFETY: prop_meta 在 ensure_prop_meta 中由 Box<Vec<Option<PropMetaEntry>>> 创建。
            let meta = unsafe { &mut *(self.prop_meta as *mut Vec<Option<PropMetaEntry>>) };
            meta.clear();
        }
        self.array_prop_count = 0;
    }

    /// 若 hash_props 为空则初始化，返回其可变引用。
    pub fn ensure_hash_props(&mut self) -> &mut Vec<JsValue> {
        if self.hash_props.is_null() {
            let vec = Box::new(Vec::<JsValue>::new());
            self.hash_props = Box::into_raw(vec) as *mut u8;
        }
        // SAFETY: hash_props 在本方法或 new_array 中由 Box<Vec<JsValue>> 创建。
        unsafe { &mut *(self.hash_props as *mut Vec<JsValue>) }
    }

    /// hash_props vec 的安全只读访问；未分配时返回 None。
    pub fn hash_props_vec(&self) -> Option<&Vec<JsValue>> {
        if self.hash_props.is_null() {
            None
        } else {
            // SAFETY: hash_props 在 ensure_hash_props/new_array 中由 Box<Vec<JsValue>> 创建。
            unsafe { Some(&*(self.hash_props as *const Vec<JsValue>)) }
        }
    }

    pub(crate) fn hash_props_vec_mut(&mut self) -> Option<&mut Vec<JsValue>> {
        if self.hash_props.is_null() {
            None
        } else {
            // SAFETY: hash_props 在 ensure_hash_props/new_array 中由 Box<Vec<JsValue>> 创建。
            unsafe { Some(&mut *(self.hash_props as *mut Vec<JsValue>)) }
        }
    }

    /// 若数组元素区为空则初始化，并把长度对齐到 `array_prop_count`，返回其可变引用。
    fn ensure_array_elements(&mut self) -> &mut Vec<JsValue> {
        if self.array_elements.is_null() {
            let vec = Box::new(Vec::<JsValue>::new());
            self.array_elements = Box::into_raw(vec) as *mut u8;
        }
        // SAFETY: array_elements 在本方法或 new_array 中由 Box<Vec<JsValue>> 创建。
        let vec = unsafe { &mut *(self.array_elements as *mut Vec<JsValue>) };
        while vec.len() < self.array_prop_count as usize {
            vec.push(JsValue::undefined());
        }
        vec
    }

    /// 数组元素区的安全只读访问；未分配时返回 None。
    pub fn array_elements_vec(&self) -> Option<&Vec<JsValue>> {
        if self.array_elements.is_null() {
            None
        } else {
            // SAFETY: array_elements 在 ensure_array_elements/new_array 中由 Box<Vec<JsValue>> 创建。
            unsafe { Some(&*(self.array_elements as *const Vec<JsValue>)) }
        }
    }

    pub(crate) fn array_elements_vec_mut(&mut self) -> Option<&mut Vec<JsValue>> {
        if self.array_elements.is_null() {
            None
        } else {
            // SAFETY: array_elements 在 ensure_array_elements/new_array 中由 Box<Vec<JsValue>> 创建。
            unsafe { Some(&mut *(self.array_elements as *mut Vec<JsValue>)) }
        }
    }

    /// 确保数组元素元数据向量已分配并返回可变引用（长度对齐当前元素数）。
    pub fn ensure_array_elements_meta(&mut self) -> &mut Vec<Option<PropMetaEntry>> {
        if self.array_elements_meta.is_null() {
            let len = self.array_prop_count as usize;
            let vec = Box::new(vec![None::<PropMetaEntry>; len]);
            self.array_elements_meta = Box::into_raw(vec) as *mut u8;
        }
        // SAFETY: array_elements_meta 在本方法中由 Box<Vec<Option<PropMetaEntry>>> 创建。
        let vec = unsafe { &mut *(self.array_elements_meta as *mut Vec<Option<PropMetaEntry>>) };
        while vec.len() < self.array_prop_count as usize {
            vec.push(None);
        }
        vec
    }

    /// 数组元素元数据区的安全只读访问；未分配时返回 None。
    pub fn array_elements_meta_vec(&self) -> Option<&Vec<Option<PropMetaEntry>>> {
        if self.array_elements_meta.is_null() {
            None
        } else {
            // SAFETY: array_elements_meta 在 ensure_array_elements_meta 中创建。
            unsafe { Some(&*(self.array_elements_meta as *const Vec<Option<PropMetaEntry>>)) }
        }
    }

    pub(crate) fn array_elements_meta_vec_mut(&mut self) -> Option<&mut Vec<Option<PropMetaEntry>>> {
        if self.array_elements_meta.is_null() {
            None
        } else {
            // SAFETY: array_elements_meta 在 ensure_array_elements_meta 中创建。
            unsafe { Some(&mut *(self.array_elements_meta as *mut Vec<Option<PropMetaEntry>>)) }
        }
    }

    /// 取绝对存储下标 position 处的值（数组：元素区在前，命名属性区在后）。
    /// 对应存储未分配或越界时返回 JsValue::undefined()。
    pub fn get_prop_at(&self, position: impl PropIndex) -> JsValue {
        let pos = position.to_u32() as usize;
        if self.is_array() {
            let count = self.array_prop_count as usize;
            if pos < count {
                if self.array_elements.is_null() {
                    return JsValue::undefined();
                }
                // SAFETY: array_elements 在 ensure_array_elements/new_array 中由 Box<Vec<JsValue>> 创建。
                let vec = unsafe { &*(self.array_elements as *const Vec<JsValue>) };
                return vec.get(pos).copied().unwrap_or(JsValue::undefined());
            }
            if self.hash_props.is_null() {
                return JsValue::undefined();
            }
            // SAFETY: hash_props 在 ensure_hash_props/new_array 中由 Box<Vec<JsValue>> 创建。
            let vec = unsafe { &*(self.hash_props as *const Vec<JsValue>) };
            return vec.get(pos - count).copied().unwrap_or(JsValue::undefined());
        }
        if self.hash_props.is_null() {
            return JsValue::undefined();
        }
        // SAFETY: hash_props 在 ensure_hash_props/new_array 中由 Box<Vec<JsValue>> 创建。
        let vec = unsafe { &*(self.hash_props as *const Vec<JsValue>) };
        vec.get(pos).copied().unwrap_or(JsValue::undefined())
    }

    /// 设置数组元素 position 处的值（数组对象）；普通对象按绝对下标写入并自动扩容。
    /// 数组元素写入会更新 `array_prop_count`（元素数随最高索引增长）。
    pub fn set_prop_at(&mut self, position: impl PropIndex, val: JsValue) {
        let pos = position.to_u32() as usize;
        if pos > MAX_DENSE_PROPS {
            return;
        }
        if self.is_array() {
            // 元素写入越过元素区：只扩独立元素区，命名属性区零搬移。
            if pos >= self.array_prop_count as usize {
                self.set_prop_count(pos + 1);
            }
            let vec = self.ensure_array_elements();
            vec[pos] = val;
            self.clear_hole_marker(pos);
            return;
        }
        {
            let vec = self.ensure_hash_props();
            if pos < vec.len() {
                vec[pos] = val;
            } else {
                while vec.len() < pos {
                    vec.push(JsValue::undefined());
                }
                vec.push(val);
            }
        }
        if let Some(meta) = self.prop_meta_vec_mut() {
            while meta.len() <= pos {
                meta.push(None);
            }
        }
    }

    /// 数组对象（shape 槽位 → 命名属性区存储索引）的属性写入；普通对象等价
    /// `set_prop_storage(shape_pos)`。
    pub fn set_prop_shape(&mut self, shape_pos: u32, val: JsValue) {
        let idx = if self.is_array() {
            self.array_prop_count as usize + shape_pos as usize
        } else {
            shape_pos as usize
        };
        self.set_prop_storage(idx, val);
    }

    /// 按绝对存储索引写入值，数组对象把元素区与命名属性区分派到各自存储。
    /// 用于调用方已知属性存储位置（如 `get_own_property_slot` 返回的索引）的场景。
    pub fn set_prop_storage(&mut self, idx: usize, val: JsValue) {
        if self.is_array() {
            let count = self.array_prop_count as usize;
            if idx < count {
                let vec = self.ensure_array_elements();
                if idx >= vec.len() {
                    vec.push(JsValue::undefined());
                }
                vec[idx] = val;
                self.clear_hole_marker(idx);
                return;
            }
            let np = idx - count;
            let vec = self.ensure_hash_props();
            while vec.len() <= np {
                vec.push(JsValue::undefined());
            }
            vec[np] = val;
            self.clear_hole_marker(idx);
            if let Some(meta) = self.prop_meta_vec_mut() {
                while meta.len() <= np {
                    meta.push(None);
                }
            }
            return;
        }
        let vec = self.ensure_hash_props();
        while vec.len() <= idx {
            vec.push(JsValue::undefined());
        }
        vec[idx] = val;
        self.clear_hole_marker(idx);
        if let Some(meta) = self.prop_meta_vec_mut() {
            while meta.len() <= idx {
                meta.push(None);
            }
        }
    }

    /// 数组对象属性读取（shape 槽位 → 命名属性区存储索引）；普通对象等价
    /// `get_prop_at(shape_pos)`。越界返回 undefined。
    pub fn get_prop_shape(&self, shape_pos: u32) -> JsValue {
        let idx = if self.is_array() {
            self.array_prop_count as usize + shape_pos as usize
        } else {
            shape_pos as usize
        };
        self.get_prop_at(idx)
    }

    /// 把值压入命名属性区，返回其绝对存储下标（数组对象 = `array_prop_count + 命名下标`）。
    pub fn push_prop(&mut self, val: JsValue) -> u32 {
        let pos = if self.is_array() {
            self.array_prop_count as usize + self.hash_props_vec().map_or(0, Vec::len)
        } else {
            self.hash_props_vec().map_or(0, Vec::len)
        };
        let vec = self.ensure_hash_props();
        vec.push(val);
        if let Some(meta) = self.prop_meta_vec_mut() {
            meta.push(None);
        }
        pos as u32
    }

    /// 返回 hash_props vec 的长度（未分配时返回 0）。
    pub fn prop_vec_len(&self) -> usize {
        if self.hash_props.is_null() {
            0
        } else {
            // SAFETY: hash_props 在 ensure_hash_props/new_array 中由 Box<Vec<JsValue>> 创建。
            unsafe { &*(self.hash_props as *const Vec<JsValue>) }.len()
        }
    }
}
