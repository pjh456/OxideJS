//! 共享字节缓冲：SharedArrayBuffer 的跨线程共享存储。
//!
//! 关键约定：
//! - 字节区一次性预分配至真实上限并零填充，此后不再重分配；`grow` 只推进活长，O(1)。
//! - `write_range` 经 `&self` 裸写（字节区归本结构独占，类型不暴露 `&mut [u8]`），多线程并发写安全。
//! - 账目单位是预分配真实上限：构造时计入 `KernelCore::shared_buffer_bytes`，`Drop` 减同值。

use std::sync::atomic::{AtomicUsize, Ordering};
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
}
