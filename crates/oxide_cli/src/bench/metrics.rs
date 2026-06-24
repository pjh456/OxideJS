use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricCollection {
    pub test_name: String,
    pub wall_time_us: u64,
    pub session_objects: usize,
    pub session_bytes: usize,
    pub epoch_objects: usize,
    pub epoch_bytes: u64,
    pub gc_trigger_count: u64,
    pub gc_bytes_freed: u64,
    pub gc_objects_scanned: u64,
    pub gc_collection_us: u64,
    pub instruction_count: u64,
    pub compile_time_us: u64,
    pub exec_time_us: u64,
    pub ic_hit_rate: f64,
    pub ic_hits: u64,
    pub ic_misses: u64,
}

impl MetricCollection {
    pub fn iter_metrics(&self) -> Vec<(&'static str, f64)> {
        vec![
            ("wall_time_us", self.wall_time_us as f64),
            ("session_objects", self.session_objects as f64),
            ("session_bytes", self.session_bytes as f64),
            ("epoch_objects", self.epoch_objects as f64),
            ("epoch_bytes", self.epoch_bytes as f64),
            ("gc_trigger_count", self.gc_trigger_count as f64),
            ("gc_bytes_freed", self.gc_bytes_freed as f64),
            ("gc_objects_scanned", self.gc_objects_scanned as f64),
            ("gc_collection_us", self.gc_collection_us as f64),
            ("instruction_count", self.instruction_count as f64),
            ("compile_time_us", self.compile_time_us as f64),
            ("exec_time_us", self.exec_time_us as f64),
            ("ic_hit_rate", self.ic_hit_rate),
            ("ic_hits", self.ic_hits as f64),
            ("ic_misses", self.ic_misses as f64),
        ]
    }
}
