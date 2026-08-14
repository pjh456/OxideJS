use std::hash::{Hash, Hasher};
use std::sync::{OnceLock, RwLock};

use dashmap::DashMap;
use oxide_types::object::JsString;
use rustc_hash::FxHasher;

use crate::{kernel_debug, kernel_trace};

/// 完整 64 位内容哈希。替代旧的 16 位 `hash16`，使 interner 的
/// hash→候选表碰撞风险可忽略。
fn hash64(s: &str) -> u64 {
    let mut h = FxHasher::default();
    s.hash(&mut h);
    h.finish()
}

/// 一条 intern 过的键。`data` 是泄漏的 `&'static str`——永久键从不释放
/// （按设计 append-only），所以泄漏即存储模型，而非 bug。
#[derive(Clone, Copy)]
struct PermEntry {
    data: &'static str,
    hash: u64,
}

/// 所有 VM 共享的 append-only、永不移动、读无锁的键 interner。
///
/// 取代旧的引用计数 `StringForge`（及其有缺陷的 `maybe_sweep` 重编号路径）。
/// 键（属性名、方法名）只 intern 一次，以 shape/IC 系统所依赖的稳定 `u32` id
/// 寻址。运行时字符串*值*不再在此 intern——它们是堆上的 `JsString`
/// 指针（见 `oxide_vm::Vm::new_string`）。
///
/// 并发性：
/// - `hash_map`（`DashMap`）在热 intern 路径上提供分片、无锁的
///   hash→候选映射读。
/// - `entries` 位于短 `RwLock` 之后；读时拷贝出 `&'static str`（Copy），
///   使借用存活时间超过锁守卫。
pub struct PermInterner {
    hash_map: DashMap<u64, Vec<u32>>,
    entries: RwLock<Vec<PermEntry>>,
    /// 惰性物化的永久 `JsString` 值，按下标索引。
    /// 用于永久键还必须以 JS 字符串*值*存在的情形
    /// （如 builtin 初始化时作为属性值暴露的方法名）。
    permanent_strings: RwLock<Vec<Option<Box<JsString>>>>,
}

impl PermInterner {
    /// 创建空 intern 表。
    pub fn new() -> Self {
        Self {
            hash_map: DashMap::new(),
            entries: RwLock::new(Vec::new()),
            permanent_strings: RwLock::new(Vec::new()),
        }
    }

    /// Intern 一个键。返回其稳定 id 与完整 64 位哈希。每个唯一字符串
    /// 恰好存储一次（单次分配，无双重 `to_string`）。
    pub fn intern(&self, s: &str) -> (u32, u64) {
        let hash = hash64(s);

        // 快路径：无锁候选读，短 entries 读锁。
        if let Some(candidates) = self.hash_map.get(&hash) {
            let entries = self.entries.read().unwrap();
            for &id in candidates.iter() {
                if entries[id as usize].data == s {
                    kernel_trace!("PermInterner intern hit id={}", id);
                    return (id, hash);
                }
            }
        }

        // 慢路径：写锁下追加，重查是否存在并发插入。
        let mut entries = self.entries.write().unwrap();
        if let Some(candidates) = self.hash_map.get(&hash) {
            for &id in candidates.iter() {
                if entries[id as usize].data == s {
                    return (id, hash);
                }
            }
        }
        let id = entries.len() as u32;
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        entries.push(PermEntry { data: leaked, hash });
        let entry_count = entries.len();
        drop(entries);
        self.hash_map.entry(hash).or_default().push(id);
        kernel_debug!("PermInterner intern new id={} len={}", id, s.len());
        if entry_count % 1000 == 0 {
            kernel_debug!("PermInterner stats: {} strings", entry_count);
        }
        (id, hash)
    }

    /// 以零克隆解析键 id 的文本。返回的 `&'static str`
    /// 在整个程序生命周期内有效（键从不释放）。
    pub fn lookup(&self, id: u32) -> Option<&'static str> {
        let entries = self.entries.read().unwrap();
        entries.get(id as usize).map(|e| e.data)
    }

    /// 键 id 的完整 64 位哈希。
    pub fn get_hash(&self, id: u32) -> Option<u64> {
        let entries = self.entries.read().unwrap();
        entries.get(id as usize).map(|e| e.hash)
    }

    /// 全部唯一 intern 键的数量。
    pub fn entry_count(&self) -> u32 {
        self.entries.read().unwrap().len() as u32
    }

    /// 是否尚无任何 intern 过的 key。
    pub fn is_empty(&self) -> bool {
        self.entry_count() == 0
    }

    /// 物化（仅一次）并返回指定键 id 的永久 `JsString` 稳定指针。
    /// 该 `JsString` 在整个程序生命周期内存活。
    pub fn string_ptr(&self, id: u32) -> *const JsString {
        {
            let perm = self.permanent_strings.read().unwrap();
            if let Some(Some(boxed)) = perm.get(id as usize) {
                return boxed.as_ref() as *const JsString;
            }
        }
        let text = self.lookup(id).unwrap_or("");
        let mut perm = self.permanent_strings.write().unwrap();
        if perm.len() <= id as usize {
            perm.resize_with(id as usize + 1, || None);
        }
        if perm[id as usize].is_none() {
            perm[id as usize] = Some(Box::new(JsString::new(text.to_string())));
        }
        perm[id as usize].as_ref().unwrap().as_ref() as *const JsString
    }
}

