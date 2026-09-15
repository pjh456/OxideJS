//! `JsString` 三形态载荷（Flat UTF-8 文本 / FlatU16 单元 / Cons rope）与 rope 节点 `ConsNode`。
//!
//! - 布局恒为 32B：载荷 24B @0、`utf16_len` @24、`tag` @28，由文件内 size_of/
//!   offset_of 断言锚定；
//! - `Cons` 子节点各自独立登记 session 字符串表（或为 perm 串），扁平化产物只挂
//!   `ConsNode::flat_cache`、随节点连带释放；
//! - 地址稳定不搬移（Box 堆分配），GC 无需 forwarding / rewrite。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
/// 堆分配的 JS 字符串值。
///
/// 字符串*值*以 48 位指针（指向 `JsString`）NaN-box（见 `JsValue::string`）。
///
/// 三种载荷形态（`tag` 区分，载荷并位 @0）：
/// - `TAG_FLAT`：整块 UTF-8 文本（`String`，保留容量）。字面量、方法结果与
///   CONCAT_N 产物保持此形态；内容无孤立 surrogate（UTF-8 合法性天然保证）；
/// - `TAG_FLAT_U16`：UTF-16 单元载荷（`Vec<u16>`，ptr/len/cap 与 String 同形），
///   承载含孤立 surrogate 的任意单元序列——`String` 在结构上不可表示该内容；
/// - `TAG_CONS`（rope）：指向 [`ConsNode`] 载荷（左右子裸指针 + O(1) 单元长 +
///   惰性扁平化缓存）。二元 `+`/`+=` 拼接 O(1) 链接不拷贝文本，单元序列在
///   首次消费时扁平化并原子发布。
///
/// 布局恒为 32B：载荷 24B @0、`utf16_len` @24（4B）、`tag` @28（1B，原 NLL
/// enum 的尾部填充位，显式 repr(C) 钉死）、尾部 3B 填充。`String` 原样存储
/// （零 realloc 收缩）；Cons 专属状态（子节点/产物缓存）独立分配在 [`ConsNode`]，
/// 仅大链持有。尺寸锚见下方 size_of/offset_of 断言。
///
/// 生命周期约定：
/// - `Cons` 子节点各自独立登记 session 字符串表（或为 perm 串），由 GC 的
///   mark 传播闭包保证随父存活；扁平化产物只挂在 `ConsNode::flat_cache`、不进
///   session 主表，随本节点连带释放。
/// - 地址稳定不搬移（Box 堆分配），GC 无需 forwarding / rewrite。
#[repr(C)]
pub struct JsString {
    /// 载荷 24B，按 `tag` 解读：Flat = `String`（data/len/cap）、FlatU16 =
    /// `Vec<u16>`（ptr/len/cap，与 String 同形）、Cons = 节点指针（首字，
    /// 余 16B 未定义填充）。
    payload: [u64; 3],
    /// JS 语义的字符串长度（UTF-16 code unit 数）。
    ///
    /// Flat 为懒缓存：构造是热路径（拼接/格式化），长度查询少见，不预付 O(n)
    /// 扫描，首次 `utf16_len()` 访问时计算一次（sentinel + `AtomicU32`：perm
    /// 串可被多线程 VM 共享读，计算幂等，relaxed 序即可）。FlatU16/Cons 构造时
    /// 预写（单元数 / 子节点归纳，各 O(1)）。
    utf16_len: AtomicU32,
    tag: u8,
}

// 布局锚：32B = 载荷 24B（三形态同形 ptr/len/cap 或节点指针）+ utf16_len 4B
// + tag 1B + 尾部填充 3B。三变体 NLL enum 需独立 tag 字（撑到 40B、utf16_len@32）
// 故以 repr(C) + 尾部 tag 字节钉死本布局；换载荷形状/字段类型先同步结构体头
// 注释再改本断言。
const _: () = assert!(
    std::mem::size_of::<JsString>() == 32
        && std::mem::offset_of!(JsString, utf16_len) == 24
        && std::mem::offset_of!(JsString, tag) == 28,
    "JsString 布局漂移：锚定布局见结构体头注释（32B，utf16_len@24，tag@28）"
);

/// 载荷形态标签（`JsString::tag` 取值）。
const TAG_FLAT: u8 = 0;
const TAG_FLAT_U16: u8 = 1;
const TAG_CONS: u8 = 2;

