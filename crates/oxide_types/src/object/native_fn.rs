//! 原生函数指针的类型安全不透明包装。
//!
//! 以 `*const ()` 存储而非具体 `fn` 类型，使本 crate 无需依赖 `oxide_vm`；
//! 指针必须由合法的 `NativeFn` 函数项创建且永不为空，`Send + Sync` 依赖
//! 函数项指针天然线程安全。

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
