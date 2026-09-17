//! 跨所有 VM 实例永久共享的不可变核心：持有 perm/shape/code/prop 四张 forge
//! Arc、VM 边界守卫计数（active_vms）、批边界 sweep 与 perm interner 建议
//! 重建信号。

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::code_forge::CodeForge;
use crate::kernel_info;
use crate::prop_forge::PropForge;
use crate::shape_forge::ShapeForge;
use crate::string_forge::PermInterner;
use oxide_log;

use super::KernelConfig;

/// 不可变、跨所有 VM 实例永久共享的状态。
/// 构造后从不原地重建——forge 表为 append-only；宿主可经
/// [`KernelCore::should_rebuild_perm`] 阈值在安全边界（无存活 VM）整体
/// 重建（换 `Arc`，非原地清表）。
pub struct KernelCore {
    pub config: KernelConfig,
    pub perm_interner: Arc<PermInterner>,
    pub shape_forge: Arc<ShapeForge>,
    pub code_forge: Arc<CodeForge>,
    pub prop_forge: Arc<PropForge>,
    /// 边界不变量守卫计数：当前持有本 kernel `Arc` 的 Vm 数。
    /// `Vm` 构造器增、`Drop for Vm` 减（恰好一次），供 sweep / kernel 重建 /
    /// kernel drop 处断言"无存活 VM"边界。
    active_vms: AtomicUsize,
}

impl KernelCore {
    /// 按配置创建共享核心：初始化日志系统与四个共享 forge（interner/shape/code/prop）。
    pub fn new(config: KernelConfig) -> Arc<Self> {
        oxide_log::init(&oxide_log::LogConfig {
            output: oxide_log::Output::Stderr,
            levels: config.log_levels,
        });
        let perm_interner = Arc::new(PermInterner::new());
        let shape_forge = Arc::new(ShapeForge::new());
        let code_forge = Arc::new(CodeForge::new(
            NonZeroUsize::new(config.max_cached_modules).expect("max_cached_modules must be greater than zero"),
        ));
        let prop_forge = Arc::new(PropForge::new());
        let max_cached = config.max_cached_modules;
        let min_pool = config.min_pool_size;
        let core = Arc::new(Self {
            config,
            perm_interner,
            shape_forge,
            code_forge,
            prop_forge,
            active_vms: AtomicUsize::new(0),
        });
        kernel_info!("KernelCore initialized: max_cached_modules={}, min_pool={}", max_cached, min_pool);
        core
    }

    /// 只读访问永久字符串 intern 表。
    pub fn perm_interner(&self) -> &Arc<PermInterner> {
        &self.perm_interner
    }

    /// 只读访问共享 hidden class（shape）存储。
    pub fn shape_forge(&self) -> &Arc<ShapeForge> {
        &self.shape_forge
    }

    /// 只读访问共享 bytecode cache。
    pub fn code_forge(&self) -> &Arc<CodeForge> {
        &self.code_forge
    }

    /// 只读访问共享属性模板缓存。
    pub fn prop_forge(&self) -> &Arc<PropForge> {
        &self.prop_forge
    }

    /// 只读访问构建时固定的配置。
    pub fn config(&self) -> &KernelConfig {
        &self.config
    }

    /// 读取 session GC 阈值（字节数）。
    pub fn session_gc_threshold(&self) -> usize {
        self.config.session_gc_threshold
    }

    /// 设置 session GC 阈值（字节数）。
    pub fn set_session_gc_threshold(&mut self, bytes: usize) {
        self.config.session_gc_threshold = bytes;
    }

    /// 读取 code cache 的 module 数量上限。
    pub fn max_cached_modules(&self) -> usize {
        self.config.max_cached_modules
    }

    /// 设置 code cache 的 module 数量上限。
    pub fn set_max_cached_modules(&mut self, cap: usize) {
        self.config.max_cached_modules = cap;
    }