impl std::fmt::Debug for JsString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.tag {
            TAG_FLAT => write!(f, "JsString::Flat({:?})", self.flat_ref()),
            TAG_FLAT_U16 => write!(f, "JsString::FlatU16({:?})", self.flat_u16_ref()),
            TAG_CONS => write!(f, "JsString::Cons(len={})", self.utf16_len()),
            _ => write!(f, "JsString::Invalid"),
        }
    }
}

impl Drop for JsString {
    fn drop(&mut self) {
        // 按 tag 恰好释放激活载荷：Flat/FlatU16 释放载荷自身（内部 heap 缓冲），
        // Cons 载荷节点由 drop_cons_node 单独释放（见其文档）。
        //
        // # Safety
        // tag 与载荷内容恒由构造点同步写入；drop 期载荷仍为激活形态。
        unsafe {
            match self.tag {
                TAG_FLAT => std::ptr::drop_in_place(self.payload.as_mut_ptr() as *mut String),
                TAG_FLAT_U16 => std::ptr::drop_in_place(self.payload.as_mut_ptr() as *mut Vec<u16>),
                TAG_CONS => {}
                _ => unreachable!(),
            }
        }
    }
}

/// Cons（rope）节点的载荷：左右子节点指针 + O(1) 拼接单元长 + 扁平化产物缓存。
///
/// 独立于 `JsString` 分配，使 Flat 路径的 `JsString` 保持 32B 结构（tag 并入
/// 尾部填充位，见其尺寸断言）；仅超过阈值的拼接（`Vm::new_cons_string`）才
/// 创建本节点。
///
/// 生命周期约定：节点由 `Box::into_raw` 分配，只经 [`JsString::drop_cons_node`]
/// 释放（连带扁平化产物）；`left`/`right` 由 GC 传播闭包保证随父存活。
pub struct ConsNode {
    left: *const JsString,
    right: *const JsString,
    /// 拼接 UTF-16 单元长：构造时 = left.utf16_len() + right.utf16_len()（各 O(1)），
    /// 免递归求长、不触发扁平化。
    unit_len: u32,
    /// 扁平化产物缓存：首次单元消费时分配整块 FlatU16 产物并原子发布（OnceLock）。
    /// 产物不进 session 主表，生命周期挂本节点——GC 传播保证其随父存活，
    /// 释放节点时连带释放。
    flat_cache: OnceLock<*const JsString>,
}

/// `utf16_len` 未计算的哨兵值（有效长度不可能为 u32::MAX；仅 Flat 形态使用）。
const UTF16_LEN_UNSET: u32 = u32::MAX;

impl ConsNode {
    /// 扁平化产物指针（未扁平化返回 null）。
    pub fn flat_cache_ptr(&self) -> *const JsString {
        self.flat_cache.get().copied().unwrap_or(std::ptr::null())
    }
}

impl JsString {
    /// Flat 载荷借用（载荷即原始 `String`）。
    ///
    /// # Safety
    /// 仅当 `tag == TAG_FLAT` 时调用（debug_assert 已覆盖常规路径）。
    #[inline(always)]
    fn flat_ref(&self) -> &String {
        debug_assert!(self.tag == TAG_FLAT, "flat_ref 仅限 Flat 形态");
        // SAFETY: 形态检查通过后载荷即 String（布局与 [u64;3] 逐位一致）。
        unsafe { &*(self.payload.as_ptr() as *const String) }
    }

    /// FlatU16 载荷借用（载荷即原始 `Vec<u16>`）。
    ///
    /// # Safety
    /// 仅当 `tag == TAG_FLAT_U16` 时调用（debug_assert 已覆盖常规路径）。
    #[inline(always)]
    fn flat_u16_ref(&self) -> &Vec<u16> {
        debug_assert!(self.tag == TAG_FLAT_U16, "flat_u16_ref 仅限 FlatU16 形态");
        // SAFETY: 形态检查通过后载荷即 Vec<u16>（布局与 [u64;3] 逐位一致）。
        unsafe { &*(self.payload.as_ptr() as *const Vec<u16>) }
    }

    /// Cons 节点指针（载荷首字即节点裸指针）。
    ///
    /// # Safety
    /// 仅当 `tag == TAG_CONS` 时调用（debug_assert 已覆盖常规路径）。
    #[inline(always)]
    fn cons_ptr(&self) -> *const ConsNode {
        debug_assert!(self.tag == TAG_CONS, "cons_ptr 仅限 Cons 形态");
        // SAFETY: 形态检查通过后载荷首字即节点指针。
        unsafe { *(self.payload.as_ptr() as *const *const ConsNode) }
    }