impl Default for PermInterner {
    fn default() -> Self {
        Self::new()
    }
}

/// 指向永久 `JsString` 的指针包装，供静态表存储。`JsString` 内容线程安全
/// （`String` + `AtomicU32`）且永久存活、只读共享，裸指针跨线程传递安全。
#[derive(Clone, Copy)]
#[repr(transparent)]
struct StringPtr(*const JsString);
unsafe impl Send for StringPtr {}
unsafe impl Sync for StringPtr {}

/// ASCII 单字符永久 `JsString` 指针表：以字节值（0..=127）为下标，
/// 首用惰性物化 `Box::into_raw` 永久泄漏，之后恒复用同一地址。
/// 泄漏面严格有界：≤128 条目 × 单字符内容（≈8KB），进程生命周期内不释放。
static SINGLE_CHAR_TABLE: [OnceLock<StringPtr>; 128] = [const { OnceLock::new() }; 128];

/// 取 ASCII 单字符的永久 `JsString` 指针，惰性物化一次后恒返回同一地址。
/// 供迭代器/charAt/空分隔 split/字符串展开等高频字符产出路径零分配复用；
/// 并发首用竞态下仍保证全局唯一指针（后者回收本线程副本）。
pub fn single_char_ptr(ch: u8) -> *const JsString {
    let slot = &SINGLE_CHAR_TABLE[ch as usize];
    if let Some(ptr) = slot.get() {
        return ptr.0;
    }
    let ptr = Box::into_raw(Box::new(JsString::new((ch as char).to_string())));
    match slot.set(StringPtr(ptr)) {
        Ok(()) => ptr,
        Err(existing) => {
            // 并发首用竞态：另一线程已物化，本线程指针尚未暴露给调用方，恰好释放一次。
            // SAFETY: ptr 来自本线程的 Box::into_raw，无任何外部引用。
            unsafe { drop(Box::from_raw(ptr)) };
            existing.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_dedup() {
        let interner = PermInterner::new();
        let (i1, h1) = interner.intern("abc");
        let (i2, h2) = interner.intern("abc");
        assert_eq!(i1, i2);
        assert_eq!(h1, h2);
    }

    #[test]
    fn intern_different() {
        let interner = PermInterner::new();
        let (i1, _) = interner.intern("x");
        let (i2, _) = interner.intern("y");
        assert_ne!(i1, i2);
    }

    #[test]
    fn lookup_zero_clone() {
        let interner = PermInterner::new();
        let (id, _) = interner.intern("hello");
        assert_eq!(interner.lookup(id), Some("hello"));
    }

    #[test]
    fn lookup_not_found() {
        let interner = PermInterner::new();
        assert_eq!(interner.lookup(99999), None);
    }

    #[test]
    fn entry_count_monotonic() {
        let interner = PermInterner::new();
        assert_eq!(interner.entry_count(), 0);
        interner.intern("a");
        interner.intern("b");
        interner.intern("a");
        assert_eq!(interner.entry_count(), 2);
    }

    #[test]
    fn many_unique_no_collision() {
        let interner = PermInterner::new();
        for i in 0..10_000 {
            let s = format!("key{i}");
            let (id, _) = interner.intern(&s);
            assert_eq!(interner.lookup(id), Some(&*Box::leak(s.into_boxed_str())));
        }
        assert_eq!(interner.entry_count(), 10_000);
    }

    #[test]
    fn string_ptr_roundtrip() {
        let interner = PermInterner::new();
        let (id, _) = interner.intern("perm");
        let ptr = interner.string_ptr(id);
        assert_eq!(unsafe { (*ptr).as_str() }, "perm");
        // 二次调用返回同一稳定指针（仅物化一次）。
        assert_eq!(interner.string_ptr(id), ptr);
    }

    #[test]
    fn single_char_ptr_idempotent() {
        let a = single_char_ptr(b'a');
        assert_eq!(unsafe { (*a).as_str() }, "a");
        // 二次调用返回同一稳定指针（仅物化一次）。
        assert_eq!(single_char_ptr(b'a'), a);
    }

    #[test]
    fn single_char_table_full_ascii() {
        // 全表 128 条目均可物化且内容为对应的单字符文本。
        for b in 0u8..=127 {
            let ptr = single_char_ptr(b);
            assert_eq!(unsafe { (*ptr).as_str() }, (b as char).to_string());
        }
    }
}
