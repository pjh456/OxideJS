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

/// 把字符串单元序列编码为 interner 键文本。配对感知：
/// - 良配 surrogate 对（高+低）编码为对应单字符（超平面字符逐字保留，
///   键文本保持恒等）；
/// - 孤立 surrogate 编码为 U+FFFD 后跟 4 位小写十六进制；
/// - 单元 U+FFFD 编码为 U+FFFD 后跟字面文本 `fffd`（与任何 surrogate
///   的转义文本可区分）。
///
/// 编码无损：`decode_key` 是其精确逆变换（对无 FFFD 的良形文本为恒等）。
pub fn encode_key(units: &[u16]) -> String {
    let mut out = String::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if (0xD800..=0xDBFF).contains(&u) && i + 1 < units.len() {
            let lo = units[i + 1];
            if (0xDC00..=0xDFFF).contains(&lo) {
                let cp = 0x10000u32 + (((u as u32) - 0xD800) << 10) + (lo as u32 - 0xDC00);
                out.push(char::from_u32(cp).expect("良配 surrogate 对必映射为合法码点"));
                i += 2;
                continue;
            }
        }
        if (0xD800..=0xDFFF).contains(&u) {
            out.push('\u{FFFD}');
            out.push_str(&format!("{:04x}", u));
        } else if u == 0xFFFD {
            out.push('\u{FFFD}');
            out.push_str("fffd");
        } else {
            // 非 surrogate 单元恒可单字符表示（BMP 或超平面字符由对分支处理）。
            out.push(char::from_u32(u as u32).expect("非 surrogate 单元必为合法码点"));
        }
        i += 1;
    }
    out
}

/// `encode_key` 的逆变换：把键文本还原为单元序列。
///
/// 转义约定：U+FFFD 后跟 `fffd` 还原为单元 U+FFFD；U+FFFD 后跟 surrogate
/// 段内的 4 位小写十六进制还原为对应孤立 surrogate 单元。FFFD 后跟其他
/// 文本是防御性兜底（`encode_key` 不产生该形态）：输出 FFFD 单元并把后续
/// 已消费字符原样补回。
///
/// 已知边界：用户来源的键文本若含裸 FFFD 且其后恰为 `fffd` 或 surrogate
/// 段 4 位十六进制，物化时该 4 字符会被当作转义吞掉。物化面仅限 builtin
/// 永久键（恒为良形恒等文本），故该歧义不实际发生。
pub fn decode_key(text: &str) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{FFFD}' {
            if (c as u32) > 0xFFFF {
                // 超平面字符还原为其良配 surrogate 对。
                let v = (c as u32) - 0x10000;
                out.push(0xD800 + (v >> 10) as u16);
                out.push(0xDC00 + (v & 0x3FF) as u16);
            } else {
                out.push(c as u16);
            }
            continue;
        }
        // FFFD：消费其后最多 4 字符判定转义形态。
        let mut tail: [char; 4] = [' '; 4];
        let mut n = 0;
        while n < 4 {
            if let Some(&ch) = chars.peek() {
                tail[n] = ch;
                chars.next();
                n += 1;
            } else {
                break;
            }
        }
        if n == 4 && tail == ['f', 'f', 'f', 'd'] {
            out.push(0xFFFD);
            continue;
        }
        if n == 4 {
            let mut v: u16 = 0;
            let mut ok = true;
            for &ch in tail.iter() {
                // 编码端恒产小写十六进制（0-9 或 a-f），大写/非 hex 一律视为裸文本。
                match ch.to_digit(16) {
                    Some(d) if matches!(ch, '0'..='9' | 'a'..='f') => v = v * 16 + d as u16,
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && (0xD800..=0xDFFF).contains(&v) {
                out.push(v);
                continue;
            }
        }
        // 防御性兜底：裸 FFFD，后续字符原样补回。
        out.push(0xFFFD);
        for ch in tail.iter().take(n) {
            out.push(*ch as u16);
        }
    }
    out
}

/// 一条 intern 过的键。`data` 是泄漏的 `&'static str`——永久键从不释放
/// （按设计 append-only），所以泄漏即存储模型，而非 bug。键 id 与 64 位
/// 哈希经 `DashMap` 的哈希键→候选 id 表寻址，条目自身不存哈希。
/// `#[repr(C)]` 把布局钉为连续（指针, 长度）：字段序与对齐由 repr(C)
/// 保证（非 pack 保证），16B 由下方尺寸断言钉死。
#[repr(C)]
#[derive(Clone, Copy)]
struct PermEntry {
    data: &'static str,
}

// 布局钉：repr(C) 保证连续 16B；布局漂移即编译失败。
const _: () = assert!(std::mem::size_of::<PermEntry>() == 16);