    /// 用 UTF-8 数据构造 Flat 字符串（内容恒 well-formed，无孤立 surrogate）。
    pub fn new(data: String) -> Self {
        let mut payload = [0u64; 3];
        // SAFETY: String 与 [u64;3] 布局一致（24B ptr/len/cap）；write 不读目的位。
        unsafe { std::ptr::write(payload.as_mut_ptr() as *mut String, data) };
        Self {
            payload,
            utf16_len: AtomicU32::new(UTF16_LEN_UNSET),
            tag: TAG_FLAT,
        }
    }

    /// 直接以单元序列构造 FlatU16 字符串（不做 well-formed 判定）。
    ///
    /// 供明确知道内容形态的构造点使用（迭代器源物化、单元解码）：smart
    /// 路由场景一律走 [`Self::from_units`]（well-formed 结果落 Flat）。
    pub fn new_flat_u16(units: Vec<u16>) -> Self {
        let len = units.len() as u32;
        let mut payload = [0u64; 3];
        // SAFETY: Vec<u16> 与 [u64;3] 布局一致（24B ptr/len/cap）；write 不读目的位。
        unsafe { std::ptr::write(payload.as_mut_ptr() as *mut Vec<u16>, units) };
        Self {
            payload,
            utf16_len: AtomicU32::new(len),
            tag: TAG_FLAT_U16,
        }
    }

    /// 智能路由构造：单元序列含孤立 surrogate 落 FlatU16，全 well-formed 落
    /// Flat（内容等价、载荷最小）。
    pub fn from_units(units: Vec<u16>) -> Self {
        if units_have_lone_surrogate(&units) {
            Self::new_flat_u16(units)
        } else {
            // 已判定无孤立 surrogate，from_utf16 必成功；Err 臂仅为防御
            // （单元判定与 from_utf16 同语义，不可能分歧）。
            match String::from_utf16(&units) {
                Ok(s) => Self::new(s),
                Err(_) => Self::new_flat_u16(units),
            }
        }
    }

    /// 以左右子节点构造 Cons（rope）节点，O(1) 链接不拷贝文本。
    ///
    /// # Safety
    /// `left`/`right` 必须指向存活的 `JsString`（session 或 perm），且由 GC
    /// 传播保证随本节点存活——调用方（`Vm::new_cons_string`）负责登记。
    pub unsafe fn new_cons(left: *const JsString, right: *const JsString) -> Self {
        // 子节点单元长各 O(1)（Flat 懒扫描首取后缓存 / FlatU16 直读 / Cons 归纳）：
        // 链接成本 = 子节点首取扫描，非本链 O(n)。
        let unit_len = (*left).utf16_len() + (*right).utf16_len();
        let node = Box::into_raw(Box::new(ConsNode {
            left,
            right,
            unit_len,
            flat_cache: OnceLock::new(),
        }));
        let mut payload = [0u64; 3];
        // SAFETY: Cons 载荷首字为节点指针，余 16B 未定义填充；指针按位写入
        // （目标小端，指针→u64 为逐位转换）。
        unsafe { *payload.as_mut_ptr() = node as u64 };
        Self {
            payload,
            utf16_len: AtomicU32::new(unit_len),
            tag: TAG_CONS,
        }
    }

    /// Cons 载荷节点指针（非 Cons 返回 null）。仅供释放路径使用。
    pub fn cons_node_ptr(&self) -> *mut ConsNode {
        if self.tag != TAG_CONS {
            return std::ptr::null_mut();
        }
        // SAFETY: tag 为 CONS 时载荷首字即节点指针，由 new_cons 创建且存活。
        self.cons_ptr() as *mut ConsNode
    }

    /// 释放 Cons 载荷节点（连带扁平化产物）。须在所属 `JsString` 的
    /// `Box::from_raw` 之前调用且恰好一次；非 Cons 串传 null 无操作。
    ///
    /// # Safety
    /// `node` 必须是 [`JsString::new_cons`] 经 `Box::into_raw` 产生的指针，且
    /// 尚未被释放。
    pub unsafe fn drop_cons_node(node: *mut ConsNode) {
        if node.is_null() {
            return;
        }
        let flat = (*node).flat_cache_ptr();
        if !flat.is_null() {
            // SAFETY: 产物由本节点 Box::into_raw 创建，且只随本节点释放一次。
            drop(Box::from_raw(flat as *mut JsString));
        }
        drop(Box::from_raw(node));
    }

