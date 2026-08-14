#![allow(clippy::arc_with_non_send_sync)]

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

use num_traits::Zero;
use oxide_bytecode::module::{CompiledModule, Constant};
use oxide_bytecode::opcode::{self, OpCode};
use smallvec::SmallVec;

/// 初始化一个 session 的内置对象（global 槽位、各构造器与原型、IC 预热）。
pub use crate::bindings::init_kernel_builtins;
use crate::native::NativeFn;
use crate::session_gc::SessionGc;
use crate::vm_state::{GcState, IterState, ProfilingState, SymbolState};
use crate::{vm_debug, vm_error, vm_trace, vm_warn};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_runtime_api as coercion;
use oxide_runtime_api::NativeResult;
use oxide_types::error::{JsError, JsErrorKind};
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{Cell, JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::{
    int_key_value, is_int_key, make_int_key, make_symbol_key, make_well_known_symbol_key, INT_KEY_COUNT,
};
use oxide_types::value::{JsValue, PTR_MASK};

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

#[allow(unused_macros)]
macro_rules! throw_err {
    ($self:ident, $kind:ident, $msg:expr) => {{
        match $self.raise_error_kind(stringify!($kind), $msg) {
            Ok(()) => continue,
            Err(e) => return Err(e),
        }
    }};
}

macro_rules! binary_arith {
    ($self:ident, $a:expr, $b:expr, $rd:expr, $op:tt, $check_zero:expr) => {{
        let lv = $self.regs[$a];
        let rv = $self.regs[$b];
        if lv.is_int() && rv.is_int() {
            $self.regs[$rd] = JsValue::float(lv.as_int() as f64 $op rv.as_int() as f64);
        } else if lv.is_bigint() && rv.is_bigint() {
            let l = $self.bigint_value(lv).clone();
            let r = $self.bigint_value(rv).clone();
            if r.is_zero() && $check_zero {
                // 仅除/模对 BigInt 零除数抛 RangeError；SUB/MUL 等其余二元运算
                // 遇到 0n 必须正常计算（Number 路径走 f64 inf/NaN）。
                $self.raise_error_kind("RangeError", "Division by zero")?;
                $self.regs[$rd] = JsValue::undefined();
            } else {
                $self.regs[$rd] = $self.new_bigint(l $op r);
            }
        } else {
            // 注意：混合 BigInt/Number 检查不能在此前置（对象操作数如
            // `{valueOf: () => 2n}` 需先 ToPrimitive 再判定），统一放 coerce 之后。
            let l = $self.coerce_primitive_bounded(lv, false)?;
            let r = $self.coerce_primitive_bounded(rv, false)?;
            if l.is_bigint() && r.is_bigint() {
                // 包装对象 coerce 后暴露双 BigInt（如 Object(2n) / 2n）。
                let lv = $self.bigint_value(l).clone();
                let rv = $self.bigint_value(r).clone();
                if rv.is_zero() && $check_zero {
                    $self.raise_error_kind("RangeError", "Division by zero")?;
                    $self.regs[$rd] = JsValue::undefined();
                } else {
                    $self.regs[$rd] = $self.new_bigint(lv $op rv);
                }
            } else if l.is_bigint() != r.is_bigint() {
                // 包装对象 coerce 后暴露 BigInt 混合（如 Object(1n) - 1）。
                $self.raise_type_error("Cannot mix BigInt and other types, use explicit conversions")?;
                $self.regs[$rd] = JsValue::undefined();
            } else {
                let ln = coercion::to_number(l);
                let rn = coercion::to_number(r);
                $self.regs[$rd] = JsValue::float(ln $op rn);
            }
        }
    }}
}

/// 调用帧被挂起后，恢复时需要继续的执行方式。
///
/// `AccessorGet` 表示 accessor getter 返回后需把结果写回 `target_reg`；
/// `AccessorSet` 表示 setter 调用完成；`None` 为普通调用返回。
#[derive(Debug, Clone, Copy)]
pub enum FrameContinuation {
    None,
    AccessorGet { target_reg: u8 },
    AccessorSet,
}

/// 一次函数调用的调用帧：记录返回地址、调用方寄存器窗口与 `this`/`new.target`。
///
/// 调用方寄存器窗口在 `save_stack` 中按 `saved_reg_offset` 保存，返回时由
/// `restore_frame` 恢复；`continuation` 描述 getter/setter 场景的恢复方式。
pub struct CallFrame {
    pub return_addr: usize,
    pub function_name: u32,
    pub caller_reg_limit: u8,
    pub saved_reg_offset: u32,
    /// 记录本帧 spill 栈起始长度，作为 SPILL/UNSPILL 的帧边界基址（push 时快照）。
    pub spill_offset: u32,
    /// 本帧完整实参在 spill 栈的起始下标（帧恢复时随 spill 区一起截断丢弃）。
    pub arguments_base: u32,
    /// 实际传入的实参个数（可大于形参个数）。
    pub arguments_count: u16,
    pub saved_this: JsValue,
    pub saved_new_target: JsValue,
    pub callee: JsValue,
    pub construct_result_reg: Option<u8>,
    pub constructed_this: Option<JsValue>,
    pub is_derived_constructor: bool,
    pub continuation: FrameContinuation,
}

/// 一次 `for-in` 迭代的游标：已收集的 key 列表与当前下标。
///
/// 每个 key 与它的 intern id 配对保存，使整型下标 key 无需重新 intern 即可排到字符串 key 之前。
pub struct ForInIter<'bump> {
    /// 每个 key 与其字符串 intern id 配对，使 for-in 排序时整型下标 key 无需
    /// 重新 intern 即可排到字符串 key 之前。
    pub keys: bumpalo::collections::Vec<'bump, (JsValue, u32)>,
    pub index: usize,
}

/// try/catch/finally 处理记录，异常展开时用于定位跳转目标与清理范围。
#[derive(Debug, Clone, Copy)]
pub struct TryHandler {
    pub catch_pc: Option<usize>,
    pub finally_pc: Option<usize>,
    /// finally 体是否已进入（normal JMP / 异常展开 / 完成穿越都会置位）。
    /// 用标志而非 pc 范围判定"当前是否执行 finally 体"：try 体末指令可能因 DCE
    /// 紧贴 finally 入口，pc 边界会误判。
    pub finally_active: bool,
    pub frame_depth: usize,
    /// try 入口时 for_of_iters 的长度，界定异常展开时哪些迭代器需要 IteratorClose。
    pub for_of_depth: usize,
}

/// 控制流完成：break/continue/return 逃出 finally 域时暂存的完成目标。
///
/// 仿 `pending_exception` 的侧通道：finally 执行期间悬挂在此，由 TRY_FINALLY_END
/// 逐个恢复（`remaining_finally` 为仍需穿越的 finally 体数，进入一个递减一个）。
#[derive(Debug, Clone, Copy)]
pub enum Completion {
    Break { target_pc: usize, remaining_finally: usize },
    Continue { target_pc: usize, remaining_finally: usize },
    Return { value: JsValue, remaining_finally: usize },
}

impl Completion {
    /// 仍需穿越的 finally 体数。
    pub fn remaining_finally(&self) -> usize {
        match *self {
            Completion::Break { remaining_finally, .. }
            | Completion::Continue { remaining_finally, .. }
            | Completion::Return { remaining_finally, .. } => remaining_finally,
        }
    }

    /// 复制并改写剩余 finally 计数（进入一个 finally 后递减）。
    pub fn with_remaining(&self, remaining: usize) -> Completion {
        match *self {
            Completion::Break { target_pc, .. } => Completion::Break {
                target_pc,
                remaining_finally: remaining,
            },
            Completion::Continue { target_pc, .. } => Completion::Continue {
                target_pc,
                remaining_finally: remaining,
            },
            Completion::Return { value, .. } => Completion::Return {
                value,
                remaining_finally: remaining,
            },
        }
    }
}

/// `call_bytecode_function_inline` 使用的堆分配快照。
/// 放在堆上避免 JS 代码链式同步字节码调用（如 sort 比较器、accessor）时
/// 耗尽 Rust 栈。
pub(crate) struct InlineSyncState {
    /// 寄存器窗口副本：`regs[0..len]`，len ≤ 253。`regs[254]/[255]` 不在此列，
    /// 由 `saved_this`/`saved_new_target` 单独保存（callee 也会重写这两个槽）。
    pub(crate) regs: Box<[JsValue]>,
    pub(crate) saved_this: JsValue,
    pub(crate) saved_new_target: JsValue,
    pub(crate) pc: usize,
    pub(crate) bytecode: Arc<[opcode::Instr]>,
    pub(crate) active_immutables: *const [JsValue],
    pub(crate) active_reg_limit: u8,
    pub(crate) root_reg_limit: u8,
    pub(crate) try_stack: Vec<TryHandler>,
    pub(crate) frames: SmallVec<[CallFrame; 16]>,
    pub(crate) exception_value: Option<JsValue>,
    pub(crate) pending_exception: Option<JsValue>,
    pub(crate) pending_error_kind: Option<&'static str>,
    pub(crate) pending_completion: Option<Completion>,
    pub(crate) for_in_iters: Vec<*mut ForInIter<'static>>,
    pub(crate) for_of_iters: Vec<JsValue>,
    pub(crate) last_for_of_result: JsValue,
    pub(crate) saved_bytecode_stack: Vec<Arc<[opcode::Instr]>>,
    pub(crate) saved_immutables_stack: Vec<*const [JsValue]>,
    pub(crate) save_stack: Vec<JsValue>,
    pub(crate) spill_stack: Vec<JsValue>,
    pub(crate) cell_stack: Vec<Vec<*mut Cell>>,
    pub(crate) inline_callee: Option<JsValue>,
    pub(crate) inline_args_base: u32,
    pub(crate) inline_args_count: u16,
    pub(crate) accessor_frame_target_reg: Option<u8>,
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
    /// 不再逐帧深拷贝。与 sub_modules 源共享同一缓冲，IC 写回经 `bytecode_mut` 的
    /// `Arc::make_mut` 写时复制，保证独占后才改写（miss 时才深拷贝，频率低）。
    pub(crate) bytecode: Arc<[opcode::Instr]>,
    /// 每次 run 转换一次的不可变常量缓存。下标 0 = 顶层模块，sub_idx+1 = sub_modules[sub_idx]。
    /// 每个 `OnceLock` 保存该模块常量本次运行中只转换一次的 `JsValue` 结果，每次 `run()` 重建。
    /// 不可变常量是标量 + perm 字符串，只读，不作为 GC 根。
    pub(crate) immutables_cache: Vec<OnceLock<Vec<JsValue>>>,
    /// 当前活动模块已转换不可变常量的只读视图（指向 immutables_cache 内部）。
    /// 用胖 `*const`：缓存 Vec 归 VM 所有且本次运行稳定（OnceLock 只填一次）。
    pub(crate) active_immutables: *const [JsValue],
    pub(crate) frames: SmallVec<[CallFrame; 16]>,
    pub(crate) kernel_core: Arc<KernelCore>,
    pub(crate) session: KernelSession,
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
    /// 全局扁平模块表：下标 = 模块 `flat_id`（顶层 0，子模块 flatten 后全局唯一）。
    /// 闭包 `sub_module_index` 即 flat_id，逃逸闭包也能自足解析。
    /// 条目为 `Arc<CompiledModule>`，与调用方模块树共享（`run()` 只做 Arc::clone，
    /// 不再每次深拷贝整棵子树）。
    pub(crate) sub_modules: Arc<Vec<Arc<CompiledModule>>>,
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
    /// `do_return` 据此把结果交付给恢复方（与 generator_dispatch 同语义）。
    pub(crate) async_dispatch: bool,
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
}