    /// 批边界清理瞬时 shape/prop 缓存，防止跨测试累积膨胀。
    ///
    /// 瞬时 forge（非根 shape/transition/position + prop 模板）的 id 空间只在两类
    /// 边界复位：（1）kernel 整体重建——新核构造即空（结构性必清；宿主义务 =
    /// 重建前旧核无存活 VM，否则旧核连同其 forge 永久驻留）；（2）批内兜底 sweep
    /// ——本函数，数据依赖：仅当 shape 或 prop 表超过 50k 阈值时才执行
    /// [`ShapeForge::clear_transient`] 与 [`PropForge::clear`]（阈值是增长闸门，
    /// 宿主的检查节奏只是采样点）。字符串 intern 表 append-only、CodeForge LRU
    /// 自管理，均不碰。
    ///
    /// # 边界与前提
    /// - 调用点须无存活 VM：对象头与 IC 词持 shape id，清空使 id 空间复位，跨复位
    ///   存活的 VM 会因 id 复用碰撞静默错槽；该前提由 debug_assert 守卫（开发期
    ///   fail-fast），release 构建无断言，宿主可经 [`Self::active_vms`] 自检；
    /// - 引擎不自动重建：id 空间的另一类复位（kernel 整体重建）由宿主在
    ///   "无存活 VM" 边界驱动（runner 由循环结构满足：VM 每测试新建即弃，
    ///   重建/sweep 点都在 VM 作用域外）。
    pub fn sweep_runner_forges(&self) {
        // id 空间复位在有存活 VM 时是静默错槽隐患，守卫先于阈值判断。
        debug_assert!(
            self.active_vms.load(Ordering::Relaxed) == 0,
            "sweep_runner_forges requires no live VMs (shape id space reset)"
        );
        // 键 interner 是 append-only（无逐次清理）；仅当瞬时 shape/prop 表超阈值
        // 时清表（批内兜底）。
        if self.shape_forge.len() > 50_000 {
            self.shape_forge.clear_transient();
            self.prop_forge.clear();
        } else if self.prop_forge.len() > 50_000 {
            self.prop_forge.clear();
        }
    }

    /// 判断宿主是否应整体重建本 kernel，并返回重建后建议写入新配置的阈值。
    ///
    /// perm interner 唯一键数 `entry_count` **超过**配置的
    /// [`KernelConfig::perm_interner_max_entries`] 阈值时返回
    /// `Some(建议上限)`——建议上限 = 阈值取 2 的幂后加倍，宿主应在整体重建
    /// 后把它写入新 kernel 的 `perm_interner_max_entries`（增长不立即再次
    /// 触顶）；阈值未设或键数未超阈值时返回 `None`。
    ///
    /// # 边界与前提
    /// - 仅在宿主边界（run 间 / 测试间 / 迭代间）调用，勿入 dispatch 热路径：
    ///   `entry_count` 为一次读锁的 O(1) 查询，intern 路径零加码；
    /// - 仅具备安全重建边界的宿主（test262 runner 批边界、嵌入宿主迭代
    ///   之间）应启用旋钮；REPL 类宿主（单持久 VM，重建 = 丢失顶层状态）
    ///   不应启用。
    ///
    /// # 注意事项
    /// - 纯建议、只读查询：引擎不自动重建。重建须由宿主在"无存活 VM"边界
    ///   驱动：归还全部 `VmGuard`（池排空）→ 旧 `Arc<KernelCore>` 归零整体
    ///   释放（PermInterner / ShapeForge / CodeForge / PropForge，含全部键
    ///   文本与物化串）→ 新建 kernel 与新池 / VM。存活 VM 持有的模块代际表
    ///   注册表与 P 对象引用旧 kernel 的物化串裸指针与 shape id，VM 存活
    ///   期间重建会使它们悬垂。
    /// - 三个预设默认 `None`（永不触发）：现有 CLI eval/run/REPL/bench/test262
    ///   宿主均未接重建边界，`None` 保证零行为漂移。
    ///
    /// # 副作用
    /// 无：只读查询。
    pub fn should_rebuild_perm(&self) -> Option<u32> {
        let cap = self.config.perm_interner_max_entries?;
        (self.perm_interner.entry_count() > cap).then_some(cap.next_power_of_two().saturating_mul(2))
    }

    /// 边界不变量守卫计数：登记一个 Vm 出生（`Vm` 两条构造路径完整构造后调用）。
    pub fn note_vm_started(&self) {
        self.active_vms.fetch_add(1, Ordering::Relaxed);
    }

    /// 边界不变量守卫计数：注销一个 Vm 死亡（`Drop for Vm` 恰好调用一次）。
    pub fn note_vm_ended(&self) {
        self.active_vms.fetch_sub(1, Ordering::Relaxed);
    }

    /// 读取当前持有本 kernel 的存活 Vm 数；release 宿主可在 id 空间复位边界自检。
    pub fn active_vms(&self) -> usize {
        self.active_vms.load(Ordering::Relaxed)
    }
}

impl Drop for KernelCore {
    fn drop(&mut self) {
        // kernel 能 drop 说明持 Arc 的 Vm 已全部归零；计数非零 = 纯计数漂移
        // （note_vm_started/note_vm_ended 配对挂接回归探测）。
        debug_assert_eq!(
            self.active_vms.load(Ordering::Relaxed),
            0,
            "KernelCore dropped with live VMs (note_vm_ended counter drift)"
        );
    }
}