    /// JS 字符串长度（UTF-16 code unit 数）。Flat 首次访问扫描并缓存，
    /// 此后恒 O(1)；FlatU16/Cons 构造时已预写。
    pub fn utf16_len(&self) -> u32 {
        if self.tag != TAG_FLAT {
            return self.utf16_len.load(Ordering::Relaxed);
        }
        let cached = self.utf16_len.load(Ordering::Relaxed);
        if cached != UTF16_LEN_UNSET {
            return cached;
        }
        let s = self.flat_ref();
        let len = if s.is_ascii() { s.len() as u32 } else { s.encode_utf16().count() as u32 };
        self.utf16_len.store(len, Ordering::Relaxed);
        len
    }

    /// 是否为空字符串。
    pub fn is_empty(&self) -> bool {
        self.utf16_len() == 0
    }

    /// 整块 UTF-8 文本的借用。**仅 Flat 形态**：FlatU16/Cons 无 `&str` 视图
    /// （内容含孤立 surrogate 或需扁平化），一律改走 [`Self::units`] /
    /// [`Self::as_lossy_str`]。
    ///
    /// # Panics
    /// 非 Flat 形态 panic（调用契约违反，非运行时正常路径）。
    pub fn as_str(&self) -> &str {
        if self.tag != TAG_FLAT {
            panic!("JsString::as_str 仅限 Flat 形态");
        }
        self.flat_ref()
    }

    /// FlatU16 单元载荷的零拷贝借用；仅 FlatU16 形态返回 Some
    /// （Flat 需编码、Cons 需扁平化，热路径经本方法免判形态）。
    pub fn units_borrowed(&self) -> Option<&[u16]> {
        (self.tag == TAG_FLAT_U16).then(|| self.flat_u16_ref()).map(|v| &**v)
    }

