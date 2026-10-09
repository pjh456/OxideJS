//! 共享字节缓冲：SharedArrayBuffer 的跨线程共享存储。
//!
//! 关键约定：
//! - 字节区一次性预分配至真实上限并零填充，此后不再重分配；`grow` 只推进活长，O(1)。
//! - `write_range` 经 `&self` 裸写（字节区归本结构独占，类型不暴露 `&mut [u8]`），多线程并发写安全。
//! - 账目单位是预分配真实上限：构造时计入 `KernelCore::shared_buffer_bytes`，`Drop` 减同值。

use std::mem::size_of;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::KernelCore;

/// 共享字节缓冲操作的越界错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedBufferError {
    /// 写范围超出活长（offset + data 长度 > len）。
    OutOfRange,
}

/// 跨线程共享字节缓冲：预分配至真实上限，活长经原子推进。
///
/// 全部字段 `Send + Sync`，可经 `Arc` 跨线程共享；账目入口是结构持有的
/// `Arc<KernelCore>`，`Drop` 时从核的 `shared_buffer_bytes` 减预分配真实上限。
pub struct SharedBuffer {
    /// 共享字节区，预分配至真实上限并零填充。
    bytes: Arc<Vec<u8>>,
    /// 活字节长（growable 经原子推进）。
    length: AtomicUsize,
    /// 预分配真实上限。
    max_length: usize,
    /// 账目入口。
    core: Arc<KernelCore>,
}

// 九操作乘四宽度 SeqCst 原子方法生成宏。
//
// 对齐前提：`AtomicU{16,32,64}::from_ptr` 要求指针按宽度对齐。字节区经全局
// 分配器分配（实返 16 字节对齐块），偏移恒为元素字节数倍数（规范保证视图
// byte_offset 是元素字节数倍数、索引为整数），与既有元素读写路径（`read_element`
// / `write_element` 对 u8 切片做按对齐读）同一前提。
macro_rules! impl_atomic_width {
    ($atomic:ty, $ty:ty,
     $load:ident, $store:ident, $swap:ident, $cas:ident,
     $fadd:ident, $fsub:ident, $fand:ident, $for:ident, $fxor:ident) => {
        /// 读取 `offset` 处的位宽整数值（SeqCst 序）。
        pub fn $load(&self, offset: usize) -> $ty {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic load 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            // SAFETY: 字节区归本结构独占且类型不暴露 `&mut [u8]`，`from_ptr`
            // 不引入别名；对齐前提见宏注记。
            unsafe { <$atomic>::from_ptr(ptr).load(Ordering::SeqCst) }
        }

        /// 向 `offset` 处写入位宽整数值（SeqCst 序）。
        pub fn $store(&self, offset: usize, value: $ty) {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic store 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            unsafe { <$atomic>::from_ptr(ptr).store(value, Ordering::SeqCst) }
        }

        /// 交换 `offset` 处值：写入 `value` 并返回操作前旧值（SeqCst 序）。
        pub fn $swap(&self, offset: usize, value: $ty) -> $ty {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic swap 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            unsafe { <$atomic>::from_ptr(ptr).swap(value, Ordering::SeqCst) }
        }

        /// 比较交换：`offset` 处等于 `expected` 时写入 `replacement` 返
        /// `Ok(replacement)`，否则返 `Err(旧值)`（SeqCst 序）。
        pub fn $cas(&self, offset: usize, expected: $ty, replacement: $ty) -> Result<$ty, $ty> {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic compare_exchange 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            match unsafe {
                <$atomic>::from_ptr(ptr).compare_exchange(expected, replacement, Ordering::SeqCst, Ordering::SeqCst)
            } {
                Ok(_) => Ok(replacement),
                Err(actual) => Err(actual),
            }
        }

        /// 原子加：返回操作前旧值（SeqCst 序，环绕语义）。
        pub fn $fadd(&self, offset: usize, value: $ty) -> $ty {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic fetch_add 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            unsafe { <$atomic>::from_ptr(ptr).fetch_add(value, Ordering::SeqCst) }
        }

        /// 原子减：返回操作前旧值（SeqCst 序，环绕语义）。
        pub fn $fsub(&self, offset: usize, value: $ty) -> $ty {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic fetch_sub 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            unsafe { <$atomic>::from_ptr(ptr).fetch_sub(value, Ordering::SeqCst) }
        }

        /// 原子按位与：返回操作前旧值（SeqCst 序）。
        pub fn $fand(&self, offset: usize, value: $ty) -> $ty {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic fetch_and 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            unsafe { <$atomic>::from_ptr(ptr).fetch_and(value, Ordering::SeqCst) }
        }

        /// 原子按位或：返回操作前旧值（SeqCst 序）。
        pub fn $for(&self, offset: usize, value: $ty) -> $ty {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic fetch_or 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            unsafe { <$atomic>::from_ptr(ptr).fetch_or(value, Ordering::SeqCst) }
        }

        /// 原子按位异或：返回操作前旧值（SeqCst 序）。
        pub fn $fxor(&self, offset: usize, value: $ty) -> $ty {
            debug_assert!(offset + size_of::<$ty>() <= self.len(), "atomic fetch_xor 越界");
            let ptr = unsafe { self.bytes.as_ptr().add(offset) as *mut $ty };
            unsafe { <$atomic>::from_ptr(ptr).fetch_xor(value, Ordering::SeqCst) }
        }
    };
}

