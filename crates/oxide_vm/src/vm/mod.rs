#![allow(clippy::arc_with_non_send_sync)]

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
#[cfg(test)]
use std::sync::OnceLock;

#[cfg(test)]
use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode;
use smallvec::SmallVec;

/// 初始化一个 session 的内置对象（global 槽位、各构造器与原型、IC 预热）。
pub use crate::bindings::init_kernel_builtins;
use crate::native::NativeFn;
use crate::vm_debug;
use crate::vm_state::{GcState, IterState, ProfilingState, SymbolState};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::error::JsErrorKind;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{Cell, JsObject, NativeFnPtr};
use oxide_types::private_key::INT_KEY_COUNT;
use oxide_types::value::JsValue;

mod call;
mod dispatch_loop;
mod dynamic;
mod error;
mod frames;
mod gc_hooks;
mod inline;
mod props;
mod tables;
mod vmhost;

pub(crate) use frames::FrameArgs;
pub use frames::{CallFrame, Completion, ForInIter, FrameContinuation, TryHandler};
pub(crate) use inline::InlineSyncState;
pub(crate) use tables::TableGen;

pub(crate) const MAX_PROTO_CHAIN_DEPTH: usize = 1024;

/// 判定字符串是否为规范数组下标（无前导零的纯数字串），并反解其值。
///
/// 命中则把该字符串键与对应整数键合并为同一键（`obj["5"]` == `obj[5]`）。
/// 只覆盖 `[0, INT_KEY_COUNT)`，更大的数字串（含 2^32 边界）走普通字符串键。
fn canonical_index_of(s: &str) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let b = s.as_bytes();
    if !b[0].is_ascii_digit() {
        return None;
    }
    if b.len() > 1 && b[0] == b'0' {
        return None;
    }
    if !b.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: u32 = s.parse().ok()?;
    if v < INT_KEY_COUNT {
        Some(v)
    } else {
        None
    }
}

/// 从单元序列判定字符串是否为规范数组下标（无前导零的纯 ASCII 数字串），
/// 并反解其值。口径与 [`canonical_index_of`] 一致，单元口径覆盖非 Flat
/// 载荷的键推导。只覆盖 `[0, INT_KEY_COUNT)`，更大的数字串走普通字符串键。
fn canonical_index_units(units: &[u16]) -> Option<u32> {
    if units.is_empty() {
        return None;
    }
    if !matches!(units[0], 0x30..=0x39) {
        return None;
    }
    if units.len() > 1 && units[0] == 0x30 {
        return None;
    }
    if units.len() > 10 {
        return None;
    }
    let mut v: u32 = 0;
    for &u in units.iter() {
        let d = match u {
            0x30..=0x39 => u - 0x30,
            _ => return None,
        };
        // 两位都须 overflow 检查：`checked_mul(10) + d` 的裸加在 10 位串
        // （如 "4294967296"）会 mod 2^32 回绕出假规范下标。
        v = v.checked_mul(10)?.checked_add(d as u32)?;
    }
    (v < INT_KEY_COUNT).then_some(v)
}

/// 将 [`NativeFnPtr`] 转换为可调用的 [`NativeFn`]。
///
/// # Safety
/// `ptr` 必须由合法的 `NativeFn` 函数项产生。
/// 这是代码库中唯一一处 `NativeFnPtr → NativeFn` 的强制转换点。
#[inline(always)]
pub(crate) unsafe fn native_fn_ptr_to_fn(ptr: NativeFnPtr) -> NativeFn {
    std::mem::transmute::<*const (), NativeFn>(ptr.as_ptr())
}

fn js_error_kind(kind: &'static str) -> JsErrorKind {
    match kind {
        "TypeError" => JsErrorKind::TypeError,
        "RangeError" => JsErrorKind::RangeError,
        "ReferenceError" => JsErrorKind::ReferenceError,
        "SyntaxError" => JsErrorKind::SyntaxError,
        "URIError" => JsErrorKind::URIError,
        "EvalError" => JsErrorKind::EvalError,
        _ => JsErrorKind::Error,
    }
}