    /// 整块单元序列视图：FlatU16 零拷贝；Flat 即时编码（ASCII 走 u8→u16）；
    /// Cons 未扁平化时迭代展开（显式栈中序，防深链爆栈）发布 FlatU16 产物缓存
    /// 后返回其零拷贝借用（缓存随本节点存活）。
    pub fn units(&self) -> std::borrow::Cow<'_, [u16]> {
        match self.tag {
            TAG_FLAT => {
                let s = self.flat_ref();
                if s.is_ascii() {
                    std::borrow::Cow::Owned(s.as_bytes().iter().map(|&b| b as u16).collect())
                } else {
                    std::borrow::Cow::Owned(s.encode_utf16().collect())
                }
            }
            TAG_FLAT_U16 => std::borrow::Cow::Borrowed(self.flat_u16_ref()),
            TAG_CONS => {
                let node = unsafe { &*self.cons_ptr() };
                if let Some(cached) = node.flat_cache.get() {
                    // SAFETY: 产物由本节点 Box::into_raw 创建且只随本节点释放，
                    // 存活期覆盖本次 &self 借用；扁平化产物恒为 FlatU16 形态。
                    let cached = unsafe { &**cached };
                    return std::borrow::Cow::Borrowed(cached.flat_u16_ref());
                }
                let units = self.flatten_units();
                let product = Box::into_raw(Box::new(JsString::new_flat_u16(units)));
                if node.flat_cache.set(product).is_err() {
                    // 并发首用竞态：另一线程已发布，本线程产物未暴露给任何调用方，
                    // 恰好释放一次。
                    // SAFETY: product 来自本线程的 Box::into_raw，无外部引用。
                    unsafe { drop(Box::from_raw(product)) };
                }
                // 上述 set 后缓存必已发布（本线程或他线程），产物恒 FlatU16。
                let cached = unsafe { &**node.flat_cache.get().unwrap() };
                std::borrow::Cow::Borrowed(cached.flat_u16_ref())
            }
            _ => unreachable!(),
        }
    }

    /// 整块文本的 lossy 借用：Flat 直读（内容 well-formed，无损）；其余形态按
    /// 单元→char 映射（孤立 surrogate 替 U+FFFD）——供展示/解析/错误消息，
    /// 语义消费一律走 [`Self::units`]。
    pub fn as_lossy_str(&self) -> std::borrow::Cow<'_, str> {
        if self.tag == TAG_FLAT {
            return std::borrow::Cow::Borrowed(self.flat_ref());
        }
        let units = self.units();
        std::borrow::Cow::Owned(match String::from_utf16(&units) {
            Ok(s) => s,
            Err(_) => units.iter().map(|&u| char::from_u32(u as u32).unwrap_or('\u{FFFD}')).collect(),
        })
    }

    /// 整块文本的 lossy owned 副本（低频路径：错误消息 / 格式化 / 源码桥接）。
    pub fn to_owned_string(&self) -> String {
        self.as_lossy_str().into_owned()
    }

    /// 是否含孤立 surrogate 单元（well-formed 判定的取反）。
    ///
    /// Flat 恒 false（不变式：UTF-8 载荷结构上不可承载）；FlatU16 恒 true
    /// （保守口径：直构造路径可能持有 well-formed 单元，单元消费路径对
    /// well-formed 内容结果仍正确，仅多走单元通道）；Cons 扁平化后扫描。
    pub fn has_lone_surrogate(&self) -> bool {
        match self.tag {
            TAG_FLAT => false,
            TAG_FLAT_U16 => true,
            TAG_CONS => {
                let units = self.units();
                units_have_lone_surrogate(&units)
            }
            _ => unreachable!(),
        }
    }

    /// 载荷的记账字节数（GC 账目口径）：Flat = UTF-8 内容字节；FlatU16 /
    /// Cons = 单元数 × 2。
    pub fn payload_bytes(&self) -> usize {
        match self.tag {
            TAG_FLAT => self.flat_ref().len(),
            TAG_FLAT_U16 => self.flat_u16_ref().len() * 2,
            TAG_CONS => (unsafe { (*self.cons_ptr()).unit_len as usize }) * 2,
            _ => unreachable!(),
        }
    }

    /// 是否为 Flat（UTF-8 文本）形态。
    pub fn is_flat(&self) -> bool {
        self.tag == TAG_FLAT
    }

    /// 是否为 FlatU16（单元载荷）形态。
    pub fn is_flat_u16(&self) -> bool {
        self.tag == TAG_FLAT_U16
    }

    /// 是否为 Cons（rope）节点。
    pub fn is_cons(&self) -> bool {
        self.tag == TAG_CONS
    }

    /// Cons 左右子节点指针对（非 Cons 返回双 null）。仅供 GC 传播闭包使用。
    pub fn cons_children(&self) -> [*const JsString; 2] {
        if self.tag != TAG_CONS {
            return [std::ptr::null(), std::ptr::null()];
        }
        let node = unsafe { &*self.cons_ptr() };
        [node.left, node.right]
    }

    /// 扁平化产物指针（未扁平化返回 null）。仅供 GC 传播与释放路径使用。
    pub fn flat_cache_ptr(&self) -> *const JsString {
        if self.tag != TAG_CONS {
            return std::ptr::null();
        }
        unsafe { (*self.cons_ptr()).flat_cache_ptr() }
    }

    /// 迭代展开 Cons 子树为整块单元序列（显式栈中序收集，防深链递归爆栈）。
    fn flatten_units(&self) -> Vec<u16> {
        let mut stack = Vec::with_capacity(8);
        stack.push(self as *const JsString);
        let mut buf: Vec<u16> = Vec::with_capacity(self.utf16_len() as usize);
        while let Some(ptr) = stack.pop() {
            // SAFETY: 树内节点由 GC 传播保证随本节点存活；展开期子树无并发修改。
            let node = unsafe { &*ptr };
            match node.tag {
                TAG_FLAT => buf.extend(node.flat_ref().encode_utf16()),
                TAG_FLAT_U16 => buf.extend_from_slice(node.flat_u16_ref()),
                TAG_CONS => {
                    // 右子先入栈、左子后入，保证左子先出（中序顺序）。
                    // SAFETY: child 由 new_cons 创建且随父存活。
                    let child = unsafe { &*node.cons_ptr() };
                    stack.push(child.right);
                    stack.push(child.left);
                }
                _ => unreachable!(),
            }
        }
        buf
    }
}

/// 扫描单元序列是否含孤立 surrogate（high 后须紧跟 low 成对；low 不得独立出现）。
fn units_have_lone_surrogate(units: &[u16]) -> bool {
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if (0xD800..=0xDBFF).contains(&u) {
            if i + 1 < units.len() && (0xDC00..=0xDFFF).contains(&units[i + 1]) {
                i += 2;
            } else {
                return true;
            }
        } else if (0xDC00..=0xDFFF).contains(&u) {
            return true;
        } else {
            i += 1;
        }
    }
    false
}

// SAFETY: 与 `StringPtr`（string_forge.rs）同款论证——Cons 节点只存在于单线程
// session VM 内；跨线程共享的 perm 串载荷为 Flat/FlatU16（无 ConsNode 裸指针
// 写路径），flat_cache 永不写入（perm 串只读共享，无任何可变路径）。裸指针
// 字段不引入跨线程数据竞争。
unsafe impl Send for JsString {}
unsafe impl Sync for JsString {}

#[cfg(test)]
mod tests;
