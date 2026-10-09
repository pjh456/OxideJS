//! 基于寄存器的 JS 虚拟机：执行状态、寄存器文件、调用栈与 session 内存，
//! 以及同步调用、指令主循环、异常通道、GC 钩子、属性解析等子模块的装配。

#![allow(clippy::arc_with_non_send_sync)]

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
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
use crate::realm::Realm;
use crate::vm_debug;
use crate::vm_state::{IterState, ProfilingState, SampleState};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::error::JsErrorKind;
use oxide_types::object::{Cell, JsObject, JsString, NativeFnPtr};
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

pub(crate) use dynamic::NoopCompilerService;
pub(crate) use frames::FrameArgs;
pub use frames::{CallFrame, Completion, ForInIter, FrameContinuation, TryHandler};
pub(crate) use gc_hooks::RootGroup;
pub(crate) use inline::InlineSyncState;
pub(crate) use tables::TableGen;

/// 原型链解析深度上限：防超长或成环的原型链把属性查找拖入无界循环。
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

/// 按 name/msg 拼接错误文本：两者皆非空时为 `name: msg`，任一为空则取另一个。
pub(crate) fn format_error_message(name: &str, msg: &str) -> String {
    if name.is_empty() {
        msg.to_string()
    } else if msg.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {msg}")
    }
}

/// 逃出 for-await-of 的异步关闭在途状态：return() 的 promise 挂起等待结算，
/// 结算闭包（微任务）恢复挂起帧后继续关闭剩余条目并执行完成。
#[derive(Debug)]
pub(crate) struct PendingAsyncEscape {
    /// 正在等待的 return() promise（结算闭包陈旧防御校验用）。
    pub(crate) close_promise: JsValue,
    /// 悬挂的完成（Break/Continue/Return），结算后执行。
    pub(crate) completion: Completion,
    /// 剩余待关闭的异步迭代器（LIFO 序，前元素先关）。单层路径恒空，多层路径填充。
    pub(crate) remaining: Vec<JsValue>,
}