/// 所有 VM 共享的 append-only、永不移动、读无锁的键 interner。
///
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
        entries.push(PermEntry { data: leaked });
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
            // 键文本经逆编码还原为单元序列再物化：良形键为逐字恒等，
            // 含转义的键（见 encode_key 约定）还原出孤立 surrogate 单元。
            let units = decode_key(text);
            perm[id as usize] = Some(Box::new(JsString::from_units(units)));
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
        Err(_) => {
            // 并发首用竞态：另一线程已物化。`set` 的 Err 载荷是本线程传入的值
            // （非槽内旧值），故先取槽内值，再把本线程副本恰好释放一次。
            let existing = slot.get().expect("set 失败时槽必已初始化").0;
            // SAFETY: ptr 来自本线程的 Box::into_raw，无任何外部引用。
            unsafe { drop(Box::from_raw(ptr)) };
            existing
        }
    }
}

/// 小整数（0..=99）永久 `JsString` 指针表：`s += j` 等数字叶子拼接的高频命中路径，
/// 免每次 `to_string` + 登记 2 次分配。泄漏面严格有界：100 条目 ≈ 4KB，进程生命
/// 期内不释放（同 `SINGLE_CHAR_TABLE` 论证）。
static SMALL_INT_TABLE: [OnceLock<StringPtr>; 100] = [const { OnceLock::new() }; 100];

/// 取 0..=99 小整数的永久 `JsString` 指针，惰性物化一次后恒返回同一地址。
/// 超出范围返回 `None`，由调用方回落普通字符串创建。
pub fn small_int_ptr(n: u32) -> Option<*const JsString> {
    if n >= 100 {
        return None;
    }
    let slot = &SMALL_INT_TABLE[n as usize];
    if let Some(ptr) = slot.get() {
        return Some(ptr.0);
    }
    let ptr = Box::into_raw(Box::new(JsString::new(n.to_string())));
    match slot.set(StringPtr(ptr)) {
        Ok(()) => Some(ptr),
        Err(_) => {
            // 并发首用竞态：与 single_char_ptr 同款处置，本线程产物恰好释放一次。
            let existing = slot.get().expect("set 失败时槽必已初始化").0;
            // SAFETY: ptr 来自本线程的 Box::into_raw，无任何外部引用。
            unsafe { drop(Box::from_raw(ptr)) };
            Some(existing)
        }
    }
}

/// typeof 结果文本表（下标见 [`typeof_string_ptr`]），进程生命周期内不释放。
const TYPEOF_TEXTS: [&str; 8] = ["undefined", "object", "boolean", "number", "string", "symbol", "bigint", "function"];

/// typeof 结果永久 `JsString` 指针表：typeof 是高频分支产出（typeof_ops 每迭代 4 次），
/// 复用静态表免每次 session 分配与 interner 锁查。泄漏面严格有界：8 条目 ≈ 数百字节。
static TYPEOF_TABLE: [OnceLock<StringPtr>; 8] = [const { OnceLock::new() }; 8];