impl SharedBuffer {
    /// 定长构造：字节序列即活内容，上限即字节序列长度。
    ///
    /// # 副作用
    /// - 核的 `shared_buffer_bytes` 加字节序列长度（真实上限）。
    pub fn new(core: Arc<KernelCore>, bytes: Vec<u8>) -> Self {
        let bytes = Arc::new(bytes);
        let len = bytes.len();
        core.shared_buffer_bytes_add(len);
        Self {
            bytes,
            length: AtomicUsize::new(len),
            max_length: len,
            core,
        }
    }

    /// 可增长构造：预分配至 `max_len` 并零填充，初始活长 `initial_len`。
    ///
    /// # 边界与前提
    /// - `initial_len` 不得超过 `max_len`（debug 构建 fail-fast）。
    ///
    /// # 副作用
    /// - 核的 `shared_buffer_bytes` 加 `max_len`（真实上限），而非 `initial_len`。
    pub fn new_growable(core: Arc<KernelCore>, initial_len: usize, max_len: usize) -> Self {
        debug_assert!(initial_len <= max_len, "initial_len must not exceed max_len");
        let bytes = Arc::new(vec![0u8; max_len]);
        core.shared_buffer_bytes_add(max_len);
        Self {
            bytes,
            length: AtomicUsize::new(initial_len),
            max_length: max_len,
            core,
        }
    }

    /// 读取当前活字节长。
    pub fn len(&self) -> usize {
        self.length.load(Ordering::SeqCst)
    }

    /// 判断活区是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 读取预分配真实上限。
    pub fn max_len(&self) -> usize {
        self.max_length
    }