impl Vm {
    const SYNC_NATIVE_ARG_BASE: usize = 0;
    const SYNC_NATIVE_ARG_LIMIT: usize = 253;

    pub(crate) fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String> {
        if !value.is_object() {
            return Ok(value);
        }

        let obj_ptr = value.as_js_object_ptr();
        if obj_ptr.is_null() {
            return Ok(value);
        }

        // ECMA-262 §7.1.1 step 1: an exotic obj[Symbol.toPrimitive] takes precedence
        // over OrdinaryToPrimitive. well-known symbol 键经 property_key_si 映射为
        // 固定 Symbol 键，读键路径与写键路径一致。
        let sym_key = {
            let sym_ptr = self.session.builtin_world().sym_to_primitive.as_ptr() as *mut JsObject;
            JsValue::from_js_object(sym_ptr)
        };
        let sym_si = self.property_key_si(sym_key)?;
        let exotic = {
            let obj = unsafe { &*obj_ptr };
            self.ordinary_get(obj, sym_si, value)?
        };
        if !exotic.is_undefined() && !exotic.is_null() {
            let exotic_ptr = exotic.as_js_object_ptr();
            if !exotic.is_object() || exotic_ptr.is_null() || !unsafe { &*exotic_ptr }.is_function() {
                // 抛可捕获的 JS 异常（dispatch 层 try/catch 可捕获），见 conversion_error。
                self.conversion_error("Symbol.toPrimitive is not a function")?;
                return Ok(JsValue::undefined());
            }
            let hint_val = self.new_string(if prefer_string { "string" } else { "number" });
            let result = match self.call_function_sync(exotic, value, &[hint_val]) {
                Ok(r) => r,
                Err(err) => return self.raise_call_error(&err),
            };
            if result.is_object() {
                self.conversion_error("Cannot convert object to primitive value")?;
                return Ok(JsValue::undefined());
            }
            return Ok(result);
        }

        let method_names = if prefer_string { ["toString", "valueOf"] } else { ["valueOf", "toString"] };

        for method_name in method_names {
            let method_si = self.kernel_core.perm_interner().intern(method_name).0;
            let method = {
                let obj = unsafe { &*obj_ptr };
                self.ordinary_get(obj, method_si, value)?
            };
            if method.is_undefined() || method.is_null() {
                continue;
            }
            // OrdinaryToPrimitive：valueOf/toString 不可调用时跳过（IsCallable == false
            // 则 continue），不抛错；仅 @@toPrimitive 不可调用时抛 TypeError。
            if !method.is_object() {
                continue;
            }
            let method_ptr = method.as_js_object_ptr();
            if method_ptr.is_null() || !unsafe { &*method_ptr }.is_function() {
                continue;
            }

            let result = match self.call_function_sync(method, value, &[]) {
                Ok(r) => r,
                Err(err) => return self.raise_call_error(&err),
            };
            if !result.is_object() {
                return Ok(result);
            }
        }

        // 主 dispatch 抛可捕获异常（外围 JS try/catch 可捕获）；原生 builtin 内部
        // 只传播格式化 Err，由其调用边界恢复为异常对象（见 conversion_error）。
        self.conversion_error("Cannot convert object to primitive value")?;
        Ok(JsValue::undefined())
    }

    /// 对象转原始值失败时的统一出口：主 dispatch（native_call_depth == 0）下抛可捕获
    /// 的 JS 异常并就地展开到外围 try/catch；原生 builtin 内部（depth > 0）不得就地
    /// 展开（展开会消费 try 处理器，builtin 却继续执行并可能产生二次错误），改为返回
    /// 格式化 Err，由原生调用边界（call_function_sync → dispatch_native_call）恢复为
    /// 原始异常对象。
    #[inline(always)]
    fn conversion_error(&mut self, msg: &str) -> Result<(), String> {
        if self.native_call_depth == 0 {
            self.raise_error_kind("TypeError", msg)
        } else {
            Err(self.error_message_text("TypeError", msg))
        }
    }

