//! 对象模型：字符串、对象、属性元数据、闭包 cell 与原生函数指针。
//!
//! `JsObject` 为定长 `repr(C)` 结构（112 字节 + 对齐），核心字段内联在
//! header 位域中；dense 属性向量、属性元数据与 upvalue cell 列表通过裸指针
//! 挂在堆上，由 VM / GC 负责生命周期。本模块同时定义属性标志
//! (`PropAttributes`)、访问器/数据属性元数据 (`PropMetaEntry`) 与
//! 闭包共享 cell (`Cell`)。

use crate::value::JsValue;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

/// 堆分配的 JS 字符串值。
///
/// 字符串*值*以 48 位指针（指向 `JsString`）NaN-box（见 `JsValue::string`）。
///
/// 两种变体：
/// - `Flat`：整块 UTF-8 文本（String，保留容量）。字面量、方法结果与 CONCAT_N
///   产物保持此形态；
/// - `Cons`（rope）：指向 [`ConsNode`] 载荷（左右子裸指针 + O(1) 字节长 +
///   惰性扁平化缓存）。二元 `+`/`+=` 拼接 O(1) 链接不拷贝文本，整块文本在
///   首次消费时扁平化并原子发布。
///
/// 结构体大小恒为 40B（`Flat(String)` 24B + utf16_len 4B + tag），较 rope 前的
/// 32B 大一个分配 bin 级，但换来 `String` 原样存储（零 realloc 收缩）；Cons
/// 专属状态（子节点/产物缓存）独立分配在 [`ConsNode`]，仅大链持有。
///
/// 生命周期约定：
/// - `Cons` 子节点各自独立登记 session 字符串表（或为 perm 串），由 GC 的
///   mark 传播闭包保证随父存活；扁平化产物只挂在 `ConsNode::flat_cache`、不进
///   session 主表，随本节点连带释放。
/// - 地址稳定不搬移（Box 堆分配），GC 无需 forwarding / rewrite。
#[derive(Debug)]
pub struct JsString {
    kind: StringKind,
    /// JS 语义的字符串长度（UTF-16 code unit 数）懒缓存。
    ///
    /// 构造是热路径（拼接/格式化），长度查询少见，故不预付 O(n) 扫描；首次
    /// `utf16_len()` 访问时计算一次。以 sentinel + `AtomicU32` 实现：perm 字符串
    /// 可被多线程 VM 共享读，计算幂等，relaxed 序即可。
    utf16_len: AtomicU32,
}

/// 字符串内容变体。
#[derive(Debug)]
enum StringKind {
    Flat(String),
    Cons(*const ConsNode),
}

/// Cons（rope）节点的载荷：左右子节点指针 + O(1) 拼接字节长 + 扁平化产物缓存。
///
/// 独立于 `JsString` 分配，使 Flat 路径的 `JsString` 保持与 rope 前相同的 32B
/// 结构；仅超过字节阈值的拼接（`Vm::new_cons_string`）才创建本节点。
///
/// 生命周期约定：节点由 `Box::into_raw` 分配，只经 [`JsString::drop_cons_node`]
/// 释放（连带扁平化产物）；`left`/`right` 由 GC 传播闭包保证随父存活。
#[derive(Debug)]
pub struct ConsNode {
    left: *const JsString,
    right: *const JsString,
    /// 拼接字节长：构造时 = left.len() + right.len()（各 O(1)），免递归求长。
    byte_len: u32,
    /// 扁平化产物缓存：首次文本消费时分配整块 Flat 产物并原子发布（OnceLock）。
    /// 产物不进 session 主表，生命周期挂本节点——GC 传播保证其随父存活，
    /// 释放节点时连带释放。
    flat_cache: OnceLock<*const JsString>,
}

/// `utf16_len` 未计算的哨兵值（有效长度不可能为 u32::MAX）。
const UTF16_LEN_UNSET: u32 = u32::MAX;

impl JsString {
    /// 用 UTF-8 数据构造 Flat 字符串。
    pub fn new(data: String) -> Self {
        Self {
            kind: StringKind::Flat(data),
            utf16_len: AtomicU32::new(UTF16_LEN_UNSET),
        }
    }

    /// 以左右子节点构造 Cons（rope）节点，O(1) 链接不拷贝文本。
    ///
    /// # Safety
    /// `left`/`right` 必须指向存活的 `JsString`（session 或 perm），且由 GC
    /// 传播保证随本节点存活——调用方（`Vm::new_cons_string`）负责登记。
    pub unsafe fn new_cons(left: *const JsString, right: *const JsString) -> Self {
        let byte_len = ((*left).len() + (*right).len()) as u32;
        let node = Box::into_raw(Box::new(ConsNode {
            left,
            right,
            byte_len,
            flat_cache: OnceLock::new(),
        }));
        Self {
            kind: StringKind::Cons(node),
            utf16_len: AtomicU32::new(UTF16_LEN_UNSET),
        }
    }

    /// Cons 载荷节点指针（Flat 返回 null）。仅供释放路径使用。
    pub fn cons_node_ptr(&self) -> *mut ConsNode {
        match &self.kind {
            StringKind::Flat(_) => std::ptr::null_mut(),
            StringKind::Cons(node) => *node as *mut ConsNode,
        }
    }

    /// 释放 Cons 载荷节点（连带扁平化产物）。须在所属 `JsString` 的
    /// `Box::from_raw` 之前调用且恰好一次；Flat 串传 null 无操作。
    ///
    /// # Safety
    /// `node` 必须是 [`JsString::new_cons`] 经 `Box::into_raw` 产生的指针，且
    /// 尚未被释放。
    pub unsafe fn drop_cons_node(node: *mut ConsNode) {
        if node.is_null() {
            return;
        }
        let flat = (*node).flat_cache.get().copied().unwrap_or(std::ptr::null());
        if !flat.is_null() {
            // SAFETY: 产物由本节点 Box::into_raw 创建，且只随本节点释放一次。
            drop(Box::from_raw(flat as *mut JsString));
        }
        drop(Box::from_raw(node));
    }

    /// 字符串的字节长度（非字符数）。Flat 直读；Cons O(1) 取构造时缓存。
    pub fn len(&self) -> usize {
        match &self.kind {
            StringKind::Flat(data) => data.len(),
            StringKind::Cons(node) => {
                // SAFETY: node 由 new_cons 创建且存活期覆盖本借用。
                unsafe { (**node).byte_len as usize }
            }
        }
    }

    /// JS 字符串长度（UTF-16 code unit 数）。首次访问计算并缓存，此后 O(1)。
    /// Cons 首次访问触发扁平化（见 [`Self::flat_str`]），取产物长度缓存。
    pub fn utf16_len(&self) -> u32 {
        let cached = self.utf16_len.load(Ordering::Relaxed);
        if cached != UTF16_LEN_UNSET {
            return cached;
        }
        let len = self.flat_str().encode_utf16().count() as u32;
        self.utf16_len.store(len, Ordering::Relaxed);
        len
    }

    /// 是否为空字符串。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 整块 UTF-8 文本的借用。Cons 未扁平化时惰性扁平化并发布缓存后返回产物文本；
    /// 返回的 `&str` 借用期 = `&self` 借用期（产物随本节点存活）。
    pub fn as_str(&self) -> &str {
        self.flat_str()
    }

