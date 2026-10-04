//! `--profile` 旗标的聚合计数器快照：只读聚合现有计数器（GC 统计、内联缓存
//! 命中/未命中、session 堆账目、指令数），按每指标一行输出。
//!
//! 零热路径写：`collect` 只从 VM 内省面读 getter，不写任何计数器；旗标默认
//! 关闭，关闭时不进入采集路径。

use oxide_vm::vm::Vm;

/// 单次 run 的聚合计数器快照（现有计数器的只读聚合）。
#[derive(Debug, Clone)]
pub struct ProfileOutput {
    // GC 统计（session 累计口径，12 字段）
    pub gc_total_collections: u64,
    pub gc_total_bytes_freed: u64,
    pub gc_total_objects_scanned: u64,
    pub gc_total_objects_live: u64,
    pub gc_total_objects_dead: u64,
    pub gc_last_objects_scanned: u64,
    pub gc_last_objects_live: u64,
    pub gc_last_objects_dead: u64,
    pub gc_last_bytes_freed: u64,
    pub gc_last_duration_us: u64,
    pub gc_max_duration_us: u64,
    pub gc_min_duration_us: u64,
    // 内联缓存命中/未命中
    pub ic_hits: u64,
    pub ic_misses: u64,
    pub ic_hit_rate: f64,
    // session 堆账目
    pub session_objects: usize,
    pub session_bytes: usize,
    pub session_bytes_peak: usize,
    pub run_alloc_peak: usize,
    pub run_alloc_bytes: usize,
    // 指令数与执行时长
    pub instruction_count: u64,
    pub exec_time_us: u64,
}

impl ProfileOutput {
    /// 从 VM 内省面只读聚合（零热路径写，不写任何计数器）。
    ///
    /// `exec_time_us` 由调用方在 `run` 前后计时后传入。
    pub fn collect(vm: &Vm, exec_time_us: u64) -> Self {
        let gc = vm.session_gc_stats();
        Self {
            gc_total_collections: gc.total_collections,
            gc_total_bytes_freed: gc.total_bytes_freed,
            gc_total_objects_scanned: gc.total_objects_scanned,
            gc_total_objects_live: gc.total_objects_live,
            gc_total_objects_dead: gc.total_objects_dead,
            gc_last_objects_scanned: gc.last_collection_objects_scanned,
            gc_last_objects_live: gc.last_collection_objects_live,
            gc_last_objects_dead: gc.last_collection_objects_dead,
            gc_last_bytes_freed: gc.last_collection_bytes_freed,
            gc_last_duration_us: gc.last_collection_duration_us,
            gc_max_duration_us: gc.max_collection_duration_us,
            // 无收集时 min 字段为 u64::MAX 哨兵，映射为 0 避免输出哨兵值。
            gc_min_duration_us: if gc.min_collection_duration_us == u64::MAX {
                0
            } else {
                gc.min_collection_duration_us
            },
            ic_hits: vm.ic_hit_count(),
            ic_misses: vm.ic_miss_count(),
            ic_hit_rate: vm.ic_hit_rate(),
            session_objects: vm.session_object_count(),
            session_bytes: vm.session_bytes_allocated(),
            session_bytes_peak: vm.session_bytes_peak(),
            run_alloc_peak: vm.run_alloc_peak(),
            run_alloc_bytes: vm.run_alloc_bytes_total(),
            instruction_count: vm.instruction_count(),
            exec_time_us,
        }
    }

    /// 导出为 `(指标名, 值)` 列表，每指标一行，与 bench 输出对齐。
    pub fn iter_metrics(&self) -> Vec<(&'static str, String)> {
        vec![
            ("instruction_count", self.instruction_count.to_string()),
            ("session_objects", self.session_objects.to_string()),
            ("session_bytes", self.session_bytes.to_string()),
            ("session_bytes_peak", self.session_bytes_peak.to_string()),
            ("run_alloc_peak", self.run_alloc_peak.to_string()),
            ("run_alloc_bytes", self.run_alloc_bytes.to_string()),
            ("ic_hit_rate", format!("{:.4}", self.ic_hit_rate)),
            ("ic_hits", self.ic_hits.to_string()),
            ("ic_misses", self.ic_misses.to_string()),
            ("gc_total_collections", self.gc_total_collections.to_string()),
            ("gc_total_bytes_freed", self.gc_total_bytes_freed.to_string()),
            ("gc_total_objects_scanned", self.gc_total_objects_scanned.to_string()),
            ("gc_total_objects_live", self.gc_total_objects_live.to_string()),
            ("gc_total_objects_dead", self.gc_total_objects_dead.to_string()),
            ("gc_last_objects_scanned", self.gc_last_objects_scanned.to_string()),
            ("gc_last_objects_live", self.gc_last_objects_live.to_string()),
            ("gc_last_objects_dead", self.gc_last_objects_dead.to_string()),
            ("gc_last_bytes_freed", self.gc_last_bytes_freed.to_string()),
            ("gc_last_duration_us", self.gc_last_duration_us.to_string()),
            ("gc_max_duration_us", self.gc_max_duration_us.to_string()),
            ("gc_min_duration_us", self.gc_min_duration_us.to_string()),
            ("exec_time_us", self.exec_time_us.to_string()),
        ]
    }
}