    /// 把 `call_function_sync` 返回的调用错误恢复为原始异常值并走异常展开，
    /// 使外围 try/catch 可捕获（native 函数抛错时原值存于 `last_uncaught_value`）。
    ///
    /// # 注意事项
    /// 仅在主 dispatch（`native_call_depth == 0`）下展开——此时 try_stack 只含当前
    /// 字节码的处理器，展开后 pc/regs[0] 不会被中途的原生调用栈覆盖。原生 builtin
    /// 内部（depth > 0）必须传播错误，由其调用边界（dispatch_native_call）转换。
    pub(crate) fn raise_call_error(&mut self, err: &str) -> Result<JsValue, String> {
        if self.native_call_depth == 0 {
            let exc = self
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, err));
            let kind = self.thrown_error_kind(exc);
            self.exception_value = Some(exc);
            self.pending_error_kind = Some(kind);
            self.unwind()?;
            return Ok(JsValue::undefined());
        }
        Err(err.to_string())
    }

    pub(crate) fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String> {
        let primitive = self.coerce_primitive_bounded(value, false)?;
        Ok(coercion::to_number(primitive))
    }

    pub(crate) fn coerce_int32_bounded(&mut self, value: JsValue) -> Result<i32, String> {
        if value.is_int() {
            return Ok(value.as_int());
        }
        let n = self.coerce_number_bounded(value)?;
        if n == 0.0 || !n.is_finite() {
            return Ok(0);
        }
        let int = n.trunc().rem_euclid(4_294_967_296.0) as u32;
        if int > i32::MAX as u32 {
            Ok((int as i64 - 4_294_967_296i64) as i32)
        } else {
            Ok(int as i32)
        }
    }

    pub(crate) fn coerce_uint32_bounded(&mut self, value: JsValue) -> Result<u32, String> {
        if value.is_int() {
            return Ok(value.as_int() as u32);
        }
        let n = self.coerce_number_bounded(value)?;
        if n == 0.0 || !n.is_finite() {
            return Ok(0);
        }
        Ok(n.trunc().rem_euclid(4_294_967_296.0) as u32)
    }

    fn pack_sync_native_call_args(&mut self, receiver: JsValue, callee: JsValue, args: &[JsValue]) -> Vec<u8> {
        self.regs[253] = receiver;
        self.regs[254] = callee;

        let mut arg_regs = Vec::with_capacity(args.len() + 1);
        arg_regs.push(253);
        for (idx, arg) in args.iter().enumerate() {
            let reg = (Self::SYNC_NATIVE_ARG_BASE + idx) as u8;
            self.regs[reg as usize] = *arg;
            arg_regs.push(reg);
        }
        arg_regs
    }

    pub(crate) fn is_session_ptr(&self, obj_ptr: *mut JsObject) -> bool {
        if obj_ptr.is_null() {
            return false;
        }
        // SAFETY: obj_ptr 非空且指向本 session 拥有的 JsObject。
        unsafe { (*obj_ptr).is_session_epoch() }
    }

    pub(crate) fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject {
        let ptr = self.epoch.alloc(obj);
        self.gc_state.track_epoch_object(ptr);
        ptr
    }

    /// 活动模块已转换不可变常量的只读视图；任何 `run()` 之前为空。
    #[inline(always)]
    pub(crate) fn immutables(&self) -> &[JsValue] {
        if self.active_immutables.is_null() {
            &[]
        } else {
            // SAFETY: active_immutables 指向 VM 拥有的 immutables_cache 内的 OnceLock<Vec<JsValue>>，
            // Vec 只填充一次，本次运行期间不再重分配。
            unsafe { &*self.active_immutables }
        }
    }

    /// 激活模块 `cache_idx` 的不可变常量（0 = 顶层，sub_idx+1 = sub_modules[sub_idx]），
    /// 只转换一次存入 `immutables_cache[cache_idx]`，并把 `active_immutables` 指向该 Vec。
    /// `constants` 由调用方传入（它已持有 `&module.constants`）。
    pub(crate) fn activate_immutables(&mut self, cache_idx: usize, constants: &[Constant]) {
        // 用裸指针访问缓存槽，避免 get_or_init（借用 immutables_cache）与 &self 的
        // convert_immutables 闭包和随后对 active_immutables 的写入发生借用冲突。
        // 成立前提：immutables_cache 归 VM 所有、只读、本次运行稳定。
        let slot: *const OnceLock<Vec<JsValue>> = &self.immutables_cache[cache_idx];
        let vec = unsafe { &*slot }.get_or_init(|| self.convert_immutables(constants));
        self.active_immutables = vec.as_slice() as *const [JsValue];
    }

    /// 当前活动字节码的可变访问入口。bytecode 以 `Arc<[Instr]>` 与 sub_modules 源共享，
    /// IC 写回经 `Arc::make_mut` 保证独占：独占时零拷贝原地写，共享时先深拷贝再写
    /// （IC miss 才触发，频率低）。所有写操作必须经此方法，防止共享缓冲被多实例污染。
    pub(crate) fn bytecode_mut(&mut self) -> &mut [opcode::Instr] {
        Arc::make_mut(&mut self.bytecode)
    }

    /// GC 根收集的统一遍历（对象与字符串都产出）。与 `rewrite_values` 字段一一对应。
    /// 覆盖执行核心的全部 JsValue 持有点：regs/帧/各栈段/cell/在途异常与完成/
    /// 挂起信号/迭代器/微任务/global。新增执行字段必须同时登记在此与
    /// `rewrite_values`。
    pub(crate) fn for_each_value(&self, mut f: impl FnMut(JsValue)) {
        for value in &self.regs {
            f(*value);
        }
        for frame in &self.frames {
            f(frame.saved_this);
            f(frame.saved_new_target);
            f(frame.callee);
            f(frame.constructed_this.unwrap_or(JsValue::undefined()));
        }
        for &v in &self.save_stack {
            f(v);
        }
        // spill 栈是 session GC 根（漏根 → 溢出值被回收 → use-after-free）。
        for &v in &self.spill_stack {
            f(v);
        }
        for cell_vec in &self.cell_stack {
            for &cell_ptr in cell_vec {
                if cell_ptr.is_null() {
                    continue;
                }
                // SAFETY: cell 由 session_epoch 分配，本 session 内指针有效。
                f(unsafe { &*cell_ptr }.value);
            }
        }
        f(self.exception_value.unwrap_or(JsValue::undefined()));
        f(self.pending_exception.unwrap_or(JsValue::undefined()));
        f(self.last_uncaught_value.unwrap_or(JsValue::undefined()));
        // 悬挂的 return 完成持有返回值，是 GC 根。
        if let Some(Completion::Return { value, .. }) = self.pending_completion {
            f(value);
        }
        f(self.generator_suspended.unwrap_or(JsValue::undefined()));
        f(self.delegated_iterator.unwrap_or(JsValue::undefined()));
        f(self.async_context.unwrap_or(JsValue::undefined()));
        f(self.async_gen_context.unwrap_or(JsValue::undefined()));
        f(self.inline_callee.unwrap_or(JsValue::undefined()));
        for &v in &self.iters.for_of_iters {
            f(v);
        }
        f(self.iters.last_for_of_result);
        // 微任务队列中的处理器/能力/值都是 GC 根。
        for job in &self.job_queue {
            crate::promise::for_each_job_value(job, &mut f);
        }
        for iter in &self.iters.for_in_iters {
            if iter.is_null() {
                continue;
            }
            // SAFETY: for_in_iters 存放由当前 VM epoch 拥有的存活迭代器指针。
            unsafe {
                for (v, _si) in (*(*iter)).keys.iter() {
                    f(*v);
                }
            }
        }
        f(JsValue::from_js_object(self.session.global_object().as_ptr() as *mut JsObject));
    }

    /// GC 指针重写（session 搬移后调用）。与 `for_each_value` 字段一一对应。
    pub(crate) fn rewrite_values(&mut self, mut rewrite: impl FnMut(JsValue) -> JsValue) {
        for value in &mut self.regs {
            *value = rewrite(*value);
        }
        for frame in &mut self.frames {
            frame.saved_this = rewrite(frame.saved_this);
            frame.saved_new_target = rewrite(frame.saved_new_target);
            frame.callee = rewrite(frame.callee);
            frame.constructed_this = frame.constructed_this.map(&mut rewrite);
        }
        for v in &mut self.save_stack {
            *v = rewrite(*v);
        }
        for v in &mut self.spill_stack {
            *v = rewrite(*v);
        }
        for cell_vec in &mut self.cell_stack {
            for &mut cell_ptr in cell_vec.iter_mut() {
                if cell_ptr.is_null() {
                    continue;
                }
                // SAFETY: cell 由 session_epoch 分配，本 session 内指针有效。
                let cell = unsafe { &mut *cell_ptr };
                cell.value = rewrite(cell.value);
            }
        }
        self.exception_value = self.exception_value.map(&mut rewrite);
        self.pending_exception = self.pending_exception.map(&mut rewrite);
        self.last_uncaught_value = self.last_uncaught_value.map(&mut rewrite);
        self.pending_completion = self.pending_completion.map(|completion| match completion {
            Completion::Return { value, remaining_finally } => Completion::Return {
                value: rewrite(value),
                remaining_finally,
            },
            other => other,
        });
        self.generator_suspended = self.generator_suspended.map(&mut rewrite);
        self.delegated_iterator = self.delegated_iterator.map(&mut rewrite);
        self.async_context = self.async_context.map(&mut rewrite);
        self.async_gen_context = self.async_gen_context.map(&mut rewrite);
        self.inline_callee = self.inline_callee.map(&mut rewrite);
        for v in &mut self.iters.for_of_iters {
            *v = rewrite(*v);
        }
        self.iters.last_for_of_result = rewrite(self.iters.last_for_of_result);
        // 微任务队列中的值随 sweep 重写。
        for job in &mut self.job_queue {
            crate::promise::rewrite_job_values(job, &mut rewrite);
        }
        for iter in &mut self.iters.for_in_iters {
            if iter.is_null() {
                continue;
            }
            // SAFETY: for_in_iters 存放由当前 VM epoch 拥有的存活迭代器指针。
            unsafe {
                for (v, _si) in (*(*iter)).keys.iter_mut() {
                    *v = rewrite(*v);
                }
            }
        }
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        if !global_ptr.is_null() {
            // SAFETY: KernelSession 在 VM 生命周期内拥有 global_object。
            unsafe {
                (*global_ptr).rewrite_object_values(rewrite);
            }
        }
    }

    pub(crate) fn for_each_root(&self, f: impl FnMut(JsValue)) {
        // 统一遍历：根收集与指针重写共用同一字段清单。
        self.for_each_value(f);
    }

    pub(crate) fn maybe_collect_session_gc(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.maybe_collect(self);
        self.gc_state.session_gc = session_gc;
    }

    /// 只读访问 session GC 的统计（回收次数、存活/死亡对象数、释放字节等）。
    pub fn session_gc_stats(&self) -> &SessionGc {
        &self.gc_state.session_gc
    }

    /// 当前 session arena 中存活（已晋升）的对象数量。
    pub fn session_object_count(&self) -> usize {
        self.gc_state.session_object_ptrs.len()
    }

    /// session 当前分配的字节数（对象 + 存活字符串）。
    pub fn session_bytes_allocated(&self) -> usize {
        self.gc_state.session_bytes_allocated
    }

    /// 当前 epoch 中已分配并跟踪的对象数量（未晋升到 session 的临时对象）。
    pub fn epoch_object_count(&self) -> usize {
        self.gc_state.epoch_object_ptrs.len()
    }

    /// inline cache 命中率（0.0~1.0），用于观测 IC 预热效果。
    pub fn ic_hit_rate(&self) -> f64 {
        self.profiling.ic_hit_rate()
    }

    /// 累计执行的指令数（profiling）。
    pub fn instruction_count(&self) -> u64 {
        self.profiling.instruction_count
    }

    /// 全局 symbol 注册表中已注册的 key 数量。
    pub fn symbol_registry_len(&self) -> usize {
        self.symbols.registry_len()
    }

    /// inline cache 命中次数。
    pub fn ic_hit_count(&self) -> u64 {
        self.profiling.ic_hits.get()
    }

    /// inline cache 未命中次数。
    pub fn ic_miss_count(&self) -> u64 {
        self.profiling.ic_misses.get()
    }

    pub(crate) fn checked_object_ptr(
        &mut self, val: JsValue, error_msg: &str,
    ) -> Result<Option<*mut JsObject>, String> {
        if !val.is_object() {
            self.raise_type_error(error_msg)?;
            return Ok(None);
        }
        let ptr = (val.to_bits() & PTR_MASK) as *mut JsObject;
        let addr = ptr as usize;
        if ptr.is_null() || addr < 0x10000 || addr % std::mem::align_of::<JsObject>() != 0 {
            self.raise_type_error(error_msg)?;
            return Ok(None);
        }
        Ok(Some(ptr))
    }

    pub(crate) fn raise_error_kind(&mut self, kind: &'static str, msg: &str) -> Result<(), String> {
        self.raise_js_error(JsError::new(js_error_kind(kind), msg))
    }

    pub(crate) fn raise_js_error(&mut self, err: JsError) -> Result<(), String> {
        let kind = js_error_kind_name(err.kind);
        vm_debug!("raise_js_error: {} \"{}\"", kind, err.message);
        let error = oxide_builtins::error::create_kind_error(self, kind, &err.message);
        self.exception_value = Some(error);
        self.pending_error_kind = Some(kind);
        self.unwind()
    }

    pub(crate) fn raise_type_error(&mut self, msg: &str) -> Result<(), String> {
        self.raise_error_kind("TypeError", msg)
    }

    pub(crate) fn error_message_text(&self, kind: &str, msg: &str) -> String {
        format_error_message(kind, msg)
    }

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
        Some(unsafe { (*val.as_string_ptr()).data.clone() })
    }

    pub(crate) fn thrown_error_kind(&self, val: JsValue) -> &'static str {
        if !val.is_object() {
            return "Error";
        }
        let name_si = self.kernel_core.perm_interner().intern("name").0;
        let obj = unsafe { &*val.as_js_object_ptr() };
        let Some(name_val) = self.resolve_property(obj, name_si) else {
            return "Error";
        };
        let Some(name) = self.lookup_str(name_val) else {
            return "Error";
        };
        match name.as_str() {
            "TypeError" => "TypeError",
            "ReferenceError" => "ReferenceError",
            "RangeError" => "RangeError",
            "SyntaxError" => "SyntaxError",
            "URIError" => "URIError",
            "EvalError" => "EvalError",
            "Error" => "Error",
            _ => "Error",
        }
    }

    /// 把 `JsValue` 转为属性键 si（`u32`）。
    ///
    /// 非负小整数（含整值 double）直接编码到整数键区间，免 to_string + intern；
    /// 字符串中形如数组下标的规范数字串（`"5"`）映射到同一整数键，保证
    /// `obj["5"]` 与 `obj[5]` 键等价。
    ///
    /// # 边界与前提
    /// - 索引超 `INT_KEY_COUNT`（2^30）时回退字符串键路径（intern + 反查仍正确）
    /// - 负数与小数不进入整数键区间（`arr[-1]`/`arr[1.5]` 是普通字符串键）
    pub(crate) fn property_key_si(&mut self, val: JsValue) -> Result<u32, String> {
        if val.is_int() {
            let i = val.as_int();
            if i >= 0 && (i as u32) < INT_KEY_COUNT {
                return Ok(make_int_key(i as u32));
            }
        } else if val.is_double() {
            let d = val.as_double();
            if d >= 0.0 && d.fract() == 0.0 && d < INT_KEY_COUNT as f64 {
                return Ok(make_int_key(d as u32));
            }
        } else if val.is_string() {
            // SAFETY: val 是字符串值，把其内容桥接为永久 key id。
            let s = unsafe { &(*val.as_string_ptr()).data };
            if let Some(i) = canonical_index_of(s) {
                return Ok(make_int_key(i));
            }
            return Ok(self.kernel_core.perm_interner().intern(s).0);
        }
        // Symbol 值直接编码为 Symbol 键（不进字符串 interner，键相互独立）。
        if val.is_symbol() {
            return Ok(make_symbol_key(val.as_symbol_index()));
        }
        // well-known symbol 是空对象：按指针比对映射到各自的 well-known Symbol 键，
        // 避免全部塌缩成同一个键。
        if val.is_object() {
            if let Some(id) = oxide_runtime_api::well_known_symbol_id(self, val.as_js_object_ptr()) {
                return Ok(make_well_known_symbol_key(id));
            }
            // ToPropertyKey：对象经 ToPrimitive(string hint)，结果为 Symbol 时直接作键。
            let prim = coercion::to_primitive(val, coercion::ToPrimitiveHint::String, self)?;
            if prim.is_symbol() {
                return Ok(make_symbol_key(prim.as_symbol_index()));
            }
            let key = coercion::to_string(prim);
            return Ok(self.kernel_core.perm_interner().intern(&key).0);
        }
        let key = coercion::to_string(val);
        Ok(self.kernel_core.perm_interner().intern(&key).0)
    }

    pub(crate) fn array_index_from_property_key(&self, prop_name_si: u32) -> Option<u32> {
        if is_int_key(prop_name_si) {
            return Some(int_key_value(prop_name_si));
        }
        let key = self.kernel_core.perm_interner().lookup(prop_name_si)?;
        if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
            return None;
        }
        key.parse::<u32>().ok()
    }

    pub(crate) fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        vm_trace!("resolve_property: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        if obj.is_array() && prop_name_si == length_si {
            return Some(obj.logical_len_value());
        }
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                // 数组元素区：hole（删除标记）视为不存在。
                if index < obj.array_prop_count && !obj.prop_meta_at(index).is_some_and(|m| m.is_hole()) {
                    return Some(obj.get_prop_at(index));
                }
            }
        }
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            let val = obj.get_prop_at(pos);
            if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                return Some(val);
            }
        }
        let mut proto = obj.proto();
        let mut depth = 0usize;
        while proto.is_object() && depth < MAX_PROTO_CHAIN_DEPTH {
            depth += 1;
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self
                .kernel_core
                .shape_forge()
                .lookup_position(proto_obj.shape_id(), prop_name_si)
            {
                let val = proto_obj.get_prop_at(pos);
                if !val.is_undefined() || proto_obj.prop_vec_len() > pos as usize {
                    return Some(val);
                }
            }
            proto = proto_obj.proto();
        }
        None
    }

    pub(crate) fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32> {
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        if obj.is_array() && prop_name_si == length_si {
            return None;
        }
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                // 数组元素区：hole（删除标记）视为不存在。
                if index < obj.array_prop_count && !obj.prop_meta_at(index).is_some_and(|m| m.is_hole()) {
                    return Some(index);
                }
            }
        }
        self.kernel_core
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
            .and_then(|pos| {
                if obj.is_array() {
                    // 数组属性存储索引 = array_prop_count + shape 槽位（与元素区分）。
                    let idx = obj.array_prop_count as usize + pos as usize;
                    let val = obj.get_prop_at(idx);
                    if !val.is_undefined() || obj.prop_vec_len() > idx {
                        Some(idx as u32)
                    } else {
                        None
                    }
                } else {
                    let val = obj.get_prop_at(pos);
                    if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                        Some(pos)
                    } else {
                        None
                    }
                }
            })
    }

    pub(crate) fn call_function_sync(
        &mut self, callee: JsValue, receiver: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        vm_debug!("call_function_sync: args={} callee_is_object={}", args.len(), callee.is_object());
        if !callee.is_object() {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        if !callee_obj.is_function() {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }

        if let Some(native_fn) = callee_obj.native_fn() {
            if self.native_call_depth >= self.kernel_core.config.max_call_depth {
                self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
                return Ok(JsValue::undefined());
            }
            if args.len() > Self::SYNC_NATIVE_ARG_LIMIT {
                self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
                return Ok(JsValue::undefined());
            }
            // native 回调只写 regs[0..args.len()] 实参区 + regs[253]/[254]（receiver/callee），
            // 窗口 = 调用方活动寄存器 ∪ 实参写入区；窗口外槽回调不触碰，无需保存。
            let window = (self.active_reg_limit as usize).max(args.len() + 3).min(253);
            let mut saved_window = self.inline_reg_pool.take().unwrap_or_default();
            saved_window.clear();
            saved_window.extend_from_slice(&self.regs[..window]);
            let saved_r253 = self.regs[253];
            let saved_r254 = self.regs[254];
            let arg_regs = self.pack_sync_native_call_args(receiver, callee, args);
            // SAFETY: native_fn 经 set_native_fn 以合法 NativeFn 指针设置；
            // native_fn_ptr_to_fn 是 NativeFnPtr → NativeFn 的唯一强制转换点。
            let func: NativeFn = unsafe { native_fn_ptr_to_fn(native_fn) };
            self.native_call_depth += 1;
            let result = func(self, &arg_regs);
            self.native_call_depth -= 1;
            // 窗口回拷 + regs[253]/[254] 单回，缓冲归还池复用。
            self.regs[..window].copy_from_slice(&saved_window);
            self.regs[253] = saved_r253;
            self.regs[254] = saved_r254;
            self.inline_reg_pool = Some(saved_window);
            return match result {
                NativeResult::Ok(val) => Ok(val),
                NativeResult::Err(err) => {
                    self.last_uncaught_value = Some(err);
                    Err(self.error_text(err))
                }
                NativeResult::TailCall { callee, this, args } => self.call_function_sync(callee, this, &args),
            };
        }

        // 字节码函数在自身（同一 epoch）内联执行，防止 use-after-free。
        // 独立子 VM 拥有不同 epoch：返回含子 VM epoch 指针的 JsValue 后再销毁子 VM，
        // 会在 release 构建中产生悬垂指针 / 访问违规。
        self.call_bytecode_function_inline(callee, callee_obj, receiver, args)
    }

    #[expect(clippy::too_many_arguments)]
    pub(crate) fn push_bytecode_frame(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue], construct_result_reg: Option<u8>,
        constructed_this: Option<JsValue>, new_target: JsValue, continuation: FrameContinuation,
    ) -> Result<(), String> {
        vm_trace!(
            "push_bytecode_frame: depth={}, args={}, continuation={:?}",
            self.frames.len(),
            args.len(),
            continuation
        );
        if !callee.is_object() {
            return Err(self.error_message_text("TypeError", "CALL target is not callable"));
        }
        let obj = unsafe { &*callee.as_js_object_ptr() };
        if !obj.is_function() || obj.sub_module_index() == 0 {
            return Err(self.error_message_text("TypeError", "CALL target is not callable"));
        }
        let sub_idx = obj.sub_module_index() as usize;
        if sub_idx >= self.sub_modules.len() {
            return Err(format!(
                "CALL: sub_module_index {} out of bounds (max {})",
                sub_idx,
                self.sub_modules.len()
            ));
        }
        if self.frames.len() >= self.kernel_core.config.max_call_depth {
            return self.raise_error_kind("RangeError", "Maximum call stack size exceeded");
        }

        let sub_bytecode = Arc::clone(&self.sub_modules[sub_idx].bytecode);
        let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
        let sub_n_registers = self.sub_modules[sub_idx].n_registers;
        let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
        let sub_is_arrow = self.sub_modules[sub_idx].is_arrow;
        let caller_reg_limit = self.active_reg_limit.max(1);
        let saved_reg_offset = self.save_stack.len() as u32;
        self.save_stack.extend_from_slice(&self.regs[..caller_reg_limit as usize]);
        let saved_this = self.regs[254];
        let saved_new_target = self.regs[255];

        for i in 0..sub_n_args {
            self.regs[sub_param_base + i] = args.get(i).copied().unwrap_or(JsValue::undefined());
        }
        self.regs[254] = if sub_is_arrow { obj.captured_this() } else { this_value };
        self.regs[255] = new_target;

        self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
        self.saved_immutables_stack.push(self.active_immutables);

        let function_name = self.sub_modules[sub_idx]
            .function_name
            .as_deref()
            .map(|name| self.kernel_core.perm_interner().intern(name).0)
            .unwrap_or(0);

        // 完整实参写入 spill 栈实参区（在帧的 spill 区之前）：CREATE_ARGUMENTS 据此
        // 构建 arguments 对象，帧恢复时随 spill 区截断一起丢弃。
        let args_base = self.spill_stack.len() as u32;
        self.spill_stack.extend_from_slice(args);
        let args_count = args.len().min(u16::MAX as usize) as u16;

        self.frames.push(CallFrame {
            return_addr: self.pc,
            function_name,
            caller_reg_limit,
            saved_reg_offset,
            spill_offset: self.spill_stack.len() as u32,
            arguments_base: args_base,
            arguments_count: args_count,
            saved_this,
            saved_new_target,
            callee,
            construct_result_reg,
            constructed_this,
            is_derived_constructor: obj.is_derived_constructor(),
            continuation,
        });

        self.pc = 0;
        self.bytecode = sub_bytecode;
        let subs = Arc::clone(&self.sub_modules);
        self.activate_immutables(sub_idx, &subs[sub_idx].constants);
        self.cell_stack.push(Vec::with_capacity(subs[sub_idx].cells_needed as usize));
        for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map.clone() {
            let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
            let global = self.session.global_object();
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            } else {
                vm_debug!(
                    "push_frame: builtin '{}' reg={} NOT on global object (stays {})",
                    name,
                    reg,
                    self.regs[*reg as usize]
                );
            }
        }

        self.active_reg_limit = sub_n_registers.max(1);
        self.pc = 0;
        Ok(())
    }

    pub(crate) fn dispatch(&mut self) -> Result<JsValue, String> {
        // config.max_steps 逐指令只读且循环内不变：提到循环外，免每次经 kernel_core
        // Arc 指针追寻读取（热点内唯一的 config 访问）。
        let max_steps = self.kernel_core.config.max_steps;
        let mut steps: u64 = 0;
        loop {
            steps += 1;
            if let Some(max_steps) = max_steps {
                if steps > max_steps {
                    vm_warn!("dispatch: step limit {} exceeded at pc={}", max_steps, self.pc);
                    self.profiling.set_instruction_count(steps);
                    return Err(format!("VM step limit exceeded at pc={}", self.pc));
                }
            }
            if self.pc >= self.bytecode.len() {
                let tail: Vec<String> = self
                    .bytecode
                    .iter()
                    .enumerate()
                    .rev()
                    .take(5)
                    .map(|(i, &instr)| format!("{i}:{:?}", opcode::opcode(instr)))
                    .collect();
                let fn_names: Vec<String> = self
                    .frames
                    .iter()
                    .map(|f| {
                        self.kernel_core
                            .perm_interner()
                            .lookup(f.function_name)
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "?".into())
                    })
                    .collect();
                vm_error!(
                    "dispatch: program counter out of bounds pc={} len={} frames={} cell_stack={} tail={:?} fns={:?}",
                    self.pc,
                    self.bytecode.len(),
                    self.frames.len(),
                    self.cell_stack.len(),
                    tail,
                    fn_names
                );
                vm_error!("dispatch: program counter out of bounds pc={} len={}", self.pc, self.bytecode.len());
                self.profiling.instruction_count = steps;
                return Err("program counter out of bounds".into());
            }

            let instr = self.bytecode[self.pc];
            let op = opcode::opcode(instr);
            let rd = opcode::rd(instr) as usize;
            let a = opcode::a(instr) as usize;
            let b = opcode::b(instr) as usize;
            self.pc += 1;

            match op {
                OpCode::NOP => {}

                OpCode::HALT => {
                    vm_trace!("HALT: regs[0]={:?}", self.regs[0]);
                    self.profiling.set_instruction_count(steps);
                    return Ok(self.regs[0]);
                }

                OpCode::LOAD_CONST => {
                    self.dispatch_load_const(rd, instr)?;
                }

                OpCode::CREATE_CLOSURE => {
                    self.dispatch_create_closure(rd, instr)?;
                }
                OpCode::MAKE_CELL => {
                    self.dispatch_make_cell(rd, instr)?;
                }
                OpCode::MAKE_CELL_FRESH => {
                    self.dispatch_make_cell_fresh(rd, instr)?;
                }
                OpCode::CELL_GET => {
                    self.dispatch_cell_get(rd, a, b)?;
                }
                OpCode::CELL_SET => {
                    self.dispatch_cell_set(a, b)?;
                }
                OpCode::LOAD_UPVALUE => {
                    self.dispatch_load_upvalue(rd, instr)?;
                }
                OpCode::STORE_UPVALUE => {
                    self.dispatch_store_upvalue(rd, a, b)?;
                }
                OpCode::CREATE_REGEXP => match self.dispatch_create_regexp(rd, a, b) {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                },

                OpCode::ADD => {
                    self.dispatch_add(rd, a, b)?;
                }

                OpCode::SUB => {
                    binary_arith!(self, a, b, rd, -, false);
                }

                OpCode::MUL => {
                    binary_arith!(self, a, b, rd, *, false);
                }

                OpCode::DIV => {
                    binary_arith!(self, a, b, rd, /, true);
                }

                OpCode::MOD => {
                    binary_arith!(self, a, b, rd, %, true);
                }

                OpCode::NEG => {
                    self.dispatch_neg(rd, a)?;
                }

                OpCode::BIT_AND => {
                    self.dispatch_bit_and(rd, a, b)?;
                }

                OpCode::BIT_OR => {
                    self.dispatch_bit_or(rd, a, b)?;
                }

                OpCode::BIT_XOR => {
                    self.dispatch_bit_xor(rd, a, b)?;
                }

                OpCode::SHL => {
                    self.dispatch_shl(rd, a, b)?;
                }

                OpCode::SHR => {
                    self.dispatch_shr(rd, a, b)?;
                }

                OpCode::USHR => {
                    self.dispatch_ushr(rd, a, b)?;
                }

                OpCode::BIT_NOT => {
                    self.dispatch_bit_not(rd, a)?;
                }

                OpCode::EQ => {
                    self.dispatch_eq(rd, a, b)?;
                }

                OpCode::NEQ => {
                    self.dispatch_neq(rd, a, b)?;
                }

                OpCode::LT => {
                    self.dispatch_lt(rd, a, b)?;
                }

                OpCode::GT => {
                    self.dispatch_gt(rd, a, b)?;
                }

                OpCode::LTE => {
                    self.dispatch_lte(rd, a, b)?;
                }

                OpCode::GTE => {
                    self.dispatch_gte(rd, a, b)?;
                }

                OpCode::STRICT_EQ => {
                    self.dispatch_strict_eq(rd, a, b);
                }

                OpCode::STRICT_NEQ => {
                    self.dispatch_strict_neq(rd, a, b);
                }

                OpCode::UNARY_PLUS => {
                    self.dispatch_unary_plus(rd, a)?;
                }

                OpCode::JMP => {
                    self.dispatch_jmp(instr);
                }

                OpCode::BREAK => {
                    self.dispatch_break(instr);
                }

                OpCode::CONTINUE => {
                    self.dispatch_continue(instr);
                }

                OpCode::JMP_IF_FALSE => {
                    self.dispatch_jmp_if_false(rd, instr);
                }

                OpCode::JMP_IF_TRUE => {
                    self.dispatch_jmp_if_true(rd, instr);
                }

                OpCode::JMP_IF_NULLISH => {
                    self.dispatch_jmp_if_nullish(rd, instr);
                }

                OpCode::LOAD_VAR => match self.dispatch_load_var(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::STORE_VAR => match self.dispatch_store_var(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::MOV => {
                    vm_trace!("MOV r{} = r{}", rd, a);
                    self.regs[rd] = self.regs[a];
                }

                OpCode::SPILL => {
                    self.dispatch_spill(rd)?;
                }

                OpCode::UNSPILL => {
                    self.dispatch_unspill(rd)?;
                }

                OpCode::CALL => match self.dispatch_call(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::CALL_NATIVE => {
                    self.dispatch_call_native(rd, a, b)?;
                }

                OpCode::CALL_SPREAD => match self.dispatch_call_spread(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::NEW_EXPRESSION => match self.dispatch_new_expression(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::NEW_EXPRESSION_SPREAD => match self.dispatch_new_expression_spread(rd, a, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::SUPER_CALL => match self.dispatch_super_call(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::SUPER_CALL_SPREAD => match self.dispatch_super_call_spread(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::SUPER_GET_PROP | OpCode::SUPER_STATIC_GET_PROP => {
                    match self.dispatch_super_get_prop(rd, a, b) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(e) => return Err(e),
                    }
                }

                OpCode::SET_HOME_OBJECT => match self.dispatch_set_home_object(rd, a) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::DEFINE_ACCESSOR => {
                    self.dispatch_define_accessor(rd, a, b)?;
                }

                OpCode::DEFINE_ACCESSOR_DYNAMIC => {
                    self.dispatch_define_accessor_dynamic(rd, a, b)?;
                }

                OpCode::DEFINE_PROP => {
                    self.dispatch_define_prop(rd, a, b)?;
                }

                OpCode::DEFINE_GLOBAL_PROP => {
                    self.dispatch_define_global_prop(rd, a, b)?;
                }

                OpCode::DEFINE_PROP_ATTRS => {
                    let attrs = self.bytecode[self.pc] as u8;
                    self.pc += 1;
                    self.dispatch_define_prop_attrs(rd, a, b, attrs)?;
                }

                OpCode::DEFINE_ACCESSOR_ATTRS => {
                    let key_word = self.bytecode[self.pc];
                    let attrs = self.bytecode[self.pc + 1] as u8;
                    self.pc += 2;
                    self.dispatch_define_accessor_attrs(rd, a, b, key_word, attrs)?;
                }

                OpCode::RETURN => match self.dispatch_return(rd) {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                },

                OpCode::IC_GET_PROP
                | OpCode::IC_SET_PROP
                | OpCode::GET_PROP
                | OpCode::SET_PROP
                | OpCode::SET_PROP_BATCH
                | OpCode::GET_PROP_DYNAMIC
                | OpCode::SET_PROP_DYNAMIC
                | OpCode::SET_ELEM
                | OpCode::GET_PRIVATE
                | OpCode::SET_PRIVATE
                | OpCode::INIT_PRIVATE
                | OpCode::PRIVATE_BRAND_IN => {
                    self.dispatch_property_op(op, rd, a, b)?;
                }

                OpCode::NEW_OBJECT => {
                    self.dispatch_new_object(rd, instr)?;
                }

                OpCode::CREATE_ARGUMENTS => {
                    self.dispatch_create_arguments(rd)?;
                }

                OpCode::CREATE_REST_ARRAY => {
                    let fixed_count = opcode::b(instr) as usize;
                    self.dispatch_create_rest_array(rd, fixed_count)?;
                }

                OpCode::NEW_ARRAY => {
                    self.dispatch_new_array(rd, instr);
                }

                OpCode::COMPOUND_ADD => {
                    self.dispatch_compound_add(rd, a)?;
                }

                OpCode::COMPOUND_SUB => {
                    self.dispatch_compound_sub(rd, a)?;
                }

                OpCode::COMPOUND_MUL => {
                    self.dispatch_compound_mul(rd, a)?;
                }

                OpCode::COMPOUND_DIV => {
                    self.dispatch_compound_div(rd, a)?;
                }

                OpCode::COMPOUND_MOD => {
                    self.dispatch_compound_mod(rd, a)?;
                }

                OpCode::COMPOUND_EXP => {
                    self.dispatch_compound_exp(rd, a)?;
                }

                OpCode::COMPOUND_AND => {
                    self.dispatch_compound_bit_and(rd, a)?;
                }

                OpCode::COMPOUND_OR => {
                    self.dispatch_compound_bit_or(rd, a)?;
                }

                OpCode::COMPOUND_XOR => {
                    self.dispatch_compound_bit_xor(rd, a)?;
                }

                OpCode::COMPOUND_SHL => {
                    self.dispatch_compound_shl(rd, a)?;
                }

                OpCode::COMPOUND_SHR => {
                    self.dispatch_compound_shr(rd, a)?;
                }

                OpCode::COMPOUND_USHR => {
                    self.dispatch_compound_ushr(rd, a)?;
                }

                OpCode::TYPEOF => {
                    self.dispatch_typeof(rd, a);
                }

                OpCode::TO_OBJECT => {
                    self.dispatch_to_object(rd)?;
                }

                OpCode::VOID => {
                    self.dispatch_void(rd);
                }

                OpCode::TEMPLATE_STR => {
                    self.dispatch_template_str(rd)?;
                }

                OpCode::DELETE_PROP_STATIC => match self.dispatch_delete_prop_static(rd) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::DELETE_PROP_DYNAMIC => match self.dispatch_delete_prop_dynamic(rd, b) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::INSTANCEOF => {
                    self.dispatch_instanceof(rd, a, b)?;
                }

                OpCode::IN => {
                    self.dispatch_in(rd, a, b)?;
                }

                OpCode::NOT => {
                    self.dispatch_not(rd, a);
                }

                OpCode::AND => {
                    self.dispatch_and(rd, a, b);
                }

                OpCode::OR => {
                    self.dispatch_or(rd, a, b);
                }

                OpCode::NULLISH => {
                    self.dispatch_nullish(rd, a, b);
                }

                OpCode::INC_PRE => {
                    self.dispatch_inc_pre(rd, a)?;
                }

                OpCode::INC_POST => {
                    self.dispatch_inc_post(rd, a)?;
                }

                OpCode::DEC_PRE => {
                    self.dispatch_dec_pre(rd, a)?;
                }

                OpCode::DEC_POST => {
                    self.dispatch_dec_post(rd, a)?;
                }

                OpCode::MEMBER_INC
                | OpCode::MEMBER_DEC
                | OpCode::DYN_MEMBER_INC
                | OpCode::DYN_MEMBER_DEC
                | OpCode::COMPOUND_MEMBER_ADD
                | OpCode::COMPOUND_MEMBER_SUB
                | OpCode::COMPOUND_MEMBER_MUL
                | OpCode::COMPOUND_MEMBER_DIV
                | OpCode::COMPOUND_MEMBER_MOD
                | OpCode::COMPOUND_MEMBER_EXP
                | OpCode::COMPOUND_MEMBER_BIT_AND
                | OpCode::COMPOUND_MEMBER_BIT_OR
                | OpCode::COMPOUND_MEMBER_BIT_XOR
                | OpCode::COMPOUND_MEMBER_SHL
                | OpCode::COMPOUND_MEMBER_SHR
                | OpCode::COMPOUND_MEMBER_USHR => {
                    self.dispatch_member_op(op, rd, a, b)?;
                }

                OpCode::FOR_IN_INIT => {
                    self.dispatch_for_in_init(a)?;
                }

                OpCode::FOR_IN_NEXT => {
                    self.dispatch_for_in_next(rd)?;
                }

                OpCode::FOR_IN_DONE => {
                    self.dispatch_for_in_done(rd);
                }

                OpCode::FOR_IN_CLEANUP => {
                    self.dispatch_for_in_cleanup();
                }

                OpCode::FOR_OF_INIT => {
                    self.dispatch_for_of_init(a)?;
                }

                OpCode::FOR_OF_NEXT => {
                    self.dispatch_for_of_next(rd)?;
                }

                OpCode::FOR_OF_DONE => {
                    self.dispatch_for_of_done(rd)?;
                }

                OpCode::FOR_OF_CLOSE => {
                    self.dispatch_for_of_close()?;
                }

                OpCode::FOR_AWAIT_OF_INIT => {
                    self.dispatch_for_await_of_init(a)?;
                }

                OpCode::FOR_AWAIT_OF_NEXT => {
                    self.dispatch_for_await_of_next(rd)?;
                }

                OpCode::FOR_AWAIT_OF_DONE => {
                    self.dispatch_for_await_of_done(rd, a)?;
                }

                OpCode::FOR_AWAIT_OF_CLOSE => {
                    self.dispatch_for_await_of_close()?;
                    // 异步 IteratorClose 可能经 await 挂起（return() 的 promise），
                    // 挂起时须像 AWAIT 一样让内嵌 dispatch 返回，由恢复方快照状态。
                    if self.async_suspended || self.async_gen_suspended {
                        self.profiling.set_instruction_count(steps);
                        return Ok(JsValue::undefined());
                    }
                }

                OpCode::REST_OBJECT => {
                    self.dispatch_rest_object(rd, a, b)?;
                }

                OpCode::SPREAD_OBJECT => {
                    self.dispatch_spread_object(rd, a)?;
                }

                OpCode::THROW => match self.dispatch_throw(rd) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::TRY_BEGIN => {
                    self.dispatch_try_begin(instr);
                }

                OpCode::TRY_END => {
                    self.dispatch_try_end();
                }

                OpCode::TRY_FINALLY_BEGIN => {
                    self.dispatch_try_finally_begin(instr);
                }

                OpCode::TRY_FINALLY_END => match self.dispatch_try_finally_end() {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(e) => return Err(e),
                },

                OpCode::YIELD => {
                    // 生成器让出：把让出值存入信号，内嵌 dispatch 返回，恢复方快照挂起状态。
                    let value = self.regs[rd];
                    self.generator_suspended = Some(value);
                    self.profiling.set_instruction_count(steps);
                    return Ok(JsValue::undefined());
                }

                OpCode::YIELD_STAR => {
                    // `yield*` 委托：取内层迭代器并推进一步。
                    // 未 done → 挂起让出（存委托迭代器）；done → 委托完成值写 reg 0 继续外层；
                    // unwind 捕获到异常 → 继续 dispatch（已展开到 catch/finally）。
                    match self.dispatch_yield_star(rd)? {
                        crate::generator::YieldStarOutcome::Suspend(value) => {
                            self.generator_suspended = Some(value);
                            self.profiling.set_instruction_count(steps);
                            return Ok(JsValue::undefined());
                        }
                        crate::generator::YieldStarOutcome::Continue(value) => {
                            self.regs[0] = value;
                        }
                        crate::generator::YieldStarOutcome::Unwind => {}
                    }
                }

                OpCode::SUSPEND_BODY => {
                    // 生成器调用时参数初始化结束：挂起在 body 起点；正常 next() 恢复时直接穿过。
                    if self.generator_init_step {
                        self.generator_body_started = true;
                        self.profiling.set_instruction_count(steps);
                        return Ok(JsValue::undefined());
                    }
                }

                OpCode::AWAIT => {
                    // 异步帧挂起：登记恢复反应后内嵌 dispatch 返回，恢复方快照挂起状态。
                    self.dispatch_await(rd)?;
                    self.profiling.set_instruction_count(steps);
                    return Ok(JsValue::undefined());
                }

                _ => {
                    return Err(format!("opcode {op} not yet implemented"));
                }
            }
        }
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl oxide_runtime_api::VmHost for Vm {
    fn reg(&self, idx: u8) -> JsValue {
        self.reg(idx)
    }
    fn set_reg(&mut self, idx: u8, val: JsValue) {
        self.set_reg(idx, val);
    }
    fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject {
        self.alloc_object(obj)
    }
    fn new_string(&mut self, s: &str) -> JsValue {
        self.new_string(s)
    }
    fn new_string_owned(&mut self, s: String) -> JsValue {
        Vm::new_string_owned(self, s)
    }
    fn string_ref(&self, val: JsValue) -> &str {
        // SAFETY: 调用方保证 val 为字符串值。perm 串由内核持有永不释放；session
        // 串仅经 &mut self 路径（new_string/new_string_owned/GC）释放，此处 &self
        // 借用期间编译器强制不存在 &mut 存续，字符串不会在借用期内回收。
        unsafe { (*val.as_string_ptr()).as_str() }
    }
    fn new_bigint(&mut self, v: num_bigint::BigInt) -> JsValue {
        Vm::new_bigint(self, v)
    }
    fn bigint_value(&mut self, val: JsValue) -> &num_bigint::BigInt {
        Vm::bigint_value(self, val)
    }
    fn kernel_core(&self) -> &Arc<KernelCore> {
        self.kernel_core()
    }
    fn session(&self) -> &KernelSession {
        self.session()
    }
    fn epoch(&self) -> &Epoch {
        self.epoch()
    }
    fn take_uncaught_value(&mut self) -> Option<JsValue> {
        self.last_uncaught_value.take()
    }
    fn property_key_si(&mut self, val: JsValue) -> u32 {
        // Object-key conversion (to_string_full) failures degrade to the empty key here:
        // this trait path is used by Reflect/Object builtins; computed property access
        // uses the inherent Result-returning version to preserve the full exception.
        self.property_key_si(val)
            .unwrap_or_else(|_| self.kernel_core.perm_interner().intern("").0)
    }
    fn string_key_si(&mut self, s: &str) -> u32 {
        if let Some(i) = canonical_index_of(s) {
            make_int_key(i)
        } else {
            self.kernel_core.perm_interner().intern(s).0
        }
    }
    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        self.resolve_property(obj, prop_name_si)
    }
    fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32> {
        self.get_own_property_slot(obj, prop_name_si)
    }
    fn ordinary_get(&mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue) -> Result<JsValue, String> {
        self.ordinary_get(obj, prop_name_si, receiver)
    }
    fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        self.ordinary_set(obj, prop_name_si, val, receiver)
    }
    fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        self.define_data_property(obj, prop_name_si, val, attributes)
    }
    fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        self.define_accessor_property(obj, prop_name_si, get, set, attributes)
    }
    fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue) {
        self.set_or_create_prop_value(obj, prop_name_si, val)
    }
    fn lookup_str(&self, val: JsValue) -> Option<String> {
        self.lookup_str(val)
    }
    fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String> {
        self.coerce_primitive_bounded(value, prefer_string)
    }
    fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String> {
        self.coerce_number_bounded(value)
    }
    fn call_function_sync(&mut self, callee: JsValue, receiver: JsValue, args: &[JsValue]) -> Result<JsValue, String> {
        self.call_function_sync(callee, receiver, args)
    }
    fn checked_object_ptr(&mut self, val: JsValue, error_msg: &str) -> Result<Option<*mut JsObject>, String> {
        self.checked_object_ptr(val, error_msg)
    }
    fn raise_type_error(&mut self, msg: &str) -> Result<(), String> {
        self.raise_type_error(msg)
    }
    fn error_message_text(&self, kind: &str, msg: &str) -> String {
        self.error_message_text(kind, msg)
    }
    fn call_stack_function_names(&self) -> Vec<String> {
        self.frames
            .iter()
            .rev()
            .map(|f| {
                self.kernel_core
                    .perm_interner()
                    .lookup(f.function_name)
                    .unwrap_or("<anonymous>")
                    .to_string()
            })
            .collect()
    }
    fn promote_if_needed_for_write_ptr(&mut self, target_ptr: *mut JsObject, value: JsValue) -> JsValue {
        self.promote_if_needed_for_write_ptr(target_ptr, value)
    }
    fn step_rng(&mut self) {
        self.step_rng()
    }
    fn math_rng_value(&self) -> f64 {
        self.math_rng_value()
    }
    fn sub_module_function_name(&self, sub_idx: u16) -> String {
        self.sub_modules
            .get(sub_idx as usize)
            .and_then(|m| m.function_name.clone())
            .unwrap_or_default()
    }
    fn create_dynamic_function(&mut self, params: &[String], body: &str) -> Result<JsValue, String> {
        self.create_dynamic_function(params, body)
    }
    fn symbol_intern(&mut self, desc: Option<String>) -> u32 {
        self.symbols.intern(desc)
    }
    fn symbol_description(&self, idx: u32) -> Option<&str> {
        self.symbols.description(idx)
    }
    fn symbol_lookup_global(&self, key: &str) -> Option<u32> {
        self.symbols.lookup_global(key)
    }
    fn symbol_register_global(&mut self, key: String, idx: u32) {
        self.symbols.register_global(key, idx)
    }
    fn symbol_key_for_id(&self, idx: u32) -> Option<String> {
        self.symbols.key_for_id(idx)
    }
}

impl Vm {
    /// 动态编译函数（`Function` 构造器路径）：把参数列表与函数体 wrap 成匿名函数
    /// 源码，走完整编译链后取匿名函数模块追加进 VM 平表，返回对应函数对象。
    ///
    /// # 步骤
    /// 1. wrap 源码 `function anonymous(p...) { body }`，parse + compile。
    /// 2. 取 `sub_modules[0]`（匿名函数模块），把其子树 flat_id 重编号到平表末尾
    ///    并重写子树内每条 `CREATE_CLOSURE` 的 imm16。
    /// 3. 同步 resize `immutables_cache`，建函数对象并设置 name/length。
    ///
    /// # 边界与前提
    /// - 编译或解析失败返回 `Err`（由 builtin 层转 SyntaxError）。
    /// - 追加的子树原 flat_id 自 1 连续（flatten 后 1=匿名体，2…=其嵌套函数）；
    ///   新 id = 平表长度 + (old - 1)。
    /// - 动态函数只在本次 `run()` 内有效：下次 run 重建平表，跨 run 引用会越界。
    ///
    /// # 副作用
    /// - 修改 `self.sub_modules` 与 `self.immutables_cache`。
    pub fn create_dynamic_function(&mut self, params: &[String], body: &str) -> Result<JsValue, String> {
        // wrap 源码：末尾换行防止 body 以行注释结尾吞掉右花括号。
        let params_str = params.join(", ");
        let source = format!("function anonymous({params_str}) {{\n{body}\n}}");

        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, &source)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))?;
        let mut module = oxide_compiler::compiler::Compiler::new().compile(&program)?;
        let anonymous = module.sub_modules.remove(0);
        // 形参数以编译结果为准：单个实参 "a,b,c" 拼接后解析为 3 个形参
        // （ES 动态函数把非末位实参以逗号连接成参数串再解析）。
        let formal_count = anonymous.n_args as i32;

        // 子树重编号 + 追加：base = 当前平表长度，DFS 前序压入，push 序即新 flat_id。
        let base = self.sub_modules.len() as u32;
        let mut added = Vec::new();
        rehome_subtree(&anonymous, base, &mut added);
        Arc::make_mut(&mut self.sub_modules).extend(added);
        // 平表变长后同步扩容常量缓存，否则激活新模块常量时越界 panic。
        self.immutables_cache
            .extend((0..self.sub_modules.len().saturating_sub(self.immutables_cache.len())).map(|_| OnceLock::new()));

        let func_val = self.create_function_object(base, false, false, false, false);
        let func_obj = unsafe { &mut *func_val.as_js_object_ptr() };
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        let name_si = self.kernel_core.perm_interner().intern("name").0;
        // length/name 为不可写不可枚举可配置，且 length 先于 name（规范属性顺序）。
        let attrs = PropAttributes::new(false, false, true);
        let length_val = JsValue::int(formal_count);
        let name_val = self.new_string("anonymous");
        self.define_data_property(func_obj, length_si, length_val, attrs)?;
        self.define_data_property(func_obj, name_si, name_val, attrs)?;
        Ok(func_val)
    }
}