/// 基于寄存器的 JS 虚拟机：持有执行状态、寄存器文件、调用栈与 session 内存。
///
/// 执行入口为 [`Vm::run`]（见 `vm_runtime` 模块）；内存模型为统一入口分配 +
/// session 对象（可被 `SessionGc` 原地清扫回收，不搬移、地址稳定）+ session 字符串。多数内部字段为
/// `pub(crate)`，对外提供统计与内省 getter。
pub struct Vm {
    pub(crate) regs: [JsValue; 256],
    pub(crate) pc: usize,
    /// 当前活动字节码。以 `Arc<[Instr]>` 共享：函数调用经 `Arc::clone` 换帧（O(1)），
    /// 不再逐帧深拷贝。顶层装载期按引用计数条件复制（共享时显式深拷贝，独占时
    /// 零拷贝），子模块平表源保持惰性 COW，IC 写回经 `bytecode_mut` 的
    /// `Arc::make_mut` 写时复制，保证独占后才改写（miss 时才深拷贝，频率低）。
    pub(crate) bytecode: Arc<[opcode::Instr]>,
    /// 当前活动模块已转换不可变常量的只读视图（指向当前表代际的 immutables 内部）。
    /// 用胖 `*const`：常量 Vec 归表代际所有且 OnceLock 只填一次（堆分配地址稳定）。
    pub(crate) active_immutables: *const [JsValue],
    pub(crate) frames: SmallVec<[CallFrame; 16]>,
    pub(crate) kernel_core: Arc<KernelCore>,
    /// 动态编译服务句柄：`Function` 构造器 / eval / `$262.evalScript` 经它编译
    /// 源码（`dynamic.rs` 三方法）。缺省为 no-op stub（返 `Err`），生产 entry
    /// points 与动态编译测试经 `set_compiler_service` 注入真实实现。
    /// `Arc<dyn CompilerService>` 因 trait `Send + Sync` 而 Send + Sync，
    /// 不破坏 `unsafe impl Send for Vm`。
    pub(crate) compiler: Arc<dyn oxide_runtime_api::CompilerService>,
    /// 每 VM 的 realm 组合：内核会话（builtin world 与 global 对象）、
    /// session GC 簿记与 10 个内建原型槽（见 `realm` 模块）。以 `Arc` 持有：
    /// 可变组经 `RefCell` 内部可变性在 `&Arc<Realm>` 下改写，teardown 归
    /// `Drop for Realm`（per-realm 消亡：Arc 计数归零时收尾）。
    pub(crate) realm: Arc<Realm>,
    /// `"length"` 属性键的 intern id 缓存：进程内稳定（PermInterner append-only、
    /// KernelCore 不重建），属性 get/set 热路径免每次 intern（hash64 + DashMap +
    /// RwLock 读锁）。
    pub(crate) length_si: u32,
    /// `"length"` 属性键 perm 串的内部指针：IC 站点的键寄存器恒为 perm 串
    /// （发射层字符串常量统一经 perm_string 物化），指针相等即键为 "length"，
    /// 入口快判只比指针免键解析。PermInterner 追加式、指针唯一，永不失效。
    pub(crate) length_perm_ptr: *const JsString,
    /// f64→string 十六槽 last-value 缓存的键（double 位模式）。空槽判定用
    /// 对应值非字符串：键 0 是 +0.0 的合法位模式，不得作空标记。
    pub(crate) number_to_string_cache_keys: [u64; 16],
    /// f64→string 十六槽 last-value 缓存的值（session 串，经 `for_each_value`
    /// 登记为 GC 根；`full_reset` 在 session 串释放前清空）。
    pub(crate) number_to_string_cache_vals: [JsValue; 16],
    /// 微任务队列（Promise reactions / thenable 委托），由 `drain_microtasks`
    /// 在 `run()` 末尾与事件循环每个 turn 边界 FIFO drain。
    pub(crate) job_queue: VecDeque<crate::promise::Microtask>,
    /// Atomics.waitAsync waiter 表：键 = (缓冲对象指针, 元素字节偏移)，值 =
    /// 登记的 promise FIFO。键取登记时刻视图的 `buffer` 现指针（同 run 无 GC 时
    /// notify 侧读同一指针恒匹配）；run 边界清空，清位后 promise 无强根自然回收。
    pub(crate) atomics_waiters: HashMap<(u64, usize), Vec<JsValue>>,
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
    /// 数组 length define 强转期用户代码抛出的原始异常：与 `last_uncaught_value`
    /// 分离，避免入口误取其它操作忽略调用残留的值。仅 `define_array_length` 写入，
    /// 由 Object/Reflect define 入口消费。
    pub(crate) pending_length_exception: Option<JsValue>,
    pub(crate) pending_exception: Option<JsValue>,
    pub(crate) pending_error_kind: Option<&'static str>,
    /// 控制流完成（break/continue/return）暂存，finally 执行后由 TRY_FINALLY_END 恢复。
    pub(crate) pending_completion: Option<Completion>,
    /// 逃出 for-await-of 的异步关闭在途状态：return() 的 promise 挂起等待结算，
    /// 结算闭包（微任务）恢复挂起帧后继续关闭剩余条目并执行完成。
    pub(crate) pending_async_escape: Option<PendingAsyncEscape>,
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
    /// `call_bytecode_function_inline` 执行期间当前回调闭包（inline 同步调用语义）。
    /// LOAD/STORE_UPVALUE 热路径直接读 `active_upvalues` 活动镜像，本字段仅经
    /// `current_callee()` 服务冷路径：惰性建 cell 路径建 cell 时回写 callee 对象，
    /// CREATE_CLOSURE 由此取父 upvalue 源。嵌套 inline 由 InlineSyncState 保存/恢复。
    pub(crate) inline_callee: Option<JsValue>,
    /// 当前执行函数的 upvalue cell 表（活动镜像，与 `CallFrame.upvalues` 同型：
    /// 胖指针，非闭包为 null 空切片）。压帧/内联/弹帧/挂起恢复四个边界置位，
    /// run 边界清零；指针恒有效——callee 对象是 GC 根，`upvalues` Box 创建后不
    /// 替换，LOAD/STORE_UPVALUE 热路径直接按下标取 cell。
    pub(crate) active_upvalues: *const [*mut Cell],
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
    /// 外层 native pack 实参区上界（独占）：`call_function_sync` native 分支在飞
    /// 期间实参已 pack 进 `regs[0..pack_end)`，恢复边界（`restore_inline_state` /
    /// `restore_frame`）的镜像重载须跳过该域——重载目标恰是外层 builtin 尚未
    /// 读取的实参寄存器。0 = 无 pack 上下文；跨嵌套 native 调用存/还原
    /// （与 overflow 描述符同构）。
    pub(crate) native_pack_end: usize,
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
    /// 当前 native 调用是否以构造形态发起（NEW/SUPER native 臂与
    /// `construct_with` native 臂在调用前置 true，普通调用入口前置 false）：
    /// builtin 构造器（`typed_array_new`）据此判定向 receiver 物化，替代
    /// 寄存器推断——native 调用与调用方共享寄存器文件，类构造器帧内的
    /// new.target 槽会残留类构造器对象，按寄存器判定会把成员式普通调用
    /// 误判为构造。调用结束即恢复/清零，不跨 native 调用存活。
    pub(crate) constructing_native: bool,
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
    /// 分组保存活跃的 for-in / for-of 迭代器状态。
    pub(crate) iters: IterState,
    /// 分组保存 inline cache 与指令计数器。
    pub(crate) profiling: ProfilingState,
    /// 分组保存指令周期采样状态（周期、top-K、样本记录）。
    pub(crate) sampling: SampleState,
    pub(crate) cell_stack: Vec<Vec<*mut Cell>>,
    /// 标签模板对象缓存（GetTemplateObject）：键 = (表代际, 模块 flat_id, site 序号)。
    /// 同一代际同 flat_id 同 site 恒返回同一对象；每次 `run()` 清空（缓存只留
    /// 本次 run 的条目，规模有界）。键含代际维度后，跨 run 调用旧代模块的
    /// flat_id 重编号不再误命中。值为 GC 根（for_each_value 遍历）。
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
    /// 逐指令 trace 开关：开启时 dispatch 主循环对每条指令向 stderr 写一行
    /// `pc + opcode + 操作数`。运行时开关，旁路 tracing 级别上限（release
    /// 构建可用）；默认关闭，关闭时热路径仅一次 bool 比较，零输出零分配。
    pub trace_instructions: bool,
    /// last-pc 现场文件路径：开启时 dispatch 主循环每 2^16 指令追加写一行
    /// `pc/opcode/flat_id/frames`，供监督者超时/崩溃杀子进程后读回挂死点；
    /// 默认 None（零输出零分配）。
    pub(crate) pc_watch: Option<PathBuf>,
    /// 每请求步数上限覆盖：执行路径按请求设置，None 回退内核配置。
    /// 池回收的 `full_reset` 路径清位；`run()` 入口的 `clear_execution_state`
    /// 刻意不清（清在那会把执行路径刚设的覆盖抹掉）。
    pub(crate) max_steps_override: Option<u64>,
    /// worker 注册表：键 = worker 编号，值 = `WorkerHandle`（主线程 → worker
    /// 通道 + worker → 主线程通道 + OS 线程句柄）。worker 对象被 GC 不终止
    /// worker 线程，条目孤儿至 `shutdown_workers()`（`Drop for Vm` 调用）。
    pub(crate) worker_registry: std::collections::HashMap<u64, crate::worker::WorkerHandle>,
    /// 下一个 worker 编号（单调递增，首个为 0）。
    pub(crate) worker_next_id: u64,
    /// Worker 对象注册表：键 = worker 编号，值 = Worker 对象（主 realm session
    /// 对象）。主线程事件循环据编号反查 Worker 对象读 `onmessage`。Worker 对象
    /// 是 GC 根（`for_each_value` 遍历），注册表保活至 `worker_terminate` /
    /// `full_reset` 清表，无悬垂风险。
    pub(crate) worker_objects: std::collections::HashMap<u64, JsValue>,
}