    /// 整块文本的借用（`as_str` 的显式别名）：Flat 直读零开销；Cons 读
    /// `flat_cache`，未命中则迭代展开扁平化（显式栈防深链爆栈）后原子发布。
    pub fn flat_str(&self) -> &str {
        match &self.kind {
            StringKind::Flat(data) => data.as_str(),
            StringKind::Cons(node) => {
                // SAFETY: node 由 new_cons 创建且存活期覆盖本借用。
                let node = unsafe { &**node };
                if let Some(flat) = node.flat_cache.get() {
                    // SAFETY: 产物由本节点 Box::into_raw 创建且只随本节点释放，
                    // 存活期覆盖本次 &self 借用。
                    let flat = unsafe { &**flat };
                    return match &flat.kind {
                        StringKind::Flat(data) => data.as_str(),
                        StringKind::Cons(_) => unreachable!("扁平化产物恒为 Flat"),
                    };
                }
                let text = self.flatten_text();
                let flat_ptr = Box::into_raw(Box::new(JsString::new(text)));
                if node.flat_cache.set(flat_ptr).is_err() {
                    // 并发首用竞态：另一线程已发布，本线程产物未暴露给任何调用方，
                    // 恰好释放一次。
                    // SAFETY: flat_ptr 来自本线程的 Box::into_raw，无外部引用。
                    unsafe { drop(Box::from_raw(flat_ptr)) };
                }
                // SAFETY: 上述 set 后缓存必已发布（本线程或他线程）。
                let flat = unsafe { &**node.flat_cache.get().unwrap() };
                match &flat.kind {
                    StringKind::Flat(data) => data.as_str(),
                    StringKind::Cons(_) => unreachable!("扁平化产物恒为 Flat"),
                }
            }
        }
    }

    /// 整块文本的 owned 副本（低频路径：错误消息 / 格式化 / 属性键桥接）。
    pub fn to_owned_string(&self) -> String {
        self.flat_str().to_string()
    }

    /// 是否为 Cons（rope）节点。
    pub fn is_cons(&self) -> bool {
        matches!(self.kind, StringKind::Cons(_))
    }

    /// Cons 左右子节点指针对（Flat 返回双 null）。仅供 GC 传播闭包使用。
    pub fn cons_children(&self) -> [*const JsString; 2] {
        match &self.kind {
            StringKind::Flat(_) => [std::ptr::null(), std::ptr::null()],
            StringKind::Cons(node) => {
                // SAFETY: node 由 new_cons 创建且存活期覆盖本借用。
                let node = unsafe { &**node };
                [node.left, node.right]
            }
        }
    }

    /// 扁平化产物指针（未扁平化返回 null）。仅供 GC 传播与释放路径使用。
    pub fn flat_cache_ptr(&self) -> *const JsString {
        match &self.kind {
            StringKind::Flat(_) => std::ptr::null(),
            StringKind::Cons(node) => {
                // SAFETY: node 由 new_cons 创建且存活期覆盖本借用。
                unsafe { (**node).flat_cache.get().copied().unwrap_or(std::ptr::null()) }
            }
        }
    }

    /// 迭代展开 Cons 子树为整块文本（显式栈中序收集，防深链递归爆栈）。
    fn flatten_text(&self) -> String {
        let mut stack = Vec::with_capacity(8);
        stack.push(self as *const JsString);
        let mut buf = String::with_capacity(self.len());
        while let Some(ptr) = stack.pop() {
            // SAFETY: 树内节点由 GC 传播保证随本节点存活；展开期子树无并发修改。
            let node = unsafe { &*ptr };
            match &node.kind {
                StringKind::Flat(data) => buf.push_str(data.as_str()),
                StringKind::Cons(child) => {
                    // 右子先入栈、左子后入，保证左子先出（中序顺序）。
                    // SAFETY: child 由 new_cons 创建且随父存活。
                    let child = unsafe { &**child };
                    stack.push(child.right);
                    stack.push(child.left);
                }
            }
        }
        buf
    }
}

// SAFETY: 与 `StringPtr`（string_forge.rs）同款论证——Cons 节点只存在于单线程
// session VM 内；跨线程共享的 perm 串恒为 Flat，且其 flat_cache 永不写入
// （perm 串只读共享，无任何可变路径）。裸指针字段不引入跨线程数据竞争。
unsafe impl Send for JsString {}
unsafe impl Sync for JsString {}

/// 原生函数指针的类型安全不透明包装。
///
/// 以 `*const ()` 存储而非具体 `fn` 类型，使 `oxide_types` 无需依赖
/// `oxide_vm::Vm`。`oxide_vm` 中的调用方经 `NativeFnPtr::call_with` 转回
/// `NativeFn`——transmute 被限制在单个泛型辅助函数中。
///
/// # Safety 不变量
///
/// `NativeFnPtr` 必须总是由合法的 `NativeFn` 函数指针（裸 `fn` 项或函数项
/// 强制转换——**不是**闭包）创建。指针永不为空。`Send + Sync` 安全是因为
/// 函数项指针天然线程安全（不含数据）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct NativeFnPtr(pub *const ());

impl NativeFnPtr {
    /// 包装裸函数指针。指针必须指向合法的 `NativeFn` 函数项。
    ///
    /// # Safety
    /// `ptr` 必须是类型为 `fn(&mut Vm, &[u8]) -> NativeResult` 的非空函数指针
    /// 转成的 `*const ()`。使用任何其它指针值在调用时是 UB。
    #[inline(always)]
    pub unsafe fn from_raw(ptr: *const ()) -> Self {
        debug_assert!(!ptr.is_null(), "NativeFnPtr must not be null");
        Self(ptr)
    }

    /// 返回底层裸指针。
    #[inline(always)]
    pub fn as_ptr(self) -> *const () {
        self.0
    }
}

// SAFETY: 函数项指针不含可变状态，可安全跨线程共享。
unsafe impl Send for NativeFnPtr {}
unsafe impl Sync for NativeFnPtr {}

/// 形状标识符（对象 header 低位 24 位）。
pub type ShapeId = u32;

/// dense 属性向量长度的硬上限，防止索引失控导致内存膨胀。
pub const MAX_DENSE_PROPS: usize = 1_000_000;

/// TypedArray 的元素类型，决定 `bytes_per_element` 与内存视图的字节序解读。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedArrayKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl TypedArrayKind {
    /// 每个元素的字节数（1/2/4/8）。
    pub const fn bytes_per_element(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 | Self::BigInt64 | Self::BigUint64 => 8,
        }
    }

    /// 对应的具体构造器名（如 `Int16Array`），供 `@@toStringTag` getter 返回。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8Array",
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int16 => "Int16Array",
            Self::Uint16 => "Uint16Array",
            Self::Int32 => "Int32Array",
            Self::Uint32 => "Uint32Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
            Self::BigInt64 => "BigInt64Array",
            Self::BigUint64 => "BigUint64Array",
        }
    }
}

/// 属性描述符标志位集合，压缩在单个 `u8` 中。
///
/// 位定义：bit0 = writable，bit1 = enumerable，bit2 = configurable。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropAttributes(pub u8);

impl PropAttributes {
    /// writable 标志位（bit0）。
    pub const WRITABLE: u8 = 0b001;
    /// enumerable 标志位（bit1）。
    pub const ENUMERABLE: u8 = 0b010;
    /// configurable 标志位（`0b100`）。
    pub const CONFIGURABLE: u8 = 0b100;
    /// 数据属性默认描述符：三标志全开。
    pub const DEFAULT_DATA: Self = Self(Self::WRITABLE | Self::ENUMERABLE | Self::CONFIGURABLE);

    /// 由三个布尔标志构造描述符。
    pub const fn new(writable: bool, enumerable: bool, configurable: bool) -> Self {
        let mut bits = 0;
        if writable {
            bits |= Self::WRITABLE;
        }
        if enumerable {
            bits |= Self::ENUMERABLE;
        }
        if configurable {
            bits |= Self::CONFIGURABLE;
        }
        Self(bits)
    }