    /// 活长区只读切片。
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len()]
    }

    /// 把 `data` 写入活长区 `offset` 起（经 `&self` 裸写）。
    ///
    /// # 边界与前提
    /// - `offset + data.len()` 超过活长时返回 `OutOfRange`，不做任何写入；
    /// - 写入不改变活长。
    ///
    /// # 注意事项
    /// - 字节区归本结构独占且类型不暴露 `&mut [u8]`，经 `&self` 裸写不构成别名；
    ///   不得新增 `bytes_mut` 一类方法，否则该前提失效。
    pub fn write_range(&self, offset: usize, data: &[u8]) -> Result<(), SharedBufferError> {
        let end = offset.checked_add(data.len()).ok_or(SharedBufferError::OutOfRange)?;
        if end > self.len() {
            return Err(SharedBufferError::OutOfRange);
        }
        unsafe {
            // 字节区归本结构独占，裸写目标即其内部可变区。
            let dest = self.bytes.as_ptr().add(offset) as *mut u8;
            std::ptr::copy_nonoverlapping(data.as_ptr(), dest, data.len());
        }
        Ok(())
    }

    /// 把活长推进到 `new_len`（只增不缩），返回操作后活长。
    ///
    /// # 边界与前提
    /// - 缩长（`new_len` 不大于当前活长）与超上限（`new_len` 大于 `max_len`）均被拒绝，
    ///   返回当前活长；
    /// - 字节区已预分配，推进只改原子计数，O(1)。
    ///
    /// # 副作用
    /// - 经 CAS 循环推进 `length`；并发调用各自至多推进一次。
    pub fn grow(&self, new_len: usize) -> usize {
        let mut cur = self.length.load(Ordering::SeqCst);
        loop {
            // 缩长与超上限统一拒绝，返回当前活长。
            if new_len <= cur || new_len > self.max_length {
                return cur;
            }
            match self
                .length
                .compare_exchange_weak(cur, new_len, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return new_len,
                Err(next) => cur = next,
            }
        }
    }

    // 九操作乘四宽度（u8 / u16 / u32 / u64）SeqCst 原子方法，宏生成。
    impl_atomic_width!(
        AtomicU8,
        u8,
        load_u8,
        store_u8,
        swap_u8,
        compare_exchange_u8,
        fetch_add_u8,
        fetch_sub_u8,
        fetch_and_u8,
        fetch_or_u8,
        fetch_xor_u8
    );
    impl_atomic_width!(
        AtomicU16,
        u16,
        load_u16,
        store_u16,
        swap_u16,
        compare_exchange_u16,
        fetch_add_u16,
        fetch_sub_u16,
        fetch_and_u16,
        fetch_or_u16,
        fetch_xor_u16
    );
    impl_atomic_width!(
        AtomicU32,
        u32,
        load_u32,
        store_u32,
        swap_u32,
        compare_exchange_u32,
        fetch_add_u32,
        fetch_sub_u32,
        fetch_and_u32,
        fetch_or_u32,
        fetch_xor_u32
    );
    impl_atomic_width!(
        AtomicU64,
        u64,
        load_u64,
        store_u64,
        swap_u64,
        compare_exchange_u64,
        fetch_add_u64,
        fetch_sub_u64,
        fetch_and_u64,
        fetch_or_u64,
        fetch_xor_u64
    );
}

impl Drop for SharedBuffer {
    fn drop(&mut self) {
        // 账目单位与构造时加账一致：预分配真实上限。
        self.core.shared_buffer_bytes_sub(self.bytes.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KernelConfig;

    fn new_core() -> Arc<KernelCore> {
        KernelCore::new(KernelConfig::minimal())
    }

    #[test]
    fn shared_buffer_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SharedBuffer>();
    }

    #[test]
    fn new_fixed_size_len_and_accounting() {
        let core = new_core();
        let buf = SharedBuffer::new(core.clone(), vec![1, 2, 3, 4]);
        assert_eq!(buf.len(), 4);
        assert_eq!(buf.max_len(), 4);
        assert_eq!(buf.as_slice(), &[1, 2, 3, 4]);
        assert_eq!(core.shared_buffer_bytes(), 4);
    }

    #[test]
    fn new_growable_preallocates_to_max() {
        let core = new_core();
        let buf = SharedBuffer::new_growable(core.clone(), 2, 8);
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.max_len(), 8);
        assert_eq!(buf.as_slice(), &[0, 0]);
        // 账目单位是预分配真实上限，而非初始活长。
        assert_eq!(core.shared_buffer_bytes(), 8);
    }