/// 把 flatten 后的子模块子树重编号到平表偏移 `base`：DFS 前序拷贝进 `out`，
/// 新 flat_id = base + (old - 1)，子树内每条 `CREATE_CLOSURE` 的 imm16 同步重写。
/// 原子树 flat_id 自 1 连续，因此拷贝顺序即新 id 顺序，`out` 下标对齐平表槽位。
fn rehome_subtree(module: &CompiledModule, base: u32, out: &mut Vec<Arc<CompiledModule>>) {
    let new_id = base + module.flat_id - 1;
    let mut bytecode = module.bytecode.to_vec();
    for instr in &mut bytecode {
        if opcode::opcode(*instr) == OpCode::CREATE_CLOSURE {
            let old = opcode::imm16(*instr) as u32;
            let new_flat = base + (old - 1);
            *instr = opcode::encode(
                OpCode::CREATE_CLOSURE,
                opcode::rd(*instr),
                (new_flat & 0xFF) as u8,
                ((new_flat >> 8) & 0xFF) as u8,
            );
        }
    }
    let mut rehomed = module.clone();
    rehomed.bytecode = Arc::from(bytecode);
    rehomed.flat_id = new_id;
    out.push(Arc::new(rehomed));
    for sub in &module.sub_modules {
        rehome_subtree(sub, base, out);
    }
}