    /// 是否 writable。
    pub const fn writable(self) -> bool {
        self.0 & Self::WRITABLE != 0
    }

    /// 是否 enumerable。
    pub const fn enumerable(self) -> bool {
        self.0 & Self::ENUMERABLE != 0
    }

    /// 是否 configurable。
    pub const fn configurable(self) -> bool {
        self.0 & Self::CONFIGURABLE != 0
    }
}

/// 单个属性的元数据条目。
///
/// 数据属性仅用 `attributes`；访问器属性额外携带 getter / setter
/// 的 [`JsValue`] 与 `is_accessor = true` 标记。`hole` 标记数组元素被删除
/// 后保留的稀疏空洞（数组元素区存在性判定依据）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropMetaEntry {
    pub attributes: PropAttributes,
    pub get: JsValue,
    pub set: JsValue,
    pub is_accessor: bool,
    pub hole: bool,
}

impl PropMetaEntry {
    /// 构造数据属性条目。
    pub fn data(attributes: PropAttributes) -> Self {
        Self {
            attributes,
            get: JsValue::undefined(),
            set: JsValue::undefined(),
            is_accessor: false,
            hole: false,
        }
    }

    /// 构造访问器属性条目（getter/setter 可为 `undefined`）。
    pub fn accessor(get: JsValue, set: JsValue, attributes: PropAttributes) -> Self {
        Self {
            attributes,
            get,
            set,
            is_accessor: true,
            hole: false,
        }
    }

    /// 构造数组元素删除后的 hole 标记条目。
    pub fn hole() -> Self {
        Self {
            attributes: PropAttributes::DEFAULT_DATA,
            get: JsValue::undefined(),
            set: JsValue::undefined(),
            is_accessor: false,
            hole: true,
        }
    }

    /// 是否数组元素 hole 标记（删除后保留的稀疏空洞）。
    pub fn is_hole(&self) -> bool {
        self.hole
    }
}

/// 可转换为 dense 属性下标的值。
///
/// 为 `u8` / `u16` / `u32` / `usize` / 非负 `i32` 实现，统一属性向量
/// 下标参数的类型。
pub trait PropIndex {
    fn to_u32(self) -> u32;
}

impl PropIndex for u8 {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for u16 {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for u32 {
    fn to_u32(self) -> u32 {
        self
    }
}

impl PropIndex for usize {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for i32 {
    fn to_u32(self) -> u32 {
        debug_assert!(self >= 0, "property index must be non-negative");
        self.max(0) as u32
    }
}

/// 布局：
///   header: u32 位
///     \[0:23\]   shape_id
///     \[24\]     is_set
///     \[25\]     is_map
///     \[26\]     is_derived_constructor
///     \[27\]     is_class_constructor
///     \[28\]     is_arrow
///     \[29\]     is_array
///     \[30\]     is_extensible
///     \[31\]     is_function
///   native_arg_count: u8 (1 字节)
///   type_tag: u8 — 标识包装/外来对象种类的 OBJ_TYPE_* 常量 (1 字节)
///   is_session_epoch: u8 (1 字节)
///   _pad: u8
///   array_elements: *mut u8 (8 字节，数组对象元素区 Box\<Vec\<JsValue\>\>)
///   array_elements_meta: *mut u8 (8 字节，数组元素元数据 Box\<Vec\<Option\<PropMetaEntry\>\>\>)
///   hash_props: *mut u8 (8 字节，命名属性 Box\<Vec\<JsValue\>\>)
///   prop_meta: *mut u8 (8 字节，命名属性元数据 Box\<Vec\<Option\<PropMetaEntry\>\>\>)
///   native_data: *mut u8 (8 字节，VM 拥有的不透明 native/外来载荷)
///   proto: JsValue (8 字节)
///   generation: u32 (4 字节 + 4 填充)
///   native_fn: Option\<NativeFnPtr\> (16 字节 — 裸 `*const ()` 无法利用 Option\<NonNull\>
///              优化；因 repr(Rust) 布局规则存为包装 8 字节指针的 Option，带 8 字节
///              判别式填充)
///   sub_module_index: u32 (4 字节 + 4 填充，索引 CompiledModule.sub_modules)
///   captured_this: JsValue (8 字节，箭头函数的词法 this)
///   home_object: JsValue (8 字节，\[\[HomeObject\]\]，供 super 查找)
///   upvalues: *mut u8 (8 字节，指向闭包的 Box<Vec<*mut Cell>>)
///
///   总计：112 字节
///   对齐：8 字节
#[repr(C)]
pub struct Cell {
    pub value: JsValue,
    pub flags: u8,
    pub _pad: [u8; 7],
}

impl Cell {
    /// 已初始化标志（TDZ / 未赋值检测）。
    pub const INITIALIZED: u8 = 0x01;
    /// GC 标记位。
    pub const GC_MARK: u8 = 0x02;

    /// 构造 cell，`initialized` 决定是否立即置 [`INITIALIZED`](Cell::INITIALIZED) 位。
    pub fn new(value: JsValue, initialized: bool) -> Self {
        Cell {
            value,
            flags: if initialized { Self::INITIALIZED } else { 0 },
            _pad: [0; 7],
        }
    }

    /// 是否已初始化。
    pub fn is_initialized(&self) -> bool {
        self.flags & Self::INITIALIZED != 0
    }

    /// 设置 / 清除已初始化标志。
    pub fn set_initialized(&mut self, val: bool) {
        if val {
            self.flags |= Self::INITIALIZED;
        } else {
            self.flags &= !Self::INITIALIZED;
        }
    }

    /// 是否被 GC 标记。
    pub fn is_gc_marked(&self) -> bool {
        self.flags & Self::GC_MARK != 0
    }

