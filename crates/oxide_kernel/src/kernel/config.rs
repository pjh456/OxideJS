//! kernel 运行配置：VM 池规模、步数/调用深度/单 run 分配上限、session GC 阈值、
//! perm interner 重建阈值、日志级别与内置对象预热开关（三预设 minimal/standard/full）。

use oxide_log::{Level, SUBSYSTEM_COUNT};

/// kernel 运行配置：VM 池规模、步数/调用深度/单 run 分配上限、session GC 阈值、
/// perm interner 重建阈值、日志级别与内置对象预热开关。由三个预设构造器
/// （[`KernelConfig::minimal`] / [`KernelConfig::standard`] / [`KernelConfig::full`]）
/// 或默认值创建。
#[derive(Clone)]
pub struct KernelConfig {
    pub min_pool_size: usize,
    pub max_pool_size: Option<usize>,
    /// perm interner advisory 重建阈值：唯一键数 `entry_count` **超过**该值时，
    /// 宿主应在安全边界（无存活 VM）整体重建 kernel，并把
    /// [`KernelCore::should_rebuild_perm`] 返回的建议上限写入新 kernel 配置；
    /// None = 无阈值（三个预设默认值，行为与无旋钮一致）。
    /// 见 [`KernelCore::should_rebuild_perm`]。
    pub perm_interner_max_entries: Option<u32>,
    pub max_steps: Option<u64>,
    /// 单次 run 的分配上限（epoch + session arena + session 堆账目字节）：
    /// 超限的 run 以 `VM memory limit exceeded` 失败，防单测试 arena 高水位
    /// 拖垮宿主内存。None 为无上限（CLI/嵌入默认；runner 按场景设置）。
    pub max_alloc_bytes: Option<usize>,
    pub max_call_depth: usize,
    pub session_gc_threshold: usize,
    pub max_cached_modules: usize,
    pub log_levels: [Level; SUBSYSTEM_COUNT],
    pub warmup_builtin_shapes: bool,
    pub warmup_builtin_code: bool,
    pub warmup_builtin_ic: bool,
}

impl KernelConfig {
    /// 最小配置：小 VM 池、关闭 code/IC 预热，适合嵌入式或单次执行场景。
    pub fn minimal() -> Self {
        Self {
            min_pool_size: 4,
            max_pool_size: Some(8),
            perm_interner_max_entries: None,
            max_steps: None,
            max_alloc_bytes: None,
            max_call_depth: 1024,
            session_gc_threshold: 33_554_432,
            max_cached_modules: 512,
            log_levels: [Level::Off; SUBSYSTEM_COUNT],
            warmup_builtin_shapes: true,
            warmup_builtin_code: false,
            warmup_builtin_ic: false,
        }
    }

    /// 标准配置：默认 VM 池大小，开启内置对象 code 预热。
    pub fn standard() -> Self {
        Self {
            min_pool_size: 8,
            max_pool_size: Some(32),
            perm_interner_max_entries: None,
            max_steps: None,
            max_alloc_bytes: None,
            max_call_depth: 1024,
            session_gc_threshold: 33_554_432,
            max_cached_modules: 512,
            log_levels: [Level::Off; SUBSYSTEM_COUNT],
            warmup_builtin_shapes: true,
            warmup_builtin_code: true,
            warmup_builtin_ic: false,
        }
    }

    /// 全量配置：无上限 VM 池，开启 shapes/code/IC 全量预热，为性能场景服务。
    pub fn full() -> Self {
        Self {
            min_pool_size: 16,
            max_pool_size: None,
            perm_interner_max_entries: None,
            max_steps: None,
            max_alloc_bytes: None,
            max_call_depth: 1024,
            session_gc_threshold: 33_554_432,
            max_cached_modules: 512,
            log_levels: [Level::Off; SUBSYSTEM_COUNT],
            warmup_builtin_shapes: true,
            warmup_builtin_code: true,
            warmup_builtin_ic: true,
        }
    }

    /// 读取 session GC 阈值（字节数）。
    pub fn session_gc_threshold(&self) -> usize {
        self.session_gc_threshold
    }

    /// 设置 session GC 阈值（字节数），超过后触发一次 session 级 GC。
    pub fn set_session_gc_threshold(&mut self, bytes: usize) {
        self.session_gc_threshold = bytes;
    }

    /// 读取 code cache 的 module 数量上限。
    pub fn max_cached_modules(&self) -> usize {
        self.max_cached_modules
    }

    /// 设置 code cache 的 module 数量上限。
    pub fn set_max_cached_modules(&mut self, cap: usize) {
        self.max_cached_modules = cap;
    }
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self::minimal()
    }
}