fn js_error_kind_name(kind: JsErrorKind) -> &'static str {
    match kind {
        JsErrorKind::TypeError => "TypeError",
        JsErrorKind::RangeError => "RangeError",
        JsErrorKind::ReferenceError => "ReferenceError",
        JsErrorKind::SyntaxError => "SyntaxError",
        JsErrorKind::Error => "Error",
        JsErrorKind::URIError => "URIError",
        JsErrorKind::EvalError => "EvalError",
    }
}

pub(crate) fn format_error_message(name: &str, msg: &str) -> String {
    if name.is_empty() {
        msg.to_string()
    } else if msg.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {msg}")
    }
}

/// 基于寄存器的 JS 虚拟机：持有执行状态、寄存器文件、调用栈与 session 内存。
///
/// 执行入口为 [`Vm::run`]（见 `vm_runtime` 模块）；内存模型为 epoch arena +
/// session 对象（可被 `SessionGc` 移动式回收）+ session 字符串。多数内部字段为
/// `pub(crate)`，对外提供统计与内省 getter。
pub struct Vm {
    pub(crate) regs: [JsValue; 256],
    pub(crate) pc: usize,
    /// 当前活动字节码。以 `Arc<[Instr]>` 共享：函数调用经 `Arc::clone` 换帧（O(1)），
    /// 不再逐帧深拷贝。与子模块平表源共享同一缓冲，IC 写回经 `bytecode_mut` 的
    /// `Arc::make_mut` 写时复制，保证独占后才改写（miss 时才深拷贝，频率低）。
    pub(crate) bytecode: Arc<[opcode::Instr]>,
    /// 当前活动模块已转换不可变常量的只读视图（指向当前表代际的 immutables 内部）。
    /// 用胖 `*const`：常量 Vec 归表代际所有且 OnceLock 只填一次（堆分配地址稳定）。
    pub(crate) active_immutables: *const [JsValue],
    pub(crate) frames: SmallVec<[CallFrame; 16]>,
    pub(crate) kernel_core: Arc<KernelCore>,
    pub(crate) session: KernelSession,
    /// `"length"` 属性键的 intern id 缓存：进程内稳定（PermInterner append-only、
    /// KernelCore 不重建），属性 get/set 热路径免每次 intern（hash64 + DashMap +
    /// RwLock 读锁）。
    pub(crate) length_si: u32,
    pub epoch: Epoch,
    pub object_prototype: P<JsObject>,
    /// `%GeneratorPrototype%`：生成器实例的原型（next/return/throw 方法挂此）。
    pub generator_proto: P<JsObject>,
    /// `%GeneratorFunction.prototype%`：生成器函数对象的原型（`constructor` 指向
    /// `%GeneratorFunction%`，使 `g.constructor.name` 解析为 "GeneratorFunction"）。
    pub generator_function_proto: P<JsObject>,
    /// `%Promise%` 构造器（resolve/reject 静态方法挂此，global 的 Promise 槽指向它）。
    pub promise_constructor: P<JsObject>,
    /// `%Promise.prototype%`：Promise 实例的原型（then/catch/finally 方法挂此）。
    pub promise_proto: P<JsObject>,
    /// `%AggregateError%` 构造器（Promise.any 拒绝时构造 AggregateError 用）。
    pub aggregate_error_constructor: P<JsObject>,
    /// `%AggregateError.prototype%`（proto = %Error.prototype%）。
    pub aggregate_error_proto: P<JsObject>,
    /// `%AsyncFunction.prototype%`：异步函数对象的原型（`constructor` 指向 `%AsyncFunction%`）。
    pub async_function_proto: P<JsObject>,
    /// `%AsyncGeneratorPrototype%`：异步生成器实例的原型（next/return/throw/@@asyncIterator）。
    pub async_generator_proto: P<JsObject>,
    /// `%AsyncGeneratorFunction.prototype%`：异步生成器函数对象的原型。
    pub async_generator_function_proto: P<JsObject>,
    /// 微任务队列（Promise reactions / thenable 委托），`run()` 末尾 FIFO drain。
    pub(crate) job_queue: VecDeque<crate::promise::Microtask>,
    pub math_rng_state: u64,
    /// 子模块平表的表代际注册表：键 = 表代际（`current_gen` 为当前 run 正在
    /// 装载的代际）。函数对象创建时记录自身所属代际（`JsObject::table_gen`），
    /// 调用期按创建期代际解析平表——跨 run 换表后存活函数仍命中原表。
    /// 条目为 `Arc<CompiledModule>`，与调用方模块树共享：顶层条目即调用方模块
    /// Arc 同一实例，`run()` 全程只做 Arc::clone，无深拷贝。run 边界回收无
    /// 存活函数对象引用的代际（`reclaim_unreferenced_tables`），内存有界于
    /// 存活函数对象数，不随 run 数单调增。
    pub(crate) tables: HashMap<u32, Box<TableGen>>,
    /// 当前 run 正在装载的表代际（构造器预登记 gen 0 空表占位，run() 换表时 +1）。
    pub(crate) current_gen: u32,
    /// 帧切换时暂存调用方字节码的 Arc 栈（与 `bytecode` 同共享语义）。
    pub(crate) saved_bytecode_stack: Vec<Arc<[opcode::Instr]>>,
    pub(crate) saved_immutables_stack: Vec<*const [JsValue]>,
    /// 共享寄存器保存栈。每个活动 `CallFrame` 在 push 时把调用方活跃寄存器
    /// （`regs[..caller_reg_limit]`）按 `saved_reg_offset` 存到这里；恢复时复制回
    /// 并截断。容量跨调用保留，避免每次调用堆分配。
    pub(crate) save_stack: Vec<JsValue>,
    /// VM 级 spill 栈。`CallFrame.spill_offset` 定位本帧区：调用子函数时从边界后分配，
    /// 帧恢复时截断到边界，子函数 spill 数据随帧丢弃。
    pub(crate) spill_stack: Vec<JsValue>,
    /// 本次 native 调用的 spill 溢出实参区：`spill_stack[base..base+count)`。
    /// 实参数超过寄存器窗口（253）时，窗口外的实参转存 spill 栈（GC 根），
    /// native 侧经 `VmHost::native_arg_count`/`native_arg_at` 读取。仅在一次
    /// native 调用期间有效，调用返回前截断回收。
    pub(crate) native_overflow_base: usize,
    pub(crate) native_overflow_count: usize,
    pub(crate) try_stack: Vec<TryHandler>,
    pub(crate) exception_value: Option<JsValue>,
    /// 同步调用抛出的原始 JsValue 侧通道：`call_function_sync`/`unwind` 把错误展平为
    /// String 后，for-of 需要重新抛出原值。放在 VM 顶层字段（不在 InlineSyncState 中）
    /// 以便跨内联调用的恢复过程存活。
    pub(crate) last_uncaught_value: Option<JsValue>,
    pub(crate) pending_exception: Option<JsValue>,
    pub(crate) pending_error_kind: Option<&'static str>,
    /// 控制流完成（break/continue/return）暂存，finally 执行后由 TRY_FINALLY_END 恢复。
    pub(crate) pending_completion: Option<Completion>,
    pub(crate) root_reg_limit: u8,
    pub(crate) active_reg_limit: u8,
    pub(crate) native_call_depth: usize,
    /// JS 重入 hop 计数：native 体内（`native_call_depth > 0`）每次嵌套 dispatch
    /// 进入计一跳。每 64 跳强制一次顶层分配上限采样——顶层 dispatch 冻结在 native
    /// 体内时 `steps` 不推进、重入 dispatch 又太短到不了循环内采样点，分配上限
    /// 否则结构性永不采样（小重入泵送盲区）。run 边界重置。
    pub(crate) reentry_hops: u64,
    /// inline 同步调用（`call_bytecode_function_inline`，frames 为空）的实参区位置。
    /// frames 非空时 CREATE_ARGUMENTS 优先读当前帧的实参区；此字段只服务内联路径。
    pub(crate) inline_args_base: u32,
    pub(crate) inline_args_count: u16,
    /// `ordinary_get` 压入字节码 accessor 帧时设为 Some(target_reg)。
    /// 调度循环检查该标志，跳过用调用结果写 `regs[target_reg]` —— 值改由 RETURN
    /// 处理器交付。
    pub(crate) accessor_frame_target_reg: Option<u8>,
    /// `call_bytecode_function_inline` 执行期间当前回调闭包；LOAD/STORE_UPVALUE 在
    /// frames 为空（inline 隔离状态）时由此取闭包 upvalues。嵌套 inline 由
    /// InlineSyncState 保存/恢复。
    pub(crate) inline_callee: Option<JsValue>,
    /// inline 目标函数的严格模式标志（与 `inline_callee` 同生命周期，
    /// InlineSyncState 保存/恢复）：内联执行期间帧表被隔离，写路径的
    /// strict/sloppy 判定取此值而非帧标志。
    pub(crate) inline_strict: bool,
    /// inline 起始时 `frames.len()` 基线（随 InlineSyncState 保存/恢复）：
    /// 内联执行中新压的帧（CALL/accessor）越过该基线，其严格性归帧栈顶。
    pub(crate) inline_frames_base: usize,
    /// 本次 run 顶层脚本的严格模式标志（`module.is_strict`，run 时填充）：
    /// 无帧且无 inline（顶层脚本赋值）时写路径的 strict/sloppy 判定来源。
    pub(crate) top_level_strict: bool,
    /// 顶层 this 值（run 初始化时记录：脚本 = 全局对象，ES module = undefined）。
    /// rerun 清空寄存器文件后据此恢复 regs[254]——不恢复则重执行时依赖 this
    /// 的顶层写（顶层 var 全局同步写）在非对象 this 上静默 no-op。
    pub(crate) top_level_this: JsValue,
    /// inline 同步调用寄存器窗口缓冲池：`save_inline_state` 取出复用、
    /// `restore_inline_state` 归还。热回调循环内 save/restore 反复使用同一块
    /// 缓冲，只在嵌套（池已被外层取走）时新分配。
    pub(crate) inline_reg_pool: Option<Vec<JsValue>>,
    /// 生成器体 `dispatch()` 让出时的信号：YIELD 置 Some(让出值)，恢复方（
    /// generator 内嵌 dispatch 循环）取走并判定挂起。None = 正常返回/异常。
    pub(crate) generator_suspended: Option<JsValue>,
    /// `yield*` 委托中的内层迭代器：YIELD_STAR 让出时置入，恢复时转发 next/return/throw
    /// 后按结局清空。随生成器挂起/恢复经 GeneratorState 传递（snapshot/rewrite 共管）。
    pub(crate) delegated_iterator: Option<JsValue>,
    /// 当前是否处于生成器内嵌 dispatch 循环：生成器帧弹出且 frames 清空时，
    /// `do_return` 据此把结果交付给恢复方（而非当作普通顶层返回继续执行）。
    /// 作用域限定于置位处包裹的那次 `dispatch()`：经 `InlineSyncState` 在
    /// state-swap 边界保存/清零/恢复，嵌套内联调用不继承本标志。
    pub(crate) generator_dispatch: bool,
    /// 生成器调用时参数初始化步：body 起点标记（SUSPEND_BODY）据此判定"挂起在 body 前"。
    pub(crate) generator_init_step: bool,
    /// 参数初始化步中已越过 body 起点标记的信号（`initialize_generator` 消费后复位）。
    pub(crate) generator_body_started: bool,
    /// 当前正在执行的异步函数上下文对象（`OBJ_TYPE_ASYNC`，持有 AsyncState 快照）。
    /// AWAIT dispatch 据此登记恢复反应；跨嵌套 async 调用保存/恢复。
    pub(crate) async_context: Option<JsValue>,
    /// 异步体 `dispatch()` 让出时的信号：AWAIT 置 true，恢复方（async 内嵌 dispatch
    /// 循环）取走并快照挂起状态。false = 正常返回/异常。
    pub(crate) async_suspended: bool,
    /// 当前是否处于异步函数内嵌 dispatch 循环：异步帧弹出且 frames 清空时，
    /// `do_return` 据此把结果交付给恢复方（与 generator_dispatch 同语义，
    /// 同样经 InlineSyncState 限定作用域于属主 dispatch）。
    pub(crate) async_dispatch: bool,
    /// 当前是否处于构造器内嵌 dispatch 循环（`call_constructor_bytecode_inline`）：
    /// 构造帧弹出且 frames 清空时，`do_return` 据此把构造结果（regs[0]，
    /// 已做非对象回退 this）交付给恢复方（与 generator_dispatch 同语义，
    /// 同样经 InlineSyncState 限定作用域于属主 dispatch）。
    pub(crate) construct_dispatch: bool,
    /// 当前正在执行的异步生成器上下文对象（`OBJ_TYPE_ASYNC_GENERATOR`，持有
    /// `AsyncGeneratorState` 快照）。AWAIT dispatch 据此登记异步生成器恢复反应；
    /// 跨嵌套 async 调用保存/恢复（与 `async_context` 同生命周期，二者互斥占用）。
    pub(crate) async_gen_context: Option<JsValue>,
    /// 当前是否处于异步生成器内嵌 dispatch 循环：AWAIT 据此走异步生成器恢复
    /// 闭包；yield 让出复用 `generator_suspended` 信号。
    pub(crate) async_gen_dispatch: bool,
    /// 异步生成器 body `dispatch()` 的 AWAIT 让出信号：置 true 表示挂起在 await，
    /// 恢复方（异步生成器内嵌 dispatch 循环）据此快照挂起状态。
    pub(crate) async_gen_suspended: bool,
    /// 分组保存 session arena / GC 簿记状态。
    pub(crate) gc_state: GcState,
    /// 分组保存 `Symbol` intern 状态。
    pub(crate) symbols: SymbolState,
    /// 分组保存活跃的 for-in / for-of 迭代器状态。
    pub(crate) iters: IterState,
    /// 分组保存 inline cache 与指令计数器。
    pub(crate) profiling: ProfilingState,
    pub(crate) cell_stack: Vec<Vec<*mut Cell>>,
    /// 标签模板对象缓存（GetTemplateObject）：键 = (表代际, 模块 flat_id, site 序号)。
    /// 同一代际同 flat_id 同 site 恒返回同一对象；每次 `run()` 清空（缓存只留
    /// 本次 run 的条目，规模有界）。键含代际维度后，跨 run 调用旧代模块的
    /// flat_id 重编号不再误命中。值为 GC 根（for_each_value/rewrite_values 遍历）。
    pub(crate) template_objects: HashMap<(u32, u32, u32), JsValue>,
    /// 当前活动字节码所属模块的 flat_id（顶层 0；帧切换时随 bytecode 换）。
    /// GET_TEMPLATE_OBJECT 据其区分不同编译树（eval 每次编译独立 site）。
    pub(crate) active_flat_id: u32,
    /// 当前活动字节码所属子模块平表的表代际（帧切换时随 flat_id 同步换）。
    /// 模板缓存键含该代际：跨 run 调用的旧代模块与当前 run 模块 flat_id
    /// 重编号互不碰撞。
    pub(crate) active_table_gen: u32,
    /// 帧切换时暂存调用方 flat_id 的栈（与 saved_bytecode_stack 同步 push/pop）。
    pub(crate) saved_flat_id_stack: Vec<u32>,
    /// 帧切换时暂存调用方表代际的栈（与 saved_flat_id_stack 同步 push/pop）。
    pub(crate) saved_table_gen_stack: Vec<u32>,
}