    /// 设置 / 清除 GC 标记。
    pub fn set_gc_mark(&mut self, marked: bool) {
        if marked {
            self.flags |= Self::GC_MARK;
        } else {
            self.flags &= !Self::GC_MARK;
        }
    }
}

/// 定长对象头 + 堆外数据指针的 JS 普通/外来对象。
///
/// 内联字段：`header`（shape_id + 一组标志位）、`type_tag`（外来对象种类）、
/// `proto`、`generation` 等；dense 属性向量、属性元数据、native payload 与
/// upvalue cell 列表以裸指针挂在堆上，由 VM / GC 维护。对象可分配在
/// session arena（`Epoch`）或全局堆（`Arc`），通过
/// `is_session_epoch` 位区分。
pub struct JsObject {
    header: u32,
    native_arg_count: u8,
    pub type_tag: u8,
    is_session_epoch: u8,
    _pad: u8,
    /// 数组元素区（仅数组对象）：`Vec<JsValue>`，`len == array_prop_count`。
    array_elements: *mut u8,
    /// 数组元素元数据区（仅数组对象，懒分配）：`Vec<Option<PropMetaEntry>>`。
    array_elements_meta: *mut u8,
    hash_props: *mut u8,
    prop_meta: *mut u8,
    native_data: *mut u8,
    proto: JsValue,
    generation: u32,
    /// 数组元素数（数组对象）。普通对象恒 0。命名属性存储在 `hash_props`
    /// （与元素区分，JS 数组命名属性不影响 length）。
    pub array_prop_count: u32,
    /// 数组逻辑长度覆盖：仅当 length 超出 dense 存储上限 `MAX_DENSE_PROPS` 时非 0。
    /// `arr.length = 4294967295` 等超大长度时，元素区仍以 `MAX_DENSE_PROPS` 封顶，
    /// 超出部分视为稀疏空洞，逻辑长度单独记录供 `a.length` 读取。
    array_len_override: u32,
    native_fn: Option<NativeFnPtr>,
    sub_module_index: u32,
    _pad3: [u8; 4],
    captured_this: JsValue,
    home_object: JsValue,
    pub upvalues: *mut u8,
}

impl JsObject {
    /// 普通对象类型标签。
    pub const OBJ_TYPE_PLAIN: u8 = 0;
    /// Date 对象类型标签。
    pub const OBJ_TYPE_DATE: u8 = 1;
    /// RegExp 对象类型标签。
    pub const OBJ_TYPE_REGEXP: u8 = 2;
    /// 装箱 Boolean 对象类型标签。
    pub const OBJ_TYPE_BOOLEAN_OBJ: u8 = 3;
    /// 装箱 Number 对象类型标签。
    pub const OBJ_TYPE_NUMBER_OBJ: u8 = 4;
    /// 装箱 String 对象类型标签。
    pub const OBJ_TYPE_STRING_OBJ: u8 = 5;
    /// ArrayBuffer 对象类型标签。
    pub const OBJ_TYPE_ARRAY_BUFFER: u8 = 6;
    /// DataView 对象类型标签。
    pub const OBJ_TYPE_DATA_VIEW: u8 = 7;
    /// TypedArray 对象类型标签。
    pub const OBJ_TYPE_TYPED_ARRAY: u8 = 8;
    /// native 构造器（Set/Array/Object 等，可 [[Construct]]）；区别于不可构造的 native 方法。
    pub const OBJ_TYPE_CONSTRUCTOR: u8 = 9;
    /// 生成器迭代器对象（`function*` 调用返回）：状态快照存于 `native_data`。
    pub const OBJ_TYPE_GENERATOR: u8 = 10;
    /// Promise 对象：状态盒（Pending/Fulfilled/Rejected + reactions）存于 `native_data`。
    pub const OBJ_TYPE_PROMISE: u8 = 11;
    /// 异步函数执行上下文（隐藏对象）：`AsyncState` 快照存于 `native_data`。
    pub const OBJ_TYPE_ASYNC: u8 = 12;
    /// 异步生成器迭代器对象（`async function*` 调用返回）：`AsyncGeneratorState`
    /// 快照存于 `native_data`。
    pub const OBJ_TYPE_ASYNC_GENERATOR: u8 = 13;
    /// Temporal.Instant 对象：纪元纳秒存于 prop 0。
    pub const OBJ_TYPE_INSTANT: u8 = 14;
    /// Temporal.PlainDate 对象：ISO 年/月/日存于 prop 0-2。
    pub const OBJ_TYPE_PLAIN_DATE: u8 = 15;
    /// Temporal.PlainTime 对象：午夜后纳秒存于 prop 0。
    pub const OBJ_TYPE_PLAIN_TIME: u8 = 16;
    /// Temporal.Duration 对象：十个时长分量依次存于 prop 0-9。
    pub const OBJ_TYPE_DURATION: u8 = 17;
    /// Temporal.ZonedDateTime 对象：纪元纳秒、时区 ID、日历 ID 存于 prop 0-2。
    pub const OBJ_TYPE_ZONED_DATE_TIME: u8 = 18;
    /// Temporal.PlainDateTime 对象：ISO 年/月/日与午夜后纳秒存于 prop 0-3。
    pub const OBJ_TYPE_PLAIN_DATE_TIME: u8 = 19;
    /// Arguments 对象：`Object.prototype.toString` 返回 `[object Arguments]`。
    /// 装箱 Symbol 对象类型标签。
    pub const OBJ_TYPE_SYMBOL_OBJ: u8 = 21;
    pub const OBJ_TYPE_ARGUMENTS: u8 = 20;
    /// Error 家族对象（Error 及 NativeError 实例）：`Object.prototype.toString` 返回
    /// `[object Error]`，与 [[ErrorData]] 内部槽对应。
    pub const OBJ_TYPE_ERROR: u8 = 22;
    /// bound 函数包装（`Function.prototype.bind` 产物）：[[BoundTargetFunction]] /
    /// [[BoundThis]] / [[BoundArguments]] 存于 dense 槽 4/5/6+，构造与 instanceof
    /// 语义转发到 target。
    pub const OBJ_TYPE_BOUND: u8 = 23;
    /// DisposableStack 对象：状态盒（Pending/Disposed + entries）存于 `native_data`。
    pub const OBJ_TYPE_DISPOSABLE_STACK: u8 = 24;
    /// AsyncDisposableStack 对象（异步资源栈）：状态盒与同步栈同构，预留 type_tag。
    pub const OBJ_TYPE_ASYNC_DISPOSABLE_STACK: u8 = 25;
    /// `is_session_epoch` 字段中的 session 标记位。
    pub const SESSION_EPOCH_BIT: u8 = 0x01;
    /// `is_session_epoch` 字段中的 GC 标记位。
    pub const GC_MARK_BIT: u8 = 0x02;
    /// `is_session_epoch` 字段中的 epoch 归属标记位。
    pub const EPOCH_BIT: u8 = 0x04;