/// 取 typeof 结果串的永久 `JsString` 指针，惰性物化一次后恒返回同一地址。
///
/// 下标约定：0=undefined、1=object、2=boolean、3=number、4=string、
/// 5=symbol、6=bigint、7=function。调用方按 `JsValue::js_type()` 映射。
pub fn typeof_string_ptr(kind: u8) -> *const JsString {
    let slot = &TYPEOF_TABLE[kind as usize];
    if let Some(ptr) = slot.get() {
        return ptr.0;
    }
    let ptr = Box::into_raw(Box::new(JsString::new(TYPEOF_TEXTS[kind as usize].to_string())));
    match slot.set(StringPtr(ptr)) {
        Ok(()) => ptr,
        Err(_) => {
            // 并发首用竞态：与 single_char_ptr 同款处置，本线程产物恰好释放一次。
            let existing = slot.get().expect("set 失败时槽必已初始化").0;
            // SAFETY: ptr 来自本线程的 Box::into_raw，无任何外部引用。
            unsafe { drop(Box::from_raw(ptr)) };
            existing
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

    #[test]
    fn typeof_table_content_and_stability() {
        // 8 个 typeof 结果串与下标约定一一对应，且二次调用返回同一稳定指针。
        let texts = ["undefined", "object", "boolean", "number", "string", "symbol", "bigint", "function"];
        for (i, t) in texts.iter().enumerate() {
            let ptr = typeof_string_ptr(i as u8);
            assert_eq!(unsafe { (*ptr).as_str() }, *t);
            assert_eq!(typeof_string_ptr(i as u8), ptr, "下标 {i} 二次调用应返回同一指针");
        }
    }

    #[test]
    fn encode_key_identity_and_escapes() {
        // 良形且无 FFFD（含超平面良配对）编码为恒等文本。
        assert_eq!(encode_key(&"abc🚀".encode_utf16().collect::<Vec<u16>>()), "abc🚀");
        // 孤立 surrogate：FFFD + 4 位小写十六进制。
        assert_eq!(encode_key(&[0xD800]), "\u{FFFD}d800");
        assert_eq!(encode_key(&[0xDFFF]), "\u{FFFD}dfff");
        assert_eq!(encode_key(&[0xDBFF, 0x61]), "\u{FFFD}dbffa");
        // FFFD 单元：FFFD + 字面 "fffd"。
        assert_eq!(encode_key(&[0xFFFD]), "\u{FFFD}fffd");
        // 两种不同单元序列的键文本互异（解码可区分）。
        assert_ne!(encode_key(&[0xFFFD]), encode_key(&[0xD800]));
    }

    #[test]
    fn encode_decode_roundtrip() {
        let cases: Vec<Vec<u16>> = vec![
            Vec::new(),
            "a".encode_utf16().collect(),
            "abc🚀😀".encode_utf16().collect(),
            vec![0xD800, 0x42, 0xDC00],
            vec![0xFFFD, 0xFFFD, 0x41, 0xFFFD, 0x66, 0x66, 0x66, 0x64],
            vec![0xFFFF, 0x0041, 0xDBFF, 0xDC00, 0xD800],
        ];
        for units in &cases {
            let key = encode_key(units);
            assert_eq!(decode_key(&key), units.as_slice(), "roundtrip 失败: key={key:?}");
        }
    }

    #[test]
    fn decode_key_defensive_raw_fffd() {
        // 裸 FFFD（文本末尾或后跟非转义文本）还原为 FFFD 单元，后续字符不被吞。
        assert_eq!(decode_key("a\u{FFFD}"), &[0x61, 0xFFFD]);
        assert_eq!(decode_key("a\u{FFFD}zz"), &[0x61, 0xFFFD, 0x7A, 0x7A]);
    }

    #[test]
    fn string_ptr_materializes_units() {
        let interner = PermInterner::new();
        // 良形键：物化内容与旧行为逐位一致，且二次调用返回同一稳定指针。
        let (id, _) = interner.intern("perm");
        let ptr = interner.string_ptr(id);
        assert_eq!(unsafe { (*ptr).as_str() }, "perm");
        assert_eq!(interner.string_ptr(id), ptr);
        // 含转义的键：物化出孤立 surrogate 单元形态。
        let (id2, _) = interner.intern(&encode_key(&[0xD800]));
        let ptr2 = interner.string_ptr(id2);
        assert!(unsafe { (*ptr2).has_lone_surrogate() });
        assert_eq!(unsafe { (*ptr2).units() }, &[0xD800][..]);
    }

    #[test]
    fn perm_table_concurrent_first_use_unique_ptr() {
        // 并发首用竞态回归：96 线程同一时刻命中冷表槽，强制多线程同入慢路径。
        // 所有调用方必须拿到同一稳定指针，且该指针恒指向存活的 JsString。
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        const N: usize = 96;
        let go = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::with_capacity(N);
        for _ in 0..N {
            let go = Arc::clone(&go);
            handles.push(std::thread::spawn(move || {
                while !go.load(Ordering::Acquire) {
                    std::hint::spin_loop();
                }
                // 裸指针 !Send，跨线程以 usize 传递，出口再转回。
                let (i, c, t) = (small_int_ptr(42).unwrap(), single_char_ptr(b'x'), typeof_string_ptr(1));
                (i as usize, c as usize, t as usize)
            }));
        }
        go.store(true, Ordering::Release);

        let mut results = Vec::with_capacity(N);
        for h in handles {
            results.push(h.join().expect("竞态线程"));
        }
        let (i0, c0, t0) = results[0];
        for (i, c, t) in &results[1..] {
            assert_eq!(*i, i0, "小整数表首用应全局唯一指针");
            assert_eq!(*c, c0, "单字符表首用应全局唯一指针");
            assert_eq!(*t, t0, "typeof 表首用应全局唯一指针");
        }

        // 返回指针必须指向存活 JsString：内容与长度可回读。
        assert_eq!(unsafe { (*(i0 as *const JsString)).as_str() }, "42");
        assert_eq!(unsafe { (*(c0 as *const JsString)).as_str() }, "x");
        let t0 = t0 as *const JsString;
        assert_eq!(unsafe { (*t0).as_str() }, "object");
        assert_eq!(unsafe { (*t0).utf16_len() }, 6);
    }
}
