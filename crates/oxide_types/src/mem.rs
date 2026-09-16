//! 内存管理抽象：跨 epoch 持久存储与单次 run 内的 arena 分配。
//!
//! `P<T>` 是 `Arc` 的透明包装，持有者跨 `Epoch::reset()` 存活；
//! `Epoch` 则是对 `bumpalo::Bump` 的封装，用于单次 run 内的高频分配，
//! `reset()` 换新 arena 整体回收（旧 arena 全量归还系统分配器，地址空间
//! 不复用），并通过 epoch ID 辅助悬挂指针检测。

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// 引用计数持久指针。
///
/// `P<T>` 是 `Arc<T>` 的透明包装（`repr(transparent)`），语义等价于 `Arc`：
/// `Clone` 递增引用计数，`Drop` 递减。对象位于全局堆上，跨 `Epoch::reset()`
/// 存活。
#[repr(transparent)]
pub struct P<T>(Arc<T>);

impl<T> P<T> {
    /// 在堆上分配并包装 `value`。
    pub fn new(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// 返回内部 `Arc<T>` 的底层裸指针。
    pub fn as_ptr(&self) -> *const T {
        Arc::as_ptr(&self.0)
    }

    /// 返回可变裸指针（用于外部 GC / 遍历改写）。
    #[inline(always)]
    pub fn as_mut_ptr(&self) -> *mut T {
        self.as_ptr() as *mut T
    }

    /// 当前强引用数。session 收尾路径据此判断该 `P` 对象是否与其他
    /// 持有方（如 `BuiltinWorld`）共享，避免对仍被其他引用方使用的
    /// 对象做归属误判。
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }
}

impl<T> Clone for P<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Deref for P<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for P<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P({:?})", self.0)
    }
}

impl<T: fmt::Display> fmt::Display for P<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 包装 `bumpalo::Bump` 并携带 epoch ID 计数器用于悬挂指针检测。
/// 所有 run 级对象分配于此；`reset()` 在每次 run 结束时换新 arena，
/// 旧 arena 内存全量归还系统分配器。
pub struct Epoch {
    bump: bumpalo::Bump,
    epoch_id: u64,
}

impl Epoch {
    /// 创建从 epoch 0 开始、空 arena 的 run 级分配器。
    pub fn new() -> Self {
        Self {
            bump: bumpalo::Bump::new(),
            epoch_id: 0,
        }
    }

    /// 在 arena 中 bump 分配一个值并返回裸指针。
    ///
    /// # Safety
    ///
    /// 返回的指针在本 epoch 的 `reset()` 调用之前有效。调用方不得在
    /// 该边界之后继续持有，并须确保 arena 重置后不再使用任何别名。
    pub fn alloc<T>(&self, value: T) -> *mut T {
        self.bump.alloc(value)
    }

    /// 直接访问底层 `bumpalo::Bump`。
    ///
    /// 供需要依赖 bumpalo API（如 `alloc_slice`）的调用方使用。
    pub fn bump(&self) -> &bumpalo::Bump {
        &self.bump
    }

    /// 用初始化闭包在 arena 上直接构造值。
    /// 对编译器优化更友好——直接在 arena 上构造。
    pub fn alloc_with<T, F>(&self, f: F) -> *mut T
    where
        F: FnOnce() -> T,
    {
        self.bump.alloc_with(f)
    }

    /// 当 `ptr` 位于当前已分配 bump chunk 之一时返回 true。
    ///
    /// 仅比较地址，不解引用 `ptr`。
    #[inline]
    pub fn is_epoch_ptr(&self, ptr: *const u8) -> bool {
        let addr = ptr as usize;
        if addr == 0 {
            return false;
        }
        // SAFETY: 本辅助函数在遍历原始 chunk 迭代器时不执行任何分配，
        // 因此 bumpalo 的 chunk 列表在迭代期间不会变化。
        unsafe {
            self.bump.iter_allocated_chunks_raw().any(|(base, len)| {
                let start = base as usize;
                let end = start.saturating_add(len);
                addr >= start && addr < end
            })
        }
    }

    /// 整体回收 arena：换新空 `Bump`，旧 arena 的全部 chunk 归还系统分配器。
    ///
    /// # 副作用
    /// - 此前的全部分配立即失效；地址空间不复用（新 arena 不占用旧地址）。
    /// - epoch ID 加 1：旧 arena 对象中存储的 epoch 值此后与当前值不一致，
    ///   解引用时可据此检出过期指针（仅 debug 断言）。
    ///
    /// # 边界与前提
    /// - 调用点须保证此刻无指向旧 arena 的存活指针（登记表已清、执行态已空）。
    ///   违反时换 arena 后旧指针要么解引用到同地址新对象（静默错值），要么成为
    ///   use-after-free；换 Bump 正是把这类漏网指针从静默错值转为可检出的
    ///   use-after-free，属护栏加强。
    pub fn reset(&mut self) {
        self.bump = bumpalo::Bump::new();
        self.epoch_id += 1;
    }

    /// 当前 epoch ID。arena 分配的对象存储此值；
    /// 解引用时校验与当前 ID 一致（仅 debug_assert）。
    pub fn current_id(&self) -> u64 {
        self.epoch_id
    }
}

impl Default for Epoch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