impl Drop for Vm {
    fn drop(&mut self) {
        // 边界守卫计数：与构造器登记恰好配对（Rust 所有权保证恰好一次）。
        self.kernel_core.note_vm_ended();
        // 直接 drop（test262 每测试新建即弃）不经 reset/full_reset 路径：
        // 统一收尾释放全部 session 堆数据与内建原型属性区，防逐测试累积泄漏。
        self.teardown_intrinsic_protos();
        self.teardown_session_heap_data();
    }
}

impl Vm {
    const SYNC_NATIVE_ARG_BASE: usize = 0;
    const SYNC_NATIVE_ARG_LIMIT: usize = 253;

    /// 推进线性同余 RNG 一步（Math.random 用）。首次调用以系统时间纳秒播种。
    pub fn step_rng(&mut self) {
        if self.math_rng_state == 0 {
            self.math_rng_state = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            vm_debug!("step_rng: seeded");
        }
        self.math_rng_state = self
            .math_rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
    }

    /// 读取当前 RNG 状态生成的 [0,1) 浮点数。
    pub fn math_rng_value(&self) -> f64 {
        (self.math_rng_state >> 33) as f64 / (1u64 << 31) as f64
    }

    /// 只读访问 VM 共享的 `KernelCore`。
    pub fn kernel_core(&self) -> &Arc<KernelCore> {
        &self.kernel_core
    }