#[cfg(test)]
mod tests {
    use super::{opcode, JsValue, TryHandler, Vm};
    use oxide_bytecode::module::CompiledModule;
    use oxide_runtime_api::NativeResult;
    use oxide_types::object::NativeFnPtr;
    use oxide_types::object::{JsObject, PropAttributes};
    use std::sync::Arc;

    fn native_return_7(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
        NativeResult::Ok(JsValue::int(7))
    }

    fn native_get_marker(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let this_val = vm.reg(args[0]);
        if !this_val.is_object() {
            return NativeResult::Ok(JsValue::undefined());
        }
        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &*this_val.as_js_object_ptr() };
        NativeResult::Ok(vm.resolve_property(obj, marker_si).unwrap_or(JsValue::undefined()))
    }

    fn native_set_marker(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let this_val = vm.reg(args[0]);
        let value = vm.reg(args[1]);
        if !this_val.is_object() {
            return NativeResult::Ok(JsValue::undefined());
        }
        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &mut *this_val.as_js_object_ptr() };
        vm.set_or_create_prop_value(obj, marker_si, value);
        NativeResult::Ok(JsValue::undefined())
    }

    fn native_return_last_arg(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let reg = *args.last().expect("receiver + args");
        NativeResult::Ok(vm.reg(reg))
    }

    fn native_return_arg_count(_vm: &mut Vm, args: &[u8]) -> NativeResult {
        NativeResult::Ok(JsValue::int(args.len().saturating_sub(1) as i32))
    }

    fn native_function(vm: &mut Vm, f: crate::native::NativeFn) -> JsValue {
        let proto = vm.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let mut obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        obj.set_function(true);
        // SAFETY: f 是 NativeFn 函数项，可作为 NativeFnPtr 存储。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(f as *const ()) }));
        JsValue::object(vm.alloc_object(obj) as *mut u8)
    }

    fn plain_object(vm: &mut Vm) -> JsValue {
        let proto = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        JsValue::object(vm.alloc_object(obj) as *mut u8)
    }

    fn add_accessor(vm: &mut Vm, obj_val: JsValue, name: &str, get: JsValue, set: JsValue) {
        let si = vm.kernel_core.perm_interner().intern(name).0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        let shape_id = vm.kernel_core.shape_forge().make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape_id);
        let pos = obj.push_prop(JsValue::undefined());
        obj.set_accessor_meta(pos, get, set, PropAttributes::DEFAULT_DATA);
        obj.bump_generation();
    }

    fn set_data(vm: &mut Vm, obj_val: JsValue, name: &str, val: JsValue) {
        let si = vm.kernel_core.perm_interner().intern(name).0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        vm.set_or_create_prop_value(obj, si, val);
    }

    #[test]
    fn reset_clears_runtime_state_like_rerun() {
        let mut vm = Vm::new();
        vm.regs[1] = JsValue::int(7);
        vm.pc = 3;
        vm.frames.push(super::CallFrame {
            return_addr: 1,
            function_name: 0,
            caller_reg_limit: 2,
            saved_reg_offset: 0,
            spill_offset: 0,
            arguments_base: 0,
            arguments_count: 0,
            saved_this: JsValue::undefined(),
            saved_new_target: JsValue::undefined(),
            callee: JsValue::undefined(),
            construct_result_reg: None,
            constructed_this: None,
            is_derived_constructor: false,
            continuation: super::FrameContinuation::None,
        });
        vm.save_stack.push(JsValue::undefined());
        vm.iters
            .for_in_iters
            .push(std::ptr::dangling_mut::<super::ForInIter<'static>>());
        vm.iters.for_of_iters.push(JsValue::undefined());
        vm.saved_bytecode_stack
            .push(Arc::from(vec![opcode::encode(opcode::OpCode::HALT, 0, 0, 0)]));
        vm.saved_immutables_stack
            .push(std::ptr::slice_from_raw_parts(std::ptr::null(), 0));
        vm.try_stack.push(TryHandler {
            catch_pc: Some(1),
            finally_pc: None,
            finally_active: false,
            frame_depth: 0,
            for_of_depth: 0,
        });
        vm.exception_value = Some(JsValue::int(2));
        vm.pending_exception = Some(JsValue::int(3));
        vm.pending_error_kind = Some("TypeError");

        vm.reset();

        assert_eq!(vm.pc, 0);
        assert!(vm.frames.is_empty());
        assert!(vm.save_stack.is_empty());
        assert!(vm.iters.for_in_iters.is_empty());
        assert!(vm.iters.for_of_iters.is_empty());
        assert!(vm.saved_bytecode_stack.is_empty());
        assert!(vm.saved_immutables_stack.is_empty());
        assert!(vm.try_stack.is_empty());
        assert!(vm.exception_value.is_none());
        assert!(vm.pending_exception.is_none());
        assert!(vm.pending_error_kind.is_none());
        assert!(vm.bytecode.is_empty());
        assert!(vm.immutables().is_empty());
    }

    #[test]
    fn full_reset_clears_symbol_state() {
        let mut vm = Vm::new();
        vm.symbols.intern(Some("shared".to_string()));

        vm.full_reset();

        assert_eq!(vm.symbols.symbol_counter, 0);
        assert!(vm.symbols.symbol_descriptions.is_empty());
        assert!(vm.symbols.symbol_registry.is_empty());
    }

    #[test]
    fn for_of_close_pops_iterator_stack() {
        let module = CompiledModule {
            bytecode: Arc::from(vec![
                opcode::encode(opcode::OpCode::FOR_OF_CLOSE, 0, 0, 0),
                opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
            ]),
            n_registers: 1,
            ..CompiledModule::new()
        };
        let mut vm = Vm::new();
        vm.iters.for_of_iters.push(JsValue::undefined());

        vm.run(&module).expect("FOR_OF_CLOSE should tolerate non-object sentinel");

        assert!(vm.iters.for_of_iters.is_empty());
    }

    #[test]
    fn write_ic_back_updates_three_extension_words() {
        let mut vm = Vm::new();
        vm.bytecode = Arc::from(vec![0, 0, 0]);
        vm.pc = 3;
        crate::ic_helper::write_ic_back(Arc::make_mut(&mut vm.bytecode), vm.pc, 0x1234_5678, 7, 0);
        assert_eq!(vm.bytecode[0], 0x0034_5678);
        assert_eq!(vm.bytecode[1], 7);
        assert_eq!(vm.bytecode[2], 0);
    }

    #[test]
    fn write_ic_back_stores_proto_depth() {
        let mut vm = Vm::new();
        vm.bytecode = Arc::from(vec![0, 0, 0]);
        vm.pc = 3;
        crate::ic_helper::write_ic_back(Arc::make_mut(&mut vm.bytecode), vm.pc, 0xAAAA_BBBB, 42, 2);
        assert_eq!(vm.bytecode[0], 0x00AA_BBBB);
        assert_eq!(vm.bytecode[1], 42);
        assert_eq!(vm.bytecode[2], 2);
    }

    #[test]
    fn unimplemented_profile_opcode_fails_explicitly() {
        let module = CompiledModule {
            bytecode: Arc::from(vec![
                opcode::encode(opcode::OpCode::PROFILE_SHAPE, 0, 0, 0),
                opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
            ]),
            n_registers: 1,
            ..CompiledModule::new()
        };
        let mut vm = Vm::new();
        let err = vm.run(&module).expect_err("unimplemented opcode should fail explicitly");
        assert_eq!(err, "opcode PROFILE_SHAPE not yet implemented");
    }

    #[test]
    fn ordinary_get_calls_own_native_getter() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        let getter = native_function(&mut vm, native_return_7);
        add_accessor(&mut vm, obj_val, "x", getter, JsValue::undefined());

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &*obj_val.as_js_object_ptr() };
        let value = vm.ordinary_get(obj, x_si, obj_val).expect("getter");
        assert_eq!(value, JsValue::int(7));
    }

    #[test]
    fn ordinary_set_calls_own_native_setter() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, obj_val, "x", JsValue::undefined(), setter);

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        vm.ordinary_set(obj, x_si, JsValue::int(9), obj_val).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &*obj_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(obj, marker_si), Some(JsValue::int(9)));
    }

    #[test]
    fn inherited_getter_uses_original_receiver() {
        let mut vm = Vm::new();
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let getter = native_function(&mut vm, native_get_marker);
        add_accessor(&mut vm, proto_val, "x", getter, JsValue::undefined());
        set_data(&mut vm, child_val, "marker", JsValue::int(42));
        unsafe {
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        let value = vm.ordinary_get(child, x_si, child_val).expect("getter");
        assert_eq!(value, JsValue::int(42));
    }

    #[test]
    fn inherited_setter_uses_original_receiver() {
        let mut vm = Vm::new();
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, proto_val, "x", JsValue::undefined(), setter);
        unsafe {
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &mut *child_val.as_js_object_ptr() };
        vm.ordinary_set(child, x_si, JsValue::int(12), child_val).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        let proto = unsafe { &*proto_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(child, marker_si), Some(JsValue::int(12)));
        assert_eq!(vm.resolve_property(proto, marker_si), None);
    }

    #[test]
    fn deep_inherited_setter_uses_original_receiver() {
        let mut vm = Vm::new();
        let grand_proto_val = plain_object(&mut vm);
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, grand_proto_val, "x", JsValue::undefined(), setter);
        unsafe {
            (*proto_val.as_js_object_ptr()).set_proto(grand_proto_val).expect("proto");
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &mut *child_val.as_js_object_ptr() };
        vm.ordinary_set(child, x_si, JsValue::int(15), child_val).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(child, marker_si), Some(JsValue::int(15)));
    }

    #[test]
    fn ordinary_data_property_still_reads_and_writes_without_meta() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        set_data(&mut vm, obj_val, "x", JsValue::int(1));

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        assert!(!obj.has_prop_meta());
        assert_eq!(vm.ordinary_get(obj, x_si, obj_val).expect("get"), JsValue::int(1));
        vm.ordinary_set(obj, x_si, JsValue::int(2), obj_val).expect("set");
        assert_eq!(vm.ordinary_get(obj, x_si, obj_val).expect("get"), JsValue::int(2));
    }

    #[test]
    fn call_function_sync_passes_high_arity_native_args_without_truncation() {
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_last_arg);
        let args: Vec<JsValue> = (0..20).map(JsValue::int).collect();

        let result = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect("high-arity sync call should succeed");

        assert_eq!(result, JsValue::int(19));
    }

    #[test]
    fn call_function_sync_reports_actual_native_arg_count() {
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_arg_count);
        let args: Vec<JsValue> = (0..32).map(JsValue::int).collect();

        let result = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect("arg count should be preserved");

        assert_eq!(result, JsValue::int(32));
    }

    #[test]
    fn call_function_sync_rejects_unrepresentable_native_arity_without_register_corruption() {
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_arg_count);
        vm.set_reg(1, JsValue::int(7));
        vm.set_reg(253, JsValue::int(11));
        vm.set_reg(254, JsValue::int(12));
        vm.set_reg(255, JsValue::int(13));

        let args = vec![JsValue::undefined(); Vm::SYNC_NATIVE_ARG_LIMIT + 1];
        let err = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect_err("too many args should fail cleanly");

        assert!(err.contains("Maximum call stack size exceeded"), "unexpected error: {err}");
        assert_eq!(vm.reg(1), JsValue::int(7));
        assert_eq!(vm.reg(253), JsValue::int(11));
        assert_eq!(vm.reg(254), JsValue::int(12));
        assert_eq!(vm.reg(255), JsValue::int(13));
    }
}