    /// 是否 Date 外来对象。
    #[inline]
    pub fn is_date_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_DATE
    }
    /// 是否 RegExp 外来对象。
    #[inline]
    pub fn is_regexp_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_REGEXP
    }
    /// 是否装箱 Boolean 对象。
    #[inline]
    pub fn is_boolean_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_BOOLEAN_OBJ
    }
    /// 是否装箱 Number 对象。
    #[inline]
    pub fn is_number_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_NUMBER_OBJ
    }
    /// 是否装箱 String 对象。
    #[inline]
    pub fn is_string_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_STRING_OBJ
    }

    /// 是否装箱 Symbol 对象。
    #[inline]
    pub fn is_symbol_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_SYMBOL_OBJ
    }
    /// 是否 ArrayBuffer 对象。
    #[inline]
    pub fn is_array_buffer_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ARRAY_BUFFER
    }
    /// 是否 DataView 对象。
    #[inline]
    pub fn is_data_view_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_DATA_VIEW
    }
    /// 是否 TypedArray 对象。
    #[inline]
    pub fn is_typed_array_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_TYPED_ARRAY
    }
    /// 是否生成器迭代器对象。
    #[inline]
    pub fn is_generator_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_GENERATOR
    }
    /// 是否 Promise 对象。
    #[inline]
    pub fn is_promise_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PROMISE
    }
    /// 是否异步函数执行上下文对象。
    #[inline]
    pub fn is_async_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ASYNC
    }
    /// 是否异步生成器迭代器对象。
    #[inline]
    pub fn is_async_generator_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ASYNC_GENERATOR
    }
    /// 是否 Temporal.Instant 对象。
    #[inline]
    pub fn is_instant_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_INSTANT
    }
    /// 是否 Temporal.PlainDate 对象。
    #[inline]
    pub fn is_plain_date_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PLAIN_DATE
    }
    /// 是否 Temporal.PlainTime 对象。
    #[inline]
    pub fn is_plain_time_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PLAIN_TIME
    }
    /// 是否 Temporal.Duration 对象。
    #[inline]
    pub fn is_duration_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_DURATION
    }
    /// 是否 Temporal.ZonedDateTime 对象。
    #[inline]
    pub fn is_zoned_date_time_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ZONED_DATE_TIME
    }
    /// 是否 Temporal.PlainDateTime 对象。
    #[inline]
    pub fn is_plain_date_time_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PLAIN_DATE_TIME
    }

    #[inline]
    pub fn is_arguments_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ARGUMENTS
    }

    /// 是否 Error 家族对象（含 NativeError 实例）。
    #[inline]
    pub fn is_error_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ERROR
    }

    /// 是否 DisposableStack 对象（同步资源栈）。
    #[inline]
    pub fn is_disposable_stack_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_DISPOSABLE_STACK
    }

    /// 是否 AsyncDisposableStack 对象（异步资源栈）。
    #[inline]
    pub fn is_async_disposable_stack_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ASYNC_DISPOSABLE_STACK
    }

    /// 构造无属性、可扩展的空对象（`new Object()` 的基础对象）。
    pub fn new_empty(shape_id: ShapeId, proto: JsValue) -> Self {
        Self {
            header: (shape_id & 0x00FF_FFFF) | (1 << 30),
            native_arg_count: 0,
            type_tag: 0,
            is_session_epoch: 0,
            _pad: 0,
            array_elements: std::ptr::null_mut(),
            array_elements_meta: std::ptr::null_mut(),
            hash_props: std::ptr::null_mut(),
            prop_meta: std::ptr::null_mut(),
            native_data: std::ptr::null_mut(),
            proto,
            generation: 1,
            array_prop_count: 0,
            array_len_override: 0,
            native_fn: None,
            sub_module_index: 0,
            _pad3: [0; 4],
            captured_this: JsValue::undefined(),
            home_object: JsValue::undefined(),
            upvalues: std::ptr::null_mut(),
        }
    }

    /// 构造数组对象：预分配 `n_elements` 个 `undefined` 的独立元素区并置 array 标志。
    pub fn new_array(shape_id: ShapeId, proto: JsValue, n_elements: usize, _bump: &bumpalo::Bump) -> Self {
        let mut obj = Self {
            header: (shape_id & 0x00FF_FFFF) | (1 << 30) | (1 << 29),
            native_arg_count: 0,
            type_tag: 0,
            is_session_epoch: 0,
            _pad: 0,
            array_elements: std::ptr::null_mut(),
            array_elements_meta: std::ptr::null_mut(),
            hash_props: std::ptr::null_mut(),
            prop_meta: std::ptr::null_mut(),
            native_data: std::ptr::null_mut(),
            proto,
            generation: 1,
            array_prop_count: 0,
            array_len_override: 0,
            native_fn: None,
            sub_module_index: 0,
            _pad3: [0; 4],
            captured_this: JsValue::undefined(),
            home_object: JsValue::undefined(),
            upvalues: std::ptr::null_mut(),
        };
        let vec = Box::new(vec![JsValue::undefined(); n_elements.min(MAX_DENSE_PROPS)]);
        obj.array_elements = Box::into_raw(vec) as *mut u8;
        obj.array_prop_count = n_elements.min(MAX_DENSE_PROPS) as u32;
        // 逻辑长度单独记录（超过 dense 上限时），供 a.length 读取。
        obj.array_len_override = if n_elements > MAX_DENSE_PROPS { n_elements as u32 } else { 0 };
        obj
    }

    /// 是否 session-epoch 分配（调用期内分配，调用结束即失效）。
    #[inline]
    pub fn is_session_epoch(&self) -> bool {
        self.is_session_epoch & Self::SESSION_EPOCH_BIT != 0
    }

    /// 设置 / 清除 session-epoch 标记。
    #[inline]
    pub fn set_session_epoch(&mut self, value: bool) {
        if value {
            self.is_session_epoch |= Self::SESSION_EPOCH_BIT;
        } else {
            self.is_session_epoch &= !Self::SESSION_EPOCH_BIT;
        }
    }

    /// 是否被 GC 标记。
    #[inline]
    pub fn is_gc_marked(&self) -> bool {
        self.is_session_epoch & Self::GC_MARK_BIT != 0
    }

    /// 设置 / 清除 GC 标记。
    #[inline]
    pub fn set_gc_mark(&mut self, marked: bool) {
        if marked {
            self.is_session_epoch |= Self::GC_MARK_BIT;
        } else {
            self.is_session_epoch &= !Self::GC_MARK_BIT;
        }
    }

    /// 是否为 epoch 分配（非 session）的对象。
    #[inline]
    pub fn is_epoch(&self) -> bool {
        self.is_session_epoch & Self::EPOCH_BIT != 0
    }

    /// 设置 / 清除 epoch 归属标记。
    #[inline]
    pub fn set_is_epoch(&mut self, value: bool) {
        if value {
            self.is_session_epoch |= Self::EPOCH_BIT;
        } else {
            self.is_session_epoch &= !Self::EPOCH_BIT;
        }
    }

    /// 深拷贝本对象到 session epoch：复制属性向量与元数据，标记为新 session 对象。
    ///
    /// 用于把持久对象快照进当前调用上下文，修改不反向传播到源对象。
    pub fn clone_for_session_epoch(&self) -> Self {
        let hash_props = self
            .hash_props_vec()
            .map(|props| Box::into_raw(Box::new(props.clone())) as *mut u8)
            .unwrap_or(std::ptr::null_mut());
        let prop_meta = self
            .prop_meta_vec()
            .map(|meta| Box::into_raw(Box::new(meta.clone())) as *mut u8)
            .unwrap_or(std::ptr::null_mut());
        let array_elements = self
            .array_elements_vec()
            .map(|elems| Box::into_raw(Box::new(elems.clone())) as *mut u8)
            .unwrap_or(std::ptr::null_mut());
        let array_elements_meta = self
            .array_elements_meta_vec()
            .map(|meta| Box::into_raw(Box::new(meta.clone())) as *mut u8)
            .unwrap_or(std::ptr::null_mut());

        Self {
            header: self.header,
            native_arg_count: self.native_arg_count,
            type_tag: self.type_tag,
            is_session_epoch: Self::SESSION_EPOCH_BIT,
            _pad: self._pad,
            array_elements,
            array_elements_meta,
            hash_props,
            prop_meta,
            native_data: self.native_data,
            proto: self.proto,
            generation: self.generation,
            array_prop_count: self.array_prop_count,
            array_len_override: self.array_len_override,
            native_fn: self.native_fn,
            sub_module_index: self.sub_module_index,
            _pad3: self._pad3,
            captured_this: self.captured_this,
            home_object: self.home_object,
            upvalues: self.upvalues,
        }
    }

    /// dense 属性向量底层指针（未分配时为空指针）。
    pub fn hash_props_raw(&self) -> *mut u8 {
        self.hash_props
    }

    /// 数组元素区底层指针（未分配时为空指针）。
    pub fn array_elements_raw(&self) -> *mut u8 {
        self.array_elements
    }

    /// 数组元素元数据区底层指针（未分配时为空指针）。
    pub fn array_elements_meta_raw(&self) -> *mut u8 {
        self.array_elements_meta
    }

    /// 属性元数据向量底层指针（未分配时为空指针）。
    pub fn prop_meta_raw(&self) -> *mut u8 {
        self.prop_meta
    }

    /// 释放四处堆外裸指针区（命名属性向量/元数据、数组元素区/元素元数据）并置空。
    ///
    /// 供 session 收尾调用：对象本体可能仍被 Arc 引用（此后属性区不再被读取），
    /// 每区至多释放一次、重复调用为 no-op。upvalue 列表不在本函数口径内
    /// （原件与晋升克隆间别名，须收尾时去重统一释放）。
    pub fn release_raw_heap(&mut self) {
        // SAFETY: 四个区各由 ensure_* 路径经 Box::into_raw 分配一次（或为空指针）；
        // Box::from_raw 回收后即刻置空，杜绝二次释放。
        unsafe {
            if !self.array_elements.is_null() {
                drop(Box::from_raw(self.array_elements as *mut Vec<JsValue>));
                self.array_elements = std::ptr::null_mut();
            }
            if !self.array_elements_meta.is_null() {
                drop(Box::from_raw(self.array_elements_meta as *mut Vec<Option<PropMetaEntry>>));
                self.array_elements_meta = std::ptr::null_mut();
            }
            if !self.hash_props.is_null() {
                drop(Box::from_raw(self.hash_props as *mut Vec<JsValue>));
                self.hash_props = std::ptr::null_mut();
            }
            if !self.prop_meta.is_null() {
                drop(Box::from_raw(self.prop_meta as *mut Vec<Option<PropMetaEntry>>));
                self.prop_meta = std::ptr::null_mut();
            }
        }
    }

    /// 原生 / 外来对象 payload 指针。
    pub fn native_data(&self) -> *mut u8 {
        self.native_data
    }

    /// 设置原生 / 外来对象 payload 指针。
    pub fn set_native_data(&mut self, ptr: *mut u8) {
        self.native_data = ptr;
    }

    /// 闭包 upvalue cell 列表切片（未设置时为空切片）。
    pub fn upvalues_slice(&self) -> &[*mut Cell] {
        if self.upvalues.is_null() {
            &[]
        } else {
            unsafe { &*(self.upvalues as *const Vec<*mut Cell>) }
        }
    }

    /// 可变 upvalue cell 列表切片（未设置时为空切片）。
    pub fn upvalues_slice_mut(&mut self) -> &mut [*mut Cell] {
        if self.upvalues.is_null() {
            &mut []
        } else {
            unsafe { &mut *(self.upvalues as *mut Vec<*mut Cell>) }
        }
    }

    /// 替换 upvalue cell 列表（释放旧列表）。
    pub fn set_upvalues(&mut self, v: Box<Vec<*mut Cell>>) {
        if !self.upvalues.is_null() {
            unsafe {
                drop(Box::from_raw(self.upvalues as *mut Vec<*mut Cell>));
            }
        }
        self.upvalues = Box::into_raw(v) as *mut u8;
    }

    /// 用 `rewrite` 改写对象内引用的所有对象值。
    ///
    /// 用于 GC 移动 / 世代晋升：遍历数组元素区、dense 属性、访问器 getter/setter、
    /// `proto`、`captured_this`、`home_object` 与 upvalue cell 中的对象值，
    /// 原地替换为新地址。非对象值保持不变。
    pub fn rewrite_object_values<F>(&mut self, mut rewrite: F)
    where
        F: FnMut(JsValue) -> JsValue,
    {
        if let Some(elements) = self.array_elements_vec_mut() {
            for value in elements {
                if value.is_object() {
                    *value = rewrite(*value);
                }
            }
        }
        if let Some(meta) = self.array_elements_meta_vec_mut() {
            for entry in meta.iter_mut().flatten() {
                if entry.get.is_object() {
                    entry.get = rewrite(entry.get);
                }
                if entry.set.is_object() {
                    entry.set = rewrite(entry.set);
                }
            }
        }
        if let Some(props) = self.hash_props_vec_mut() {
            for value in props {
                if value.is_object() {
                    *value = rewrite(*value);
                }
            }
        }
        if let Some(meta) = self.prop_meta_vec_mut() {
            for entry in meta.iter_mut().flatten() {
                if entry.get.is_object() {
                    entry.get = rewrite(entry.get);
                }
                if entry.set.is_object() {
                    entry.set = rewrite(entry.set);
                }
            }
        }
        if self.proto.is_object() {
            self.proto = rewrite(self.proto);
        }
        if self.captured_this.is_object() {
            self.captured_this = rewrite(self.captured_this);
        }
        if self.home_object.is_object() {
            self.home_object = rewrite(self.home_object);
        }
        if !self.upvalues.is_null() {
            let cells = unsafe { &mut *(self.upvalues as *mut Vec<*mut Cell>) };
            for cell_ptr in cells {
                let cell = unsafe { &mut **cell_ptr };
                if cell.value.is_object() {
                    cell.value = rewrite(cell.value);
                }
            }
        }
    }

    /// 当前形状 ID（header 低位 24 位）。
    pub fn shape_id(&self) -> ShapeId {
        self.header & 0x00FF_FFFF
    }

    /// 设置形状 ID（低位 24 位，保留其它标志位）。
    pub fn set_shape_id(&mut self, id: ShapeId) {
        self.header = (self.header & !0x00FF_FFFF) | (id & 0x00FF_FFFF);
    }

    /// 返回 hash_props vec 的长度作为属性数。
    /// hash_props 未分配时返回 0。
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

    /// 同 `set_prop_count`，但假定 `hash_props` 已分配。
    /// 调用方须保证对象至少含一个元素（或经 `new_array` 创建）。
    /// 用于热路径数组 builtin，跳过每次变更时冗余的 `ensure_hash_props` 空检查。
    #[inline]
    pub fn set_prop_count_fast(&mut self, count: impl PropIndex) {
        self.set_prop_count(count);
    }

    /// 确保属性元数据向量已分配并返回可变引用。
    ///
    /// 初始长度与当前 dense 属性数对齐，全部初始化为 `None`（无元数据）。
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

    fn prop_meta_vec_mut(&mut self) -> Option<&mut Vec<Option<PropMetaEntry>>> {
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

    /// 是否数组（header bit 29）。
    pub fn is_array(&self) -> bool {
        (self.header >> 29) & 1 != 0
    }

    /// 是否可扩展（header bit 30，与 `Object.preventExtensions` 对应）。
    pub fn is_extensible(&self) -> bool {
        (self.header >> 30) & 1 != 0
    }

    /// 设置可扩展标志。
    pub fn set_extensible(&mut self, ext: bool) {
        if ext {
            self.header |= 1 << 30;
        } else {
            self.header &= !(1 << 30);
        }
    }

    /// 是否冻结（`Object.freeze`，存在访问器/描述符时仍由属性级 configurable 约束）。
    pub fn is_frozen(&self) -> bool {
        (self._pad) & 1 != 0
    }

    /// 设置冻结标志。
    pub fn set_frozen(&mut self, frozen: bool) {
        if frozen {
            self._pad |= 1 << 0;
        } else {
            self._pad &= !(1 << 0);
        }
    }

    /// 是否密封（`Object.seal`）。
    pub fn is_sealed(&self) -> bool {
        (self._pad >> 1) & 1 != 0
    }

    /// 设置密封标志。
    pub fn set_sealed(&mut self, sealed: bool) {
        if sealed {
            self._pad |= 1 << 1;
        } else {
            self._pad &= !(1 << 1);
        }
    }

    /// 是否 Set 实例（header bit 24）。
    pub fn is_set(&self) -> bool {
        (self.header >> 24) & 1 != 0
    }

    /// 设置 Set 实例标志。
    pub fn set_set(&mut self, s: bool) {
        if s {
            self.header |= 1 << 24;
        } else {
            self.header &= !(1 << 24);
        }
    }

    /// 是否 Map 实例（header bit 25）。
    pub fn is_map(&self) -> bool {
        (self.header >> 25) & 1 != 0
    }

    /// 设置 Map 实例标志。
    pub fn set_map(&mut self, m: bool) {
        if m {
            self.header |= 1 << 25;
        } else {
            self.header &= !(1 << 25);
        }
    }

    /// 是否函数对象（header bit 31）。
    pub fn is_function(&self) -> bool {
        (self.header >> 31) & 1 != 0
    }

    /// 设置函数对象标志。
    pub fn set_function(&mut self, f: bool) {
        if f {
            self.header |= 1 << 31;
        } else {
            self.header &= !(1 << 31);
        }
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

    fn hash_props_vec_mut(&mut self) -> Option<&mut Vec<JsValue>> {
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

    fn array_elements_vec_mut(&mut self) -> Option<&mut Vec<JsValue>> {
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

    fn array_elements_meta_vec_mut(&mut self) -> Option<&mut Vec<Option<PropMetaEntry>>> {
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

    /// 当前 `[[Prototype]]` 值。
    pub fn proto(&self) -> JsValue {
        self.proto
    }

    /// 设置 `[[Prototype]]`，成功时递增 generation。
    ///
    /// 仅接受 `null` 或对象；沿新原型链检查是否构成环，成环返回
    /// `Err("cyclic __proto__ value")`。
    pub fn set_proto(&mut self, proto: JsValue) -> Result<(), &'static str> {
        if proto.is_null() {
            self.proto = proto;
            self.generation = self.generation.wrapping_add(1);
            return Ok(());
        }
        if !proto.is_object() {
            return Err("__proto__ must be an object or null");
        }
        let mut cursor = proto;
        let self_ptr = self as *const JsObject;
        while cursor.is_object() {
            let cursor_ptr = cursor.as_js_object_ptr();
            if std::ptr::eq(cursor_ptr, self_ptr) {
                return Err("cyclic __proto__ value");
            }
            debug_assert!(!cursor_ptr.is_null(), "prototype cursor pointer must not be null");
            // SAFETY: cursor 已知为对象 JsValue，编码的是合法 JsObject 指针。
            let obj = unsafe { &*cursor_ptr };
            cursor = obj.proto;
        }
        self.proto = proto;
        self.generation = self.generation.wrapping_add(1);
        Ok(())
    }

    /// 当前 generation（对象结构变更计数器，IC 失效依据之一）。
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// 递增 generation（`wrapping_add`）。
    pub fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// 关联的原生函数指针（若为内建/原生函数对象）。
    pub fn native_fn(&self) -> Option<NativeFnPtr> {
        self.native_fn
    }

    /// 设置 / 清除原生函数指针。
    pub fn set_native_fn(&mut self, ptr: Option<NativeFnPtr>) {
        self.native_fn = ptr;
    }

    /// 原生函数声明的参数个数。
    pub fn native_arg_count(&self) -> u8 {
        self.native_arg_count
    }

    /// 设置原生函数参数个数。
    pub fn set_native_arg_count(&mut self, n: u8) {
        self.native_arg_count = n;
    }

    /// 子模块下标（函数对象对应的 `CompiledModule` 索引）。
    pub fn sub_module_index(&self) -> u32 {
        self.sub_module_index
    }

    /// 设置子模块下标。
    pub fn set_sub_module_index(&mut self, idx: u32) {
        self.sub_module_index = idx;
    }

    /// 箭头函数标志（header bit 28）。
    /// 为 true 时，CALL 分发捕获创建时的词法 `this`。
    pub fn is_arrow(&self) -> bool {
        (self.header >> 28) & 1 != 0
    }

    /// 设置箭头函数标志（header bit 28）。
    pub fn set_arrow(&mut self, v: bool) {
        if v {
            self.header |= 1 << 28;
        } else {
            self.header &= !(1 << 28);
        }
    }

    /// 箭头函数创建时捕获的词法 `this`。
    /// 仅在 `is_arrow()` 返回 true 时有意义。
    pub fn captured_this(&self) -> JsValue {
        self.captured_this
    }

    /// 设置箭头函数捕获的词法 `this`。
    pub fn set_captured_this(&mut self, v: JsValue) {
        self.captured_this = v;
    }

    /// 类构造函数标志（header bit 27）。
    /// 普通 CALL 拒绝带此标志的对象；NEW_EXPRESSION 允许。
    pub fn is_class_constructor(&self) -> bool {
        (self.header >> 27) & 1 != 0
    }

    /// 设置类构造函数标志（header bit 27）。
    pub fn set_class_constructor(&mut self, v: bool) {
        if v {
            self.header |= 1 << 27;
        } else {
            self.header &= !(1 << 27);
        }
    }

    /// 是否派生类构造函数（有 `extends` 子句，header bit 26）。
    pub fn is_derived_constructor(&self) -> bool {
        (self.header >> 26) & 1 != 0
    }

    /// 设置派生类构造函数标志（header bit 26）。
    pub fn set_derived_constructor(&mut self, v: bool) {
        if v {
            self.header |= 1 << 26;
        } else {
            self.header &= !(1 << 26);
        }
    }

    /// 当前 `[[HomeObject]]`（供 `super` 属性访问解析）。
    pub fn home_object(&self) -> JsValue {
        self.home_object
    }

    /// 设置 `[[HomeObject]]`。
    pub fn set_home_object(&mut self, v: JsValue) {
        self.home_object = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::EMPTY_SHAPE_ID;

    #[test]
    fn object_size_bounds() {
        let sz = std::mem::size_of::<JsObject>();
        assert!(sz <= 256, "JsObject grew unexpectedly: {sz}B");
    }

    #[test]
    fn js_string_cons_basic() {
        // 左右子 Flat 构造 Cons：len/utf16_len/is_empty/flat 全部按拼接语义。
        let left = Box::new(JsString::new("ab".to_string()));
        let right = Box::new(JsString::new("cd".to_string()));
        let cons = Box::new(unsafe { JsString::new_cons(&*left, &*right) });
        assert_eq!(cons.len(), 4);
        assert!(!cons.is_empty());
        assert_eq!(cons.utf16_len(), 4);
        assert_eq!(cons.flat_str(), "abcd");
        assert_eq!(cons.as_str(), "abcd");
        assert_eq!(cons.to_owned_string(), "abcd");
        assert!(cons.is_cons());
        // 扁平化产物缓存命中：内容仍一致。
        assert_eq!(cons.flat_str(), "abcd");
    }

    #[test]
    fn js_string_cons_deep_chain_flattens_iteratively() {
        // 2000 层左倾链：显式栈展开不爆栈，产物与逐段拼接一致。
        let mut nodes: Vec<Box<JsString>> = Vec::new();
        nodes.push(Box::new(JsString::new("x".to_string())));
        for _ in 0..2000 {
            nodes.push(Box::new(JsString::new("y".to_string())));
        }
        let mut chain_ptr = nodes[0].as_ref() as *const JsString;
        for i in 1..nodes.len() {
            let cons = Box::new(unsafe { JsString::new_cons(chain_ptr, nodes[i].as_ref() as *const JsString) });
            chain_ptr = cons.as_ref() as *const JsString;
            nodes.push(cons);
        }
        let text = nodes.last().unwrap().flat_str();
        assert_eq!(text.len(), 1 + 2000);
        assert!(text.starts_with('x'));
        assert!(text.ends_with('y'));
    }

    #[test]
    fn js_string_cons_empty_parts() {
        // 双空串 Cons：空判定与长度均为 0，扁平化为空文本。
        let left = Box::new(JsString::new(String::new()));
        let right = Box::new(JsString::new(String::new()));
        let cons = Box::new(unsafe { JsString::new_cons(&*left, &*right) });
        assert!(cons.is_empty());
        assert_eq!(cons.len(), 0);
        assert_eq!(cons.flat_str(), "");
        assert_eq!(cons.utf16_len(), 0);
    }

    #[test]
    fn new_empty_defaults() {
        let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert_eq!(obj.shape_id(), EMPTY_SHAPE_ID);
        assert_eq!(obj.prop_count(), 0);
        assert!(obj.is_extensible());
        assert!(!obj.is_array());
        assert!(!obj.is_function());
        assert!(!obj.is_session_epoch());
        assert_eq!(obj.generation(), 1);
        assert!(obj.hash_props_vec().is_none());
        assert!(!obj.has_prop_meta());
    }

    #[test]
    fn session_epoch_marker_roundtrip() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert!(!obj.is_session_epoch());
        obj.set_session_epoch(true);
        assert!(obj.is_session_epoch());
        obj.set_session_epoch(false);
        assert!(!obj.is_session_epoch());
    }

    #[test]
    fn session_epoch_marker_preserves_gc_mark_bit() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert!(!obj.is_gc_marked());
        obj.set_gc_mark(true);
        assert!(obj.is_gc_marked());
        assert!(!obj.is_session_epoch());

        obj.set_session_epoch(true);
        assert!(obj.is_session_epoch());
        assert!(obj.is_gc_marked());

        obj.set_gc_mark(false);
        assert!(!obj.is_gc_marked());
    }

    #[test]
    fn session_epoch_marker_keeps_object_size_bound() {
        let sz = std::mem::size_of::<JsObject>();
        assert!(sz <= 256, "JsObject grew unexpectedly: {sz}B");
    }

    #[test]
    fn clone_for_session_epoch_marks_clone_and_does_not_alias_hash_props() {
        let mut source = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        source.set_prop_at(0, JsValue::int(1));

        let mut clone = source.clone_for_session_epoch();
        assert!(clone.is_session_epoch());
        clone.set_prop_at(0, JsValue::int(2));

        assert_eq!(source.get_prop_at(0), JsValue::int(1));
        assert_eq!(clone.get_prop_at(0), JsValue::int(2));
    }

    #[test]
    fn clone_for_session_epoch_does_not_alias_prop_meta() {
        let mut source = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        source.set_prop_at(0, JsValue::undefined());
        source.set_accessor_meta(0, JsValue::int(10), JsValue::int(11), PropAttributes::DEFAULT_DATA);

        let mut clone = source.clone_for_session_epoch();
        clone.set_accessor_meta(0, JsValue::int(20), JsValue::int(21), PropAttributes::DEFAULT_DATA);

        let source_meta = source.prop_meta_at(0).expect("source meta");
        let clone_meta = clone.prop_meta_at(0).expect("clone meta");
        assert_eq!(source_meta.get, JsValue::int(10));
        assert_eq!(source_meta.set, JsValue::int(11));
        assert_eq!(clone_meta.get, JsValue::int(20));
        assert_eq!(clone_meta.set, JsValue::int(21));
    }

    #[test]
    fn shape_id_roundtrip() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_shape_id(0x00AB_CDEF);
        assert_eq!(obj.shape_id(), 0x00AB_CDEF);
    }

    #[test]
    fn prop_count_roundtrip() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert_eq!(obj.prop_count(), 0);
        obj.ensure_hash_props().push(JsValue::int(17));
        assert_eq!(obj.prop_count(), 1);
    }

    #[test]
    fn flags_individual() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert!(obj.is_extensible());
        obj.set_extensible(false);
        assert!(!obj.is_extensible());
    }

    #[test]
    fn hash_prop_read_write() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(42));
        assert_eq!(obj.get_prop_at(0), JsValue::int(42));
    }

    #[test]
    fn new_array_flags() {
        let bump = bumpalo::Bump::new();
        let obj = JsObject::new_array(5, JsValue::null(), 3, &bump);
        assert!(obj.is_array());
        assert_eq!(obj.shape_id(), 5);
        assert_eq!(obj.prop_count(), 3);
    }

    #[test]
    fn generation_bump() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert_eq!(obj.generation(), 1);
        obj.bump_generation();
        assert_eq!(obj.generation(), 2);
    }

    #[test]
    fn hash_props_lazy_init() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert!(obj.hash_props_vec().is_none());
        assert_eq!(obj.prop_count(), 0);
        obj.set_prop_at(0, JsValue::int(1));
        assert!(obj.hash_props_vec().is_some());
        assert_eq!(obj.prop_count(), 1);
    }

    #[test]
    fn hash_props_flat_storage_roundtrip() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(100));
        obj.set_prop_at(1, JsValue::int(200));
        assert_eq!(obj.get_prop_at(0), JsValue::int(100));
        assert_eq!(obj.get_prop_at(1), JsValue::int(200));
    }

    #[test]
    fn prop_meta_lazy_init() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(1));
        assert!(!obj.has_prop_meta());

        obj.set_data_meta(0, PropAttributes::new(false, true, false));
        assert!(obj.has_prop_meta());
        let meta = obj.prop_meta_at(0).expect("meta");
        assert!(!meta.is_accessor);
        assert!(!meta.attributes.writable());
        assert!(meta.attributes.enumerable());
        assert!(!meta.attributes.configurable());
    }

    #[test]
    fn accessor_meta_roundtrip_and_alignment() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(1));
        obj.set_accessor_meta(2, JsValue::int(10), JsValue::int(11), PropAttributes::new(false, false, true));

        assert_eq!(obj.prop_count(), 3);
        assert!(obj.is_accessor_meta(2));
        let meta = obj.prop_meta_at(2).expect("accessor meta");
        assert_eq!(meta.get, JsValue::int(10));
        assert_eq!(meta.set, JsValue::int(11));
        assert!(!meta.attributes.writable());
        assert!(!meta.attributes.enumerable());
        assert!(meta.attributes.configurable());

        obj.push_prop(JsValue::int(4));
        assert_eq!(obj.prop_meta_vec().expect("meta").len(), obj.prop_vec_len());
    }

    #[test]
    fn array_element_write_preserves_props_after_element_growth() {
        // 先写属性再 push 元素：元素区增长必须整体搬移属性区，不覆盖属性。
        let bump = bumpalo::Bump::new();
        let mut obj = JsObject::new_array(EMPTY_SHAPE_ID, JsValue::null(), 3, &bump);
        obj.set_prop_shape(0, JsValue::int(99));
        obj.set_prop_at(3, JsValue::int(4));
        assert_eq!(obj.prop_count(), 4);
        assert_eq!(obj.get_prop_at(3), JsValue::int(4));
        assert_eq!(obj.get_prop_shape(0), JsValue::int(99));
    }

    #[test]
    fn array_element_write_beyond_count_relocates_prop_zone() {
        // 稀疏写入（越界索引）把属性区推到新元素之后，属性读取仍命中。
        let bump = bumpalo::Bump::new();
        let mut obj = JsObject::new_array(EMPTY_SHAPE_ID, JsValue::null(), 2, &bump);
        obj.set_prop_shape(0, JsValue::int(7));
        obj.set_prop_at(5, JsValue::int(50));
        assert_eq!(obj.prop_count(), 6);
        assert_eq!(obj.get_prop_at(5), JsValue::int(50));
        assert_eq!(obj.get_prop_shape(0), JsValue::int(7));
    }

    #[test]
    fn array_prop_count_truncate_keeps_prop_zone() {
        // pop 截断元素区时属性区不得被删（meta 同步 insert/drain 对齐）。
        let bump = bumpalo::Bump::new();
        let mut obj = JsObject::new_array(EMPTY_SHAPE_ID, JsValue::null(), 3, &bump);
        obj.set_prop_shape(0, JsValue::int(5));
        obj.set_data_meta(3, PropAttributes::new(true, false, true));
        obj.set_prop_count_fast(2);
        assert_eq!(obj.prop_count(), 2);
        assert_eq!(obj.get_prop_shape(0), JsValue::int(5));
        assert!(obj.prop_meta_at(2).is_some());
    }
}