    /// 只读访问当前 session（builtin world 与 global object）。
    pub fn session(&self) -> &KernelSession {
        &self.session
    }

    pub(crate) fn is_object_prototype(&self, ptr: *const JsObject) -> bool {
        let proto_ptr = self.session.builtin_world().object_proto.as_ptr();
        std::ptr::eq(ptr, proto_ptr)
    }

    /// 读取寄存器 `idx` 的值（`VmHost` 与 native 绑定的统一入口）。
    pub fn reg(&self, idx: u8) -> JsValue {
        self.regs[idx as usize]
    }

    /// 写入寄存器 `idx` 的值。
    pub fn set_reg(&mut self, idx: u8, val: JsValue) {
        self.regs[idx as usize] = val;
    }

    /// 只读访问 VM 的 epoch arena。
    pub fn epoch(&self) -> &Epoch {
        &self.epoch
    }

    /// 若 `val` 是字符串，返回其内容的 `String` 副本；否则返回 `None`。
    pub fn lookup_str(&self, val: JsValue) -> Option<String> {
        if !val.is_string() {
            return None;
        }
        // SAFETY: val 是字符串值，其 JsString 指针在生命周期内有效。
        Some(unsafe { (*val.as_string_ptr()).to_owned_string() })
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl Vm {
    /// 测试用：当前代际的子模块平表只读视图（消费点全在测试钉）。
    pub(crate) fn current_table(&self) -> &TableGen {
        self.tables.get(&self.current_gen).expect("当前代际表构造期已预登记")
    }

    /// 测试用：整表替换当前代际的子模块平表并同步常量缓存槽位（绕过 run() 装载，
    /// 直接注入模块表后压帧/内联调用）。
    pub(crate) fn install_module_table_for_test(&mut self, modules: Arc<Vec<Arc<CompiledModule>>>) {
        let table = self.current_table_mut();
        table.modules = modules;
        table.immutables.resize(table.modules.len(), OnceLock::new());
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_index_of, canonical_index_units};
    use oxide_types::private_key::INT_KEY_COUNT;

    fn units_of(s: &str) -> Vec<u16> {
        s.chars().map(|c| c as u16).collect()
    }

    #[test]
    fn canonical_index_str_boundaries() {
        assert_eq!(canonical_index_of("0"), Some(0));
        assert_eq!(canonical_index_of("1073741823"), Some(INT_KEY_COUNT - 1));
        // 2^30 起不再是整数键候选；2^32 边界串（ToUint32 回绕源）必须走字符串键
        assert_eq!(canonical_index_of("1073741824"), None);
        assert_eq!(canonical_index_of("4294967295"), None);
        assert_eq!(canonical_index_of("4294967296"), None);
        assert_eq!(canonical_index_of("4294967299"), None);
        // 非规范形态：前导零、符号、小数、空串
        assert_eq!(canonical_index_of("007"), None);
        assert_eq!(canonical_index_of("-1"), None);
        assert_eq!(canonical_index_of("1.5"), None);
        assert_eq!(canonical_index_of(""), None);
    }

    #[test]
    fn canonical_index_units_boundaries() {
        assert_eq!(canonical_index_units(&units_of("0")), Some(0));
        assert_eq!(canonical_index_units(&units_of("1073741823")), Some(INT_KEY_COUNT - 1));
        assert_eq!(canonical_index_units(&units_of("1073741824")), None);
        // 10 位串在 u32 累加尾位溢出：回绕会造出假规范下标，须溢出检查归 None
        assert_eq!(canonical_index_units(&units_of("4294967295")), None);
        assert_eq!(canonical_index_units(&units_of("4294967296")), None);
        assert_eq!(canonical_index_units(&units_of("4294967299")), None);
        assert_eq!(canonical_index_units(&units_of("9999999999")), None);
        assert_eq!(canonical_index_units(&units_of("007")), None);
        assert_eq!(canonical_index_units(&units_of("-1")), None);
        assert_eq!(canonical_index_units(&units_of("1.5")), None);
        assert_eq!(canonical_index_units(&[]), None);
    }
}