    #[test]
    fn write_range_updates_live_region() {
        let core = new_core();
        let buf = SharedBuffer::new(core, vec![0u8; 8]);
        buf.write_range(2, &[9, 8, 7]).unwrap();
        assert_eq!(buf.as_slice(), &[0, 0, 9, 8, 7, 0, 0, 0]);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn write_range_rejects_out_of_range() {
        let core = new_core();
        let buf = SharedBuffer::new(core, vec![0u8; 4]);
        assert_eq!(buf.write_range(3, &[1, 2]), Err(SharedBufferError::OutOfRange));
        // offset 溢出加法同样按越界处理。
        assert_eq!(buf.write_range(usize::MAX, &[1]), Err(SharedBufferError::OutOfRange));
        // 拒绝时活长不变。
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn grow_only_increases_and_caps_at_max() {
        let core = new_core();
        let buf = SharedBuffer::new_growable(core, 2, 8);
        assert_eq!(buf.grow(5), 5);
        assert_eq!(buf.as_slice(), &[0, 0, 0, 0, 0]);
        // 缩长被拒绝，活长不变。
        assert_eq!(buf.grow(3), 5);
        // 超上限被拒绝，活长不变。
        assert_eq!(buf.grow(9), 5);
        // 恰好到上限成功。
        assert_eq!(buf.grow(8), 8);
    }

    #[test]
    fn drop_returns_accounting_to_zero() {
        let core = new_core();
        {
            let fixed = SharedBuffer::new(core.clone(), vec![0u8; 4]);
            // 作用域末隐式析构，验证减账归零。
            let _growable = SharedBuffer::new_growable(core.clone(), 0, 16);
            assert_eq!(core.shared_buffer_bytes(), 20);
            drop(fixed);
            assert_eq!(core.shared_buffer_bytes(), 16);
        }
        assert_eq!(core.shared_buffer_bytes(), 0);
    }

    #[test]
    fn arc_shared_across_threads() {
        let core = new_core();
        let buf = Arc::new(SharedBuffer::new_growable(core, 0, 16));
        let child = {
            let buf = Arc::clone(&buf);
            std::thread::spawn(move || {
                buf.grow(8);
                buf.write_range(4, &[0xAB, 0xCD]).unwrap();
                buf.len()
            })
        };
        let len = child.join().unwrap();
        assert_eq!(len, 8);
        assert_eq!(buf.as_slice(), &[0, 0, 0, 0, 0xAB, 0xCD, 0, 0]);
    }

    #[test]
    fn fetch_add_atomicity_8_threads() {
        let core = new_core();
        let buf = Arc::new(SharedBuffer::new(core, vec![0u8; 4]));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let buf = Arc::clone(&buf);
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        buf.fetch_add_u32(0, 1);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // 真原子性判别：8 线程各 1000 次自增，终值恰为 8000（读-改-写会丢更新）。
        assert_eq!(buf.load_u32(0), 8000);
    }

    #[test]
    fn compare_exchange_contention_exactly_one_wins() {
        let core = new_core();
        let buf = Arc::new(SharedBuffer::new(core, vec![0u8; 4]));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let buf = Arc::clone(&buf);
                std::thread::spawn(move || buf.compare_exchange_u32(0, 0, 1).is_ok())
            })
            .collect();
        let winners = handles.into_iter().map(|h| h.join().unwrap()).filter(|won| *won).count();
        assert_eq!(winners, 1);
        assert_eq!(buf.load_u32(0), 1);
    }

    #[test]
    fn load_store_cross_thread_visibility() {
        let core = new_core();
        let buf = Arc::new(SharedBuffer::new(core, vec![0u8; 8]));
        let child = {
            let buf = Arc::clone(&buf);
            std::thread::spawn(move || {
                buf.store_u64(0, 0x1122_3344_5566_7788);
            })
        };
        child.join().unwrap();
        assert_eq!(buf.load_u64(0), 0x1122_3344_5566_7788);
    }

    #[test]
    fn fetch_add_wraps_u8() {
        let core = new_core();
        let buf = SharedBuffer::new(core, vec![255u8; 1]);
        // 255 + 255 环绕：返回操作前旧值 255，槽值 254。
        let old = buf.fetch_add_u8(0, 255);
        assert_eq!(old, 255);
        assert_eq!(buf.load_u8(0), 254);
    }

    #[test]
    fn fetch_add_all_widths() {
        let core = new_core();
        let buf = SharedBuffer::new(core, vec![0u8; 16]);
        assert_eq!(buf.fetch_add_u8(0, 5), 0);
        assert_eq!(buf.fetch_add_u16(2, 7), 0);
        assert_eq!(buf.fetch_add_u32(4, 9), 0);
        assert_eq!(buf.fetch_add_u64(8, 11), 0);
        assert_eq!(buf.load_u8(0), 5);
        assert_eq!(buf.load_u16(2), 7);
        assert_eq!(buf.load_u32(4), 9);
        assert_eq!(buf.load_u64(8), 11);
    }
}