impl Drop for Vm {
    fn drop(&mut self) {
        // 先终止全部 worker 并 join（防线程泄漏）：worker 线程各持自有 Vm，
        // 其收尾独立于本 Vm，join 后线程不再引用本 Vm 的 kernel_core。
        self.shutdown_workers();
        // 边界守卫计数：与构造器登记恰好配对（Rust 所有权保证恰好一次）。
        self.kernel_core.note_vm_ended();
        // 活跃 for-in / for-of 迭代器是 per-VM 执行态：收尾路径逐条释放，
        // 防 Vm 直接 drop 时表内残留体泄漏。
        self.iters.reset();
        // realm 收尾归 `Drop for Realm`（per-realm 消亡：Arc 计数归零时触发，
        // 直接 drop 时恰好一次）。
    }
}

// SAFETY: Vm 可安全跨线程移动（Send），依据四条不变量：
//
// 1. session 堆随 Vm 整体迁移：Vm 内全部裸指针（session 对象表
//    session_object_ptrs、字符串表 session_string_ptrs、BigInt/cell 表、
//    upvalue cell 指针、active_immutables 视图、length_perm_ptr）指向的堆对象
//    均由该 Vm 独占分配（Box::into_raw 统一入口），地址稳定、不随 Vm 移动而
//    搬移。跨线程移动 Vm 时指针恒有效，不产生悬垂。
//
// 2. &Vm 不跨线程共享：Vm 只经 &mut Vm 在单线程内访问（池化 VmGuard 独占
//    借出），RefCell（GcState 的 BigInt/cell 表、Realm 的 session/gc/P 字段）
//    与 Cell（profiling 计数）的非 Sync 性不构成跨线程数据竞争。
//
// 3. GC 根集不跨线程共享：每个 Vm 持有独立的 session 堆、独立的 mark/sweep
//    表与水位，无跨 Vm 的 GC 边。KernelCore 经 Arc<KernelCore> 共享
//    （Send + Sync），是唯一的跨 Vm 共享面，其内部为 append-only forge 与
//    原子计数，无 per-VM 可变状态。
//
// 4. 裸指针不跨线程逃逸：Vm 内的裸指针只指向本 Vm 的 session 堆或共享 kernel
//    对象（perm 串 / shape / code forge），不指向其他 Vm 的堆。跨线程移动
//    Vm 时这些指针的指向不变（堆对象地址稳定），不引入别名。
//
// realm 字段为 Arc<Realm>：Realm 的 session 堆不跨线程共享，Arc 引用计数为
// 原子操作，跨线程移动 Arc 安全；Realm 只经 Vm（单线程 &mut）访问，
// 其非 Sync 性对 Send 无碍。
//
// 5. worker_registry 字段（HashMap<u64, WorkerHandle>）不持 session 堆指针：
//    WorkerHandle 的 tx（Sender<WorkerMail>）与 handle（JoinHandle<()>）均 Send，
//    rx_out（Receiver<MessageValue>）虽非 Send 但只在主线程（Vm 属主线程）经
//    poll_worker_messages 访问、不跨线程移动；MessageValue 是 Send 中间表示
//    （无 realm 局部指针）。跨线程移动 Vm 时 worker_registry 整体迁移，
//    其中无指向 session 堆的裸指针，不引入别名。
//
// 6. worker_objects 字段（HashMap<u64, JsValue>）持本 Vm session 堆的 Worker
//    对象指针，归不变量 1 覆盖：对象由本 Vm 独占分配、地址稳定、随 Vm 整体
//    迁移，跨线程移动不悬垂。
unsafe impl Send for Vm {}

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

    /// 注入动态编译服务（`Function` 构造器 / eval / `$262.evalScript` 的编译面）。
    ///
    /// # 边界与前提
    /// - 构造后缺省为 no-op stub（动态编译返 `Err`）；需要动态编译的调用方
    ///   （生产 entry points 与动态编译测试）须在首次动态编译前注入真实实现。
    ///
    /// # 副作用
    /// - 替换 `compiler` 句柄（`Arc` 克隆，无其他状态变更）。
    pub fn set_compiler_service(&mut self, service: Arc<dyn oxide_runtime_api::CompilerService>) {
        self.compiler = service;
    }

    /// 只读访问当前 session（builtin world 与 global object）。
    ///
    /// 返回 `Ref` 守卫（session 入 `RefCell` 后无法再给稳定 `&`）：调用方
    /// 在单表达式内消费，不跨 `borrow_mut` 长存。
    pub fn session(&self) -> std::cell::Ref<'_, KernelSession> {
        self.realm.session.borrow()
    }

    /// 读取本 VM 所属 realm 的编号（符号身份 = (realm 编号, 局部下标) 的 realm 维度）。
    pub fn realm_id(&self) -> u32 {
        self.realm.realm_id
    }

    /// 判定裸指针是否指向当前 session 的 `%Object.prototype%`。
    pub(crate) fn is_object_prototype(&self, ptr: *const JsObject) -> bool {
        let proto_ptr = self.realm.session.borrow().builtin_world().object_proto.as_ptr();
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

    /// 设置逐指令 trace 开关（开启后 dispatch 主循环逐指令向 stderr 写
    /// `pc + opcode + 操作数` 行，pc 为字节码 word 序号，与反汇编 offset 同单位）。
    pub fn set_instruction_trace(&mut self, on: bool) {
        self.trace_instructions = on;
    }

    /// 设置 last-pc 现场文件路径（开启后 dispatch 主循环每 2^16 指令追加写一行
    /// `pc/opcode/flat_id/frames`，供监督者超时/崩溃杀子进程后读回挂死点）；
    /// None 清除。
    pub fn set_pc_watch(&mut self, path: Option<PathBuf>) {
        self.pc_watch = path;
    }

    /// 设置每请求步数上限覆盖：dispatch 主循环取 `覆盖值.or(内核配置)`，
    /// 覆盖优先、None 回退内核配置。执行路径按请求必设（含 None），池回收的
    /// `full_reset` 路径清位。
    pub fn set_max_steps(&mut self, limit: Option<u64>) {
        self.max_steps_override = limit;
    }

    /// 设置指令周期采样周期（2 的幂，0 关闭）。开启后 dispatch 主循环每
    /// `period` 条指令记一条样本（flat_id、pc、opcode、frames 深度），run 末
    /// 按 flat_id 聚合并向 stderr 输出 top-K 直方图；关闭时热路径仅一次
    /// 可预测分支，零写零分配。
    pub fn set_sample_period(&mut self, period: u64) {
        self.sampling.period = period;
    }

    /// 设置采样直方图的 top-K 大小（默认 10）。
    pub fn set_sample_top_k(&mut self, k: usize) {
        self.sampling.top_k = k;
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
        table.si_tables.resize(table.modules.len(), OnceLock::new());
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
