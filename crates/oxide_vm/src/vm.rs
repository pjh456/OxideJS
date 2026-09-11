#![allow(clippy::arc_with_non_send_sync)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

use num_traits::Zero;
use oxide_bytecode::module::{CompiledModule, Constant};
use oxide_bytecode::opcode::{self, OpCode};
use smallvec::SmallVec;

/// 初始化一个 session 的内置对象（global 槽位、各构造器与原型、IC 预热）。
pub use crate::bindings::init_kernel_builtins;
use crate::native::NativeFn;
use crate::session_gc::SessionGc;
use crate::vm_state::{ForOfEntry, GcState, IterState, ProfilingState, SymbolState};
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

/// 压帧实参来源：已物化切片或寄存器连续区间。
///
/// `RegRange` 直接引用调用方寄存器文件（CALL/NEW/SUPER_CALL 的实参在寄存器中
/// 天然连续排列），免去临时堆 `Vec<JsValue>` 物化，每次调用省 1 次分配/释放。
#[derive(Clone, Copy)]
pub(crate) enum FrameArgs<'a> {
    /// 已物化实参：spread 展开、TailCall、accessor 与初始执行路径。
    Slice(&'a [JsValue]),
    /// 寄存器连续区间 `regs[first .. first+count)`。
    RegRange { first: u8, count: usize },
}

impl FrameArgs<'_> {
    /// 实参总数。
    pub(crate) fn len(&self) -> usize {
        match self {
            FrameArgs::Slice(s) => s.len(),
            FrameArgs::RegRange { count, .. } => *count,
        }
    }
}

/// 一次函数调用的调用帧：记录返回地址、调用方寄存器窗口与 `this`/`new.target`。
///
/// 调用方寄存器窗口在 `save_stack` 中按 `saved_reg_offset` 保存，返回时由
/// `restore_frame` 恢复；`continuation` 描述 getter/setter 场景的恢复方式。
pub struct CallFrame {
    pub return_addr: usize,
    pub function_name: u32,
    /// 压帧保存的调用方寄存器窗口长度（= save_stack 中本帧段大小）。
    /// 普通字节码调用按调用点存活上界截断；运行时发起（accessor/内联）为
    /// 调用方 `active_reg_limit` 全量。
    pub caller_reg_limit: u8,
    /// 压帧时刻调用方 `active_reg_limit`（真实值）：帧恢复时据此还原，与
    /// `caller_reg_limit`（可能被存活上界截断的窗口）解耦。
    pub caller_active_reg_limit: u8,
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
    /// derived 构造帧是否已成功调用 super()（父构造器正常返回后置位）。
    ///
    /// 置位识别依赖 ABI 不变量：`construct_result_reg == Some(254)` 仅 SUPER_CALL
    /// 压帧产生（emit 从寄存器 ≥1 分配，254 保留给 this）；父构造器抛错走 unwind
    /// 弹帧不进 do_return，不置位。此标志取代依赖 regs[254] 值判定 super 状态的
    /// 旧方案——多层继承时中间 derived 帧由 SUPER_CALL 以构造 this 压入，
    /// regs[254] 恒非 undefined，值判定三重失效。
    pub super_called: bool,
    /// 本帧函数的严格模式标志（来源：模块编译产物 `is_strict`）。
    /// 属性写失败时据此分派：严格模式抛 TypeError，sloppy 静默 no-op。
    pub strict: bool,
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
/// `for_of_count`/`for_in_count` 为逃出时需关闭的迭代器层数：在全部 finally 穿越
/// 之后、跳转/返回之前执行（规范 §13.7.5.4 的 IteratorClose 在完成值之后）。
#[derive(Debug, Clone, Copy)]
pub enum Completion {
    Break {
        target_pc: usize,
        remaining_finally: usize,
        for_of_count: usize,
        for_in_count: usize,
    },
    Continue {
        target_pc: usize,
        remaining_finally: usize,
        for_of_count: usize,
        for_in_count: usize,
    },
    Return {
        value: JsValue,
        remaining_finally: usize,
        for_of_count: usize,
        for_in_count: usize,
    },
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

    /// 逃出时需关闭的 for-of 迭代器层数。
    pub fn for_of_count(&self) -> usize {
        match *self {
            Completion::Break { for_of_count, .. }
            | Completion::Continue { for_of_count, .. }
            | Completion::Return { for_of_count, .. } => for_of_count,
        }
    }

    /// 逃出时需弹出的 for-in 迭代器层数。
    pub fn for_in_count(&self) -> usize {
        match *self {
            Completion::Break { for_in_count, .. }
            | Completion::Continue { for_in_count, .. }
            | Completion::Return { for_in_count, .. } => for_in_count,
        }
    }

    /// 复制并改写剩余 finally 计数（进入一个 finally 后递减）。
    pub fn with_remaining(&self, remaining: usize) -> Completion {
        match *self {
            Completion::Break {
                target_pc,
                for_of_count,
                for_in_count,
                ..
            } => Completion::Break {
                target_pc,
                remaining_finally: remaining,
                for_of_count,
                for_in_count,
            },
            Completion::Continue {
                target_pc,
                for_of_count,
                for_in_count,
                ..
            } => Completion::Continue {
                target_pc,
                remaining_finally: remaining,
                for_of_count,
                for_in_count,
            },
            Completion::Return {
                value,
                for_of_count,
                for_in_count,
                ..
            } => Completion::Return {
                value,
                remaining_finally: remaining,
                for_of_count,
                for_in_count,
            },
        }
    }
}

/// `call_bytecode_function_inline` 使用的堆分配快照。
/// 放在堆上避免 JS 代码链式同步字节码调用（如 sort 比较器、accessor）时
/// 耗尽 Rust 栈。
pub(crate) struct InlineSyncState {
    /// 寄存器窗口副本：`regs[0..len]`，len ≤ 254（含 RegAlloc 最高合法物理槽
    /// 253）。`regs[254]/[255]` 不在此列，由 `saved_this`/`saved_new_target`
    /// 单独保存（callee 也会重写这两个槽）。
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
    pub(crate) for_of_iters: Vec<ForOfEntry>,
    pub(crate) saved_bytecode_stack: Vec<Arc<[opcode::Instr]>>,
    pub(crate) saved_immutables_stack: Vec<*const [JsValue]>,
    pub(crate) save_stack: Vec<JsValue>,
    pub(crate) spill_stack: Vec<JsValue>,
    pub(crate) cell_stack: Vec<Vec<*mut Cell>>,
    pub(crate) inline_callee: Option<JsValue>,
    /// inline 目标函数的严格模式标志（内联执行期间写路径的 strict/sloppy 判定
    /// 来源；嵌套内联时随本快照保存/恢复）。
    pub(crate) inline_strict: bool,
    /// inline 起始时 `frames.len()` 基线（嵌套内联时随本快照保存/恢复）。
    pub(crate) inline_frames_base: usize,
    pub(crate) inline_args_base: u32,
    pub(crate) inline_args_count: u16,
    pub(crate) accessor_frame_target_reg: Option<u8>,
    pub(crate) active_flat_id: u32,
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
    /// 当前是否处于构造器内嵌 dispatch 循环（`call_constructor_bytecode_inline`）：
    /// 构造帧弹出且 frames 清空时，`do_return` 据此把构造结果（regs[0]，
    /// 已做非对象回退 this）交付给恢复方（与 generator_dispatch 同语义）。
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
    /// 标签模板对象缓存（GetTemplateObject）：键 = (模块 flat_id, site 序号)。
    /// 同一编译树同 site 恒返回同一对象；每次 `run()` 清空（flat_id 复用防误命中）。
    /// 值为 GC 根（for_each_value/rewrite_values 遍历）。
    pub(crate) template_objects: HashMap<(u32, u32), JsValue>,
    /// 当前活动字节码所属模块的 flat_id（顶层 0；帧切换时随 bytecode 换）。
    /// GET_TEMPLATE_OBJECT 据其区分不同编译树（eval 每次编译独立 site）。
    pub(crate) active_flat_id: u32,
    /// 帧切换时暂存调用方 flat_id 的栈（与 saved_bytecode_stack 同步 push/pop）。
    pub(crate) saved_flat_id_stack: Vec<u32>,
}

impl Drop for Vm {
    fn drop(&mut self) {
        // 直接 drop（test262 每测试新建即弃）不经 reset/full_reset 路径：
        // 统一收尾释放全部 session 堆数据与内建原型属性区，防逐测试累积泄漏。
        self.teardown_intrinsic_protos();
        self.teardown_session_heap_data();
    }
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
        // immutables 缓存含 session BigInt（new_bigint 分配），未入根则被 sweep 释放
        // → 常量池加载时悬垂。perm 字符串无害（不在 session 集合中）。
        for once_lock in &self.immutables_cache {
            if let Some(immutable_vec) = once_lock.get() {
                for &value in immutable_vec.iter() {
                    f(value);
                }
            }
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
        // 标签模板对象缓存：命中的模板对象是 GC 根（未根 → sweep 搬移/回收悬垂）。
        for &cached in self.template_objects.values() {
            f(cached);
        }
        for &entry in &self.iters.for_of_iters {
            f(entry.iterator);
            f(entry.last_result);
        }
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
        for cached in self.template_objects.values_mut() {
            *cached = rewrite(*cached);
        }
        self.pending_completion = self.pending_completion.map(|completion| match completion {
            Completion::Return {
                value,
                remaining_finally,
                for_of_count,
                for_in_count,
            } => Completion::Return {
                value: rewrite(value),
                remaining_finally,
                for_of_count,
                for_in_count,
            },
            other => other,
        });
        self.generator_suspended = self.generator_suspended.map(&mut rewrite);
        self.delegated_iterator = self.delegated_iterator.map(&mut rewrite);
        self.async_context = self.async_context.map(&mut rewrite);
        self.async_gen_context = self.async_gen_context.map(&mut rewrite);
        self.inline_callee = self.inline_callee.map(&mut rewrite);
        for entry in &mut self.iters.for_of_iters {
            entry.iterator = rewrite(entry.iterator);
            entry.last_result = rewrite(entry.last_result);
        }
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

    /// 执行期字符串阈值回收：仅回收 session 字符串（跳过对象搬移）。热路径只在
    /// 超阈值后进入，`mem::take` 不承担每次分配的开销。
    pub(crate) fn maybe_collect_session_strings(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.maybe_collect_strings_only(self);
        self.gc_state.session_gc = session_gc;
    }

    /// 执行期完整 GC（对象+字符串）的 dispatch 安全点入口：按水位判定触发。
    /// 仅在 native_call_depth == 0 的指令边界调用——此时无 builtin 局部裸指针，
    /// 对象搬移安全。预留给 17.3b（对象侧执行期触发）安全点审计后使用。
    #[allow(dead_code)]
    pub(crate) fn maybe_collect_session_gc_at_dispatch(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.maybe_collect_gc(self);
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

    /// 执行期 session 堆账目的峰值高水位（顶层指令边界采样，全量重置清零）。
    pub fn session_bytes_peak(&self) -> usize {
        self.gc_state.session_bytes_peak
    }

    /// 本 run 累计分配字节：epoch arena + session 对象 arena + session 手工堆
    /// 账目（session 串 + session 对象及其属性向量 + GC 后补回的 BigInt）。
    /// 单次 run 内单调不减（执行期对象不回收、
    /// 串 GC 只降手工堆账目而 arena 不减）；run 边界（reset）后重新起算，
    /// 供单 run 分配上限判定。
    ///
    /// 注意：手工堆账目只在 promote/字符串分配/GC 回收点更新，执行期对象
    /// 属性区（元素/属性向量扩容）增长对其不可见——上限判定须配合
    /// [`Self::run_alloc_bytes_full`] 的深采样层。
    pub(crate) fn run_alloc_bytes(&self) -> usize {
        self.epoch.bump().allocated_bytes()
            + self.gc_state.session_epoch.allocated_bytes()
            + self.gc_state.session_bytes_allocated
    }

    /// 本 run 累计分配字节的全量重算版：base 只取两个 arena 计数器
    /// （epoch + session 对象 arena），手工堆逐一重算——已登记对象的堆数据
    /// （属性/元素向量 + upvalue 列表 + native 状态盒）、session 串、BigInt
    /// 与 upvalue cell 的容量。三分量（arena / 对象堆数据 / 串-BigInt-cell）
    /// 两两不相交，且不含 session 手工堆账目——无交叠不双计，重算即属性区
    /// 扩容等账目盲区的兜底。
    pub(crate) fn run_alloc_bytes_full(&self) -> u64 {
        let mut bytes = (self.epoch.bump().allocated_bytes() + self.gc_state.session_epoch.allocated_bytes()) as u64;
        for &ptr in self
            .gc_state
            .epoch_object_ptrs
            .iter()
            .chain(self.gc_state.session_object_ptrs.iter())
        {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自对象表登记，dispatch 安全点处仍有效。
            bytes += SessionGc::object_heap_data_bytes(unsafe { &*ptr });
        }
        for &ptr in &self.gc_state.session_string_ptrs {
            // SAFETY: ptr 来自字符串表登记，收尾前始终有效。
            bytes += (std::mem::size_of::<oxide_types::object::JsString>() + unsafe { (*ptr).len() }) as u64;
        }
        bytes += (self.gc_state.session_bigint_ptrs.borrow().len() * std::mem::size_of::<num_bigint::BigInt>()) as u64;
        bytes += (self.gc_state.session_cell_ptrs.borrow().len() * std::mem::size_of::<Cell>()) as u64;
        bytes
    }

    /// 无条件执行一次完整 session GC（mark + 移动式 sweep + 串/BigInt 清扫）。
    ///
    /// # 副作用
    /// - 存活对象复制进新 session arena，全部根与原生盒按转发表重写；
    ///   `session_bytes_allocated` 重置为清扫后的存活字节。
    ///
    /// # 注意事项
    /// - 须在执行外的安全点调用（无在途 builtin 局部裸指针、dispatch 未重入）；
    ///   执行期触发仍走水位路径，本入口供事后观测（如基准测 workload 后留存堆）。
    pub fn collect_session_gc(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.collect(self);
        self.gc_state.session_gc = session_gc;
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
        Some(unsafe { (*val.as_string_ptr()).to_owned_string() })
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
            // SAFETY: val 是字符串值，把其内容桥接为永久 key id（rope 经惰性扁平化）。
            let s = unsafe { (*val.as_string_ptr()).as_str() };
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
            // ToPropertyKey：对象经 ToPrimitive(string hint)，结果为 Symbol 时直接作键；
            // 其余字符串经规范化（规范数字串映射整数键）与字符串分支统一口径。
            let prim = coercion::to_primitive(val, coercion::ToPrimitiveHint::String, self)?;
            if prim.is_symbol() {
                return Ok(make_symbol_key(prim.as_symbol_index()));
            }
            let key = coercion::to_string(prim);
            return Ok(oxide_runtime_api::VmHost::string_key_si(self, &key));
        }
        // 其它原始值（BigInt 等）：ToPropertyKey 一律转字符串并走规范化，避免与
        // 数字键区间分裂（`o[5n]` 与 `o["5"]`/`o[5]` 必须同键）。
        let key = coercion::to_string(val);
        Ok(oxide_runtime_api::VmHost::string_key_si(self, &key))
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
        let length_si = self.length_si;
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
        let length_si = self.length_si;
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
            // 大实参集（超过寄存器窗口）：窗口外的实参转存 spill 栈溢出区（GC 根），
            // native 侧经 native_arg_count/native_arg_at 读取；窗口内仍按寄存器
            // 协议打包，保证未迁移的 builtin 行为不变。
            let overflow_base = self.spill_stack.len();
            let saved_overflow_base = self.native_overflow_base;
            let saved_overflow_count = self.native_overflow_count;
            if args.len() > Self::SYNC_NATIVE_ARG_LIMIT {
                self.spill_stack.extend_from_slice(&args[Self::SYNC_NATIVE_ARG_LIMIT..]);
                self.native_overflow_base = overflow_base;
                self.native_overflow_count = args.len() - Self::SYNC_NATIVE_ARG_LIMIT;
            }
            // native 回调只写 regs[0..args.len()] 实参区 + regs[253]/[254]（receiver/callee），
            // 窗口 = 调用方活动寄存器 ∪ 实参写入区；窗口外槽回调不触碰，无需保存。
            let window = (self.active_reg_limit as usize).max(args.len() + 3).min(253);
            let mut saved_window = self.inline_reg_pool.take().unwrap_or_default();
            saved_window.clear();
            saved_window.extend_from_slice(&self.regs[..window]);
            let saved_r253 = self.regs[253];
            let saved_r254 = self.regs[254];
            let pack_limit = args.len().min(Self::SYNC_NATIVE_ARG_LIMIT);
            let arg_regs = self.pack_sync_native_call_args(receiver, callee, &args[..pack_limit]);
            // SAFETY: native_fn 经 set_native_fn 以合法 NativeFn 指针设置；
            // native_fn_ptr_to_fn 是 NativeFnPtr → NativeFn 的唯一强制转换点。
            let func: NativeFn = unsafe { native_fn_ptr_to_fn(native_fn) };
            self.native_call_depth += 1;
            let result = func(self, &arg_regs);
            self.native_call_depth -= 1;
            // 溢出区随本次调用结束截断回收，并还原外层的溢出区描述（嵌套调用安全）。
            if args.len() > Self::SYNC_NATIVE_ARG_LIMIT {
                self.spill_stack.truncate(overflow_base);
            }
            self.native_overflow_base = saved_overflow_base;
            self.native_overflow_count = saved_overflow_count;
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

    /// 压帧窗口上界：调用方活动寄存器与调用点存活上界取 min。
    ///
    /// # 边界与前提
    /// - `call_window` 为 CALL ext 高 8 位编码的存活上界；0 = 未编码（回退全量）。
    /// - 窗口至少为 1（reg 0 恒为调用结果槽）。
    pub(crate) fn call_window_limit(&self, caller_active_reg_limit: u8, call_window: u8) -> u8 {
        if call_window == 0 {
            caller_active_reg_limit
        } else {
            caller_active_reg_limit.min(call_window)
        }
        .max(1)
    }

    /// 当前执行上下文的严格模式标志：属性写失败路径据此分派（严格抛
    /// TypeError，sloppy 静默 no-op）。
    ///
    /// # 边界与前提
    /// - inline 执行期间帧表可能非空（调用方帧隔离在外），须以
    ///   `inline_frames_base` 为基线区分"inline 前已有帧"与"inline 内新压帧"
    ///   （CALL/accessor 帧严格性归目标函数，最内层执行上下文即帧栈顶）。
    /// - 无帧且无 inline 时为顶层脚本执行，取 `top_level_strict`。
    pub(crate) fn current_strict(&self) -> bool {
        if self.inline_callee.is_some() {
            if self.frames.len() > self.inline_frames_base {
                return self.frames.last().unwrap().strict;
            }
            return self.inline_strict;
        }
        if let Some(frame) = self.frames.last() {
            return frame.strict;
        }
        self.top_level_strict
    }

    #[expect(clippy::too_many_arguments)]
    pub(crate) fn push_bytecode_frame(
        &mut self, callee: JsValue, this_value: JsValue, args: FrameArgs, construct_result_reg: Option<u8>,
        constructed_this: Option<JsValue>, new_target: JsValue, continuation: FrameContinuation, call_window: u8,
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
        let sub_is_strict = self.sub_modules[sub_idx].is_strict;
        // 窗口 = min(调用方活动寄存器, 存活上界)；call_window=0 表示调用方全量
        // （运行时发起路径 / 未编码的旧模块）。恢复按窗口回拷，active_reg_limit
        // 仍还原为调用方真实值（caller_active_reg_limit）。
        let caller_active_reg_limit = self.active_reg_limit.max(1);
        let caller_reg_limit = self.call_window_limit(caller_active_reg_limit, call_window);
        let saved_reg_offset = self.save_stack.len() as u32;
        self.save_stack.extend_from_slice(&self.regs[..caller_reg_limit as usize]);
        let saved_this = self.regs[254];
        let saved_new_target = self.regs[255];

        // 完整实参先写入 spill 栈实参区（在帧的 spill 区之前）：CREATE_ARGUMENTS 据此
        // 构建 arguments 对象，帧恢复时随 spill 区截断一起丢弃。spill 在前、形参在后，
        // 且形参源改读 spill 区——实参源区间与形参写入区在共享寄存器文件内重叠时
        // 不会先写后读串值（nested callee identity 高位 param_base 可落入实参区间）。
        let args_base = self.spill_stack.len() as u32;
        match args {
            FrameArgs::Slice(s) => self.spill_stack.extend_from_slice(s),
            FrameArgs::RegRange { first, count } => {
                for i in 0..count {
                    self.spill_stack.push(self.regs[first.wrapping_add(i as u8) as usize]);
                }
            }
        }
        let args_count = args.len().min(u16::MAX as usize) as u16;

        // 形参拷贝：源为 spill 实参区（与调用方寄存器隔离），实参不足补 undefined。
        for i in 0..sub_n_args {
            let v = if i < args_count as usize {
                self.spill_stack[args_base as usize + i]
            } else {
                JsValue::undefined()
            };
            self.regs[sub_param_base + i] = v;
        }
        // this 绑定：箭头函数恒用词法捕获；sloppy 普通函数 this 为 null/undefined 时
        // 替换为全局对象（ECMA-262 10.4.3）；严格模式与显式方法/构造 this 原样保留。
        self.regs[254] = if sub_is_arrow {
            obj.captured_this()
        } else if !sub_is_strict && this_value.is_nullish() {
            JsValue::from_js_object(self.session.global_object().as_ptr() as *mut JsObject)
        } else {
            this_value
        };
        self.regs[255] = new_target;

        self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
        self.saved_immutables_stack.push(self.active_immutables);
        // 记录调用方 flat_id，进入被调模块（标签模板 site 缓存按模块隔离）。
        self.saved_flat_id_stack.push(self.active_flat_id);
        self.active_flat_id = sub_idx as u32;

        let function_name = self.sub_modules[sub_idx]
            .function_name
            .as_deref()
            .map(|name| self.kernel_core.perm_interner().intern(name).0)
            .unwrap_or(0);

        self.frames.push(CallFrame {
            return_addr: self.pc,
            function_name,
            caller_reg_limit,
            caller_active_reg_limit,
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
            super_called: false,
            strict: sub_is_strict,
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
        // config.max_steps / max_alloc_bytes 逐指令只读且循环内不变：提到循环外，
        // 免每次经 kernel_core Arc 指针追寻读取（热点内仅有的 config 访问）。
        let max_steps = self.kernel_core.config.max_steps;
        let max_alloc_bytes = self.kernel_core.config.max_alloc_bytes;
        let mut steps: u64 = 0;
        loop {
            steps += 1;
            // 执行期 GC 安全点：仅在顶层 dispatch（native_call_depth == 0）
            // 的指令边界触发——嵌套 dispatch（builtin 经 call_function_sync 重入
            // 执行 JS 回调、generator/async 恢复）期间，调用方寄存器窗口副本存于
            // inline 状态（非 GC 根），此时回收会把调用方 regs 中的活值当死值释放。
            // 返回顶层后检查恢复，存活值此时已回拷为执行根。账目未超水位时仅
            // 少量字段比较。
            if self.native_call_depth == 0 {
                // 峰值高水位：同一边界采样 session 堆账目上界
                let bytes = self.gc_state.session_bytes_allocated;
                if bytes > self.gc_state.session_bytes_peak {
                    self.gc_state.session_bytes_peak = bytes;
                }
                if bytes >= self.gc_state.string_gc_watermark {
                    self.maybe_collect_session_strings();
                }
            }
            if let Some(max_steps) = max_steps {
                if steps > max_steps {
                    vm_warn!("dispatch: step limit {} exceeded at pc={}", max_steps, self.pc);
                    self.profiling.set_instruction_count(steps);
                    return Err(format!("VM step limit exceeded at pc={}", self.pc));
                }
            }
            // 单 run 分配上限：账目盲区（属性区扩容）靠两层采样兜住——轻层每
            // 64 指令读三个计数器，深层每 2^18 指令全量重算（含逐对象属性区
            // 重算）；超限 run 按步数超限同款处理（默认 skip / --no-skip 下
            // fail），防单测试 arena 高水位拖垮宿主。
            if let Some(cap) = max_alloc_bytes {
                let deep = (steps & 0x3FFFF) == 0;
                if deep || (steps & 0x3F) == 0 {
                    let used = if deep { self.run_alloc_bytes_full() as usize } else { self.run_alloc_bytes() };
                    if used > cap {
                        vm_warn!("dispatch: memory limit {cap} exceeded (used {used}) at pc={}", self.pc);
                        self.profiling.set_instruction_count(steps);
                        return Err(format!("VM memory limit {cap} exceeded (used {used}) at pc={}", self.pc));
                    }
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

                OpCode::LOAD_GLOBAL => match self.dispatch_load_global(rd, instr) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },

                OpCode::LOAD_GLOBAL_TYPEOF => {
                    self.dispatch_load_global_typeof(rd, instr)?;
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

                OpCode::CONCAT_N => {
                    self.dispatch_concat_n(rd, a)?;
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

                OpCode::EXP => {
                    self.dispatch_exp(rd, a, b)?;
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
                    self.dispatch_break(instr)?;
                }

                OpCode::CONTINUE => {
                    self.dispatch_continue(instr)?;
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

                OpCode::DEFINE_GLOBAL_PROP_C => {
                    let key_idx = self.bytecode[self.pc] as u16;
                    self.pc += 1;
                    self.dispatch_define_global_prop_c(a, key_idx)?;
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

                OpCode::RETURN => match self.dispatch_return(instr) {
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

                OpCode::GET_TEMPLATE_OBJECT => {
                    self.dispatch_get_template_object(rd)?;
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

                OpCode::TRY_FINALLY_ENTER => {
                    self.dispatch_try_finally_enter();
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
    fn native_overflow_count(&self) -> usize {
        self.native_overflow_count
    }
    fn native_overflow_at(&self, i: usize) -> JsValue {
        self.spill_stack[self.native_overflow_base + i]
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
    fn restore_uncaught_value(&mut self, value: Option<JsValue>) {
        self.last_uncaught_value = value;
    }
    fn property_key_si(&mut self, val: JsValue) -> u32 {
        // Object-key conversion (to_string_full) failures degrade to the empty key here:
        // this trait path is used by Reflect/Object builtins; computed property access
        // uses the inherent Result-returning version to preserve the full exception.
        self.property_key_si(val)
            .unwrap_or_else(|_| self.kernel_core.perm_interner().intern("").0)
    }
    fn to_property_key_si(&mut self, val: JsValue) -> Result<u32, String> {
        // ToPropertyKey 完整路径：转换异常（对象 ToPrimitive 抛错 / Symbol 处理）
        // 原样返回，供需传播异常的 builtins（groupBy / __defineGetter__ 等）使用。
        self.property_key_si(val)
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
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, strict: bool,
    ) -> Result<(), String> {
        self.ordinary_set(obj, prop_name_si, val, receiver, strict)
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
    fn create_dynamic_script(&mut self, code: &str) -> Result<JsValue, String> {
        self.create_dynamic_script(code)
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

    /// 动态编译脚本（eval 脚本模式）：把源码按脚本模式编译，var/函数声明落全局对象
    /// （属性 configurable:true，区别于普通脚本顶层的 false）。
    ///
    /// # 步骤
    /// 1. parse（脚本模式）→ compile（emit_program 置 is_global_scope=true，
    ///    Compiler 置 is_eval_script=true）。
    /// 2. 整棵模块树（根 flat_id=0 + 嵌套函数）追加进平表：`rehome_subtree(&module, base+1)`，
    ///    使根落 base、子函数 old→base+old，CREATE_CLOSURE imm16 同步重写。
    /// 3. 扩容 immutables_cache，建函数对象（sub_module_index = base）返回。
    ///
    /// # 边界与前提
    /// - 顶层 return 不报 SyntaxError（emit 无此检查，与 CLI 脚本路径一致）——已知偏差。
    /// - 动态模块只在本次 run() 内有效（同 create_dynamic_function）。
    /// - 返回函数对象仅供内部同步调用，不设 name/length（用户不可见）。
    ///
    /// # 副作用
    /// - 修改 `self.sub_modules` 与 `self.immutables_cache`。
    pub fn create_dynamic_script(&mut self, code: &str) -> Result<JsValue, String> {
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, code)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))?;
        // eval 脚本：顶层 var/function 声明落全局属性 configurable:true。
        let module = oxide_compiler::compiler::Compiler::new()
            .with_eval_script(true)
            .compile(&program)?;
        let base = self.sub_modules.len() as u32;
        let mut added = Vec::new();
        // 根模块 flat_id=0 传 base+1，重编号后落 base（避开 sub_module_index()==0 守卫）。
        rehome_subtree(&module, base + 1, &mut added);
        Arc::make_mut(&mut self.sub_modules).extend(added);
        // 平表变长后同步扩容常量缓存，否则激活新模块常量时越界 panic。
        self.immutables_cache
            .extend((0..self.sub_modules.len().saturating_sub(self.immutables_cache.len())).map(|_| OnceLock::new()));
        Ok(self.create_function_object(base, false, false, false, false))
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
    use super::{opcode, ForOfEntry, JsValue, TryHandler, Vm};
    use oxide_bytecode::module::CompiledModule;
    use oxide_runtime_api::{NativeResult, VmHost};
    use oxide_types::object::NativeFnPtr;
    use oxide_types::object::{JsObject, PropAttributes};
    use std::sync::{Arc, OnceLock};

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

    fn native_return_full_arg_count(vm: &mut Vm, args: &[u8]) -> NativeResult {
        NativeResult::Ok(JsValue::int(vm.native_arg_count(args) as i32))
    }

    fn native_return_last_full_arg(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let n = vm.native_arg_count(args);
        NativeResult::Ok(vm.native_arg_at(args, n - 1))
    }

    fn native_nested_inline_254(vm: &mut Vm, args: &[u8]) -> NativeResult {
        // 外层 native 回调：receiver 落 regs[253]（native 分支单存槽），内嵌执行
        // n_registers=254 的 inline 字节码回调后，receiver 槽必须保持外层值。
        let receiver = vm.reg(args[0]);
        let callee = vm.reg(args[1]);
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        let mut call_args = Vec::with_capacity(args.len().saturating_sub(2));
        for &r in &args[2..] {
            call_args.push(vm.reg(r));
        }
        let kept = match vm.call_bytecode_function_inline(callee, callee_obj, receiver, &call_args) {
            Ok(_) => vm.regs[253] == receiver,
            Err(_) => false,
        };
        NativeResult::Ok(JsValue::int(if kept { 1 } else { 0 }))
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
            caller_active_reg_limit: 2,
            saved_reg_offset: 0,
            spill_offset: 0,
            arguments_base: 0,
            arguments_count: 0,
            saved_this: JsValue::undefined(),
            saved_new_target: JsValue::undefined(),
            callee: JsValue::undefined(),
            construct_result_reg: None,
            strict: false,
            constructed_this: None,
            is_derived_constructor: false,
            super_called: false,
            continuation: super::FrameContinuation::None,
        });
        vm.save_stack.push(JsValue::undefined());
        vm.iters
            .for_in_iters
            .push(std::ptr::dangling_mut::<super::ForInIter<'static>>());
        vm.iters.for_of_iters.push(ForOfEntry {
            iterator: JsValue::undefined(),
            last_result: JsValue::undefined(),
            is_async: false,
        });
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
        vm.iters.for_of_iters.push(ForOfEntry {
            iterator: JsValue::undefined(),
            last_result: JsValue::undefined(),
            is_async: false,
        });

        vm.run(&module).expect("FOR_OF_CLOSE should tolerate non-object sentinel");

        assert!(vm.iters.for_of_iters.is_empty());
    }

    #[test]
    fn write_ic_back_updates_slot_zero_ext_words() {
        let mut vm = Vm::new();
        vm.bytecode = Arc::from(vec![0; oxide_bytecode::opcode::IC_EXT_WORDS]);
        vm.pc = oxide_bytecode::opcode::IC_EXT_WORDS;
        crate::ic_helper::write_ic_back(Arc::make_mut(&mut vm.bytecode), vm.pc, 0x1234_5678, 7, 0);
        assert_eq!(vm.bytecode[0], 0x0034_5678);
        assert_eq!(vm.bytecode[1], 7);
    }

    #[test]
    fn write_ic_back_rolls_fifo_and_drops_oldest_slot() {
        let mut vm = Vm::new();
        // 预置 8 字四槽（2 字/槽）：槽 0=(0xA1,1,0)、槽 1=(0xA2,2,1)、槽 2=(0xA3,3,0)、槽 3=(0xA4,4,0)。
        let mut bc = vec![0u32; oxide_bytecode::opcode::IC_EXT_WORDS];
        bc[0] = 0xA1;
        bc[1] = 1;
        bc[2] = 0xA2 | (1 << 24);
        bc[3] = 2;
        bc[4] = 0xA3;
        bc[5] = 3;
        bc[6] = 0xA4;
        bc[7] = 4;
        vm.bytecode = Arc::from(bc);
        vm.pc = oxide_bytecode::opcode::IC_EXT_WORDS;
        crate::ic_helper::write_ic_back(Arc::make_mut(&mut vm.bytecode), vm.pc, 0xB0, 9, 2);
        // 新条目进槽 0；原槽 0..2 顺移到槽 1..3；最老槽 3 丢弃。
        assert_eq!(vm.bytecode[0], 0xB0 | (2 << 24));
        assert_eq!(vm.bytecode[1], 9);
        assert_eq!(vm.bytecode[2], 0xA1);
        assert_eq!(vm.bytecode[3], 1);
        assert_eq!(vm.bytecode[4], 0xA2 | (1 << 24));
        assert_eq!(vm.bytecode[5], 2);
        assert_eq!(vm.bytecode[6], 0xA3);
        assert_eq!(vm.bytecode[7], 3);
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
        vm.ordinary_set(obj, x_si, JsValue::int(9), obj_val, true).expect("setter");

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
        vm.ordinary_set(child, x_si, JsValue::int(12), child_val, true).expect("setter");

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
        vm.ordinary_set(child, x_si, JsValue::int(15), child_val, true).expect("setter");

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
        vm.ordinary_set(obj, x_si, JsValue::int(2), obj_val, true).expect("set");
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
    fn call_function_sync_overflow_preserves_large_native_arity_and_registers() {
        // 大实参集（超寄存器窗口 253）经 spill 溢出区完整送达 native：
        // 全量计数与末位实参都可读，且调用方寄存器不被打包过程污染。
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_full_arg_count);
        vm.set_reg(1, JsValue::int(7));
        vm.set_reg(253, JsValue::int(11));
        vm.set_reg(254, JsValue::int(12));
        vm.set_reg(255, JsValue::int(13));

        let args: Vec<JsValue> = (0..(Vm::SYNC_NATIVE_ARG_LIMIT + 100)).map(|i| JsValue::int(i as i32)).collect();
        let count = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect("大实参集应经 spill 溢出区完整送达 native");
        assert_eq!(count, JsValue::int(args.len() as i32));

        let last_callee = native_function(&mut vm, native_return_last_full_arg);
        let last = vm
            .call_function_sync(last_callee, JsValue::undefined(), &args)
            .expect("溢出区末位实参应可读");
        assert_eq!(last, JsValue::int((args.len() - 1) as i32));

        assert_eq!(vm.reg(1), JsValue::int(7));
        assert_eq!(vm.reg(253), JsValue::int(11));
        assert_eq!(vm.reg(254), JsValue::int(12));
        assert_eq!(vm.reg(255), JsValue::int(13));
        // 溢出区随调用结束截断回收，不残留 spill 增长。
        assert!(vm.spill_stack.is_empty(), "溢出区应已回收: {:?}", vm.spill_stack);
        assert_eq!(vm.native_overflow_count, 0);
    }

    /// 构造 `n_registers = 254` 的子模块：RegAlloc 合法产物（builtin 槽落 253 或
    /// 高活度着色），其函数体写物理槽 253 后返回。
    fn sub_module_254_with_r253_write() -> CompiledModule {
        CompiledModule {
            bytecode: Arc::from(vec![
                opcode::encode(opcode::OpCode::MOV, 253, 0, 0),
                opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
            ]),
            n_registers: 254,
            ..CompiledModule::new()
        }
    }

    #[test]
    fn inline_callee_254_registers_preserves_caller_active_r253() {
        // 窗口化边界回归：inline 回调 callee 的 n_registers = 254（写物理槽 253）
        // 且调用方 active_reg_limit = 254（regs[253] 为活动值）时，调用后
        // regs[253] 必须恢复为调用方值，不得被 callee 写值覆盖。
        let mut vm = Vm::new();
        vm.sub_modules = Arc::new(vec![Arc::new(CompiledModule::new()), Arc::new(sub_module_254_with_r253_write())]);
        vm.immutables_cache
            .extend((0..vm.sub_modules.len().saturating_sub(vm.immutables_cache.len())).map(|_| OnceLock::new()));
        vm.active_reg_limit = 254;
        vm.regs[253] = JsValue::int(42);

        let callee = vm.create_function_object(1, false, false, false, false);
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        let result = vm
            .call_bytecode_function_inline(callee, callee_obj, JsValue::undefined(), &[])
            .expect("inline call should succeed");

        assert_eq!(result, JsValue::undefined());
        assert_eq!(vm.regs[253], JsValue::int(42), "调用方 regs[253] 活动值不得被 callee 覆盖");
        assert_eq!(vm.active_reg_limit, 254, "restore 应还原调用方活动寄存器上限");
    }

    #[test]
    fn native_callback_nested_254_register_inline_callee_keeps_receiver() {
        // 嵌套回归防线：native 回调体（receiver 落 regs[253]）内嵌 n_registers=254
        // 的 inline 字节码回调时，内层写 regs[253] 不得污染外层 receiver 槽。
        let mut vm = Vm::new();
        vm.sub_modules = Arc::new(vec![Arc::new(CompiledModule::new()), Arc::new(sub_module_254_with_r253_write())]);
        vm.immutables_cache
            .extend((0..vm.sub_modules.len().saturating_sub(vm.immutables_cache.len())).map(|_| OnceLock::new()));
        vm.active_reg_limit = 254;
        vm.regs[253] = JsValue::int(7);
        vm.regs[254] = JsValue::int(8);

        let inner_callee = vm.create_function_object(1, false, false, false, false);
        let outer_native = native_function(&mut vm, native_nested_inline_254);
        let result = vm
            .call_function_sync(outer_native, JsValue::int(99), &[inner_callee])
            .expect("native callback should succeed");

        assert_eq!(result, JsValue::int(1), "内嵌 inline 回调后外层 receiver 槽必须保持原值");
        assert_eq!(vm.regs[253], JsValue::int(7), "native 分支恢复后调用方 regs[253] 保持");
        assert_eq!(vm.regs[254], JsValue::int(8), "native 分支恢复后调用方 regs[254] 保持");
    }

    #[test]
    fn push_bytecode_frame_param_overlap_reads_spill_first() {
        // 实参源区间 regs[1..3) 与 callee 形参写入区 regs[2..4) 重叠（first < param_base）：
        // 压帧必须先拷 spill 实参区、形参再从 spill 区取源，避免边写形参边读实参
        // 造成先写后读串值（形参 b 与 spill 实参区都取错）。
        let mut vm = Vm::new();
        // sub_modules[1] = callee：2 个形参，param_base=2（与调用方实参槽 2 重叠）
        let mut callee_mod = CompiledModule::new();
        callee_mod.n_args = 2;
        callee_mod.param_base = 2;
        callee_mod.n_registers = 5;
        callee_mod.bytecode = Arc::from(vec![opcode::encode(opcode::OpCode::RETURN, 0, 0, 0)]);
        vm.sub_modules = Arc::new(vec![Arc::new(CompiledModule::new()), Arc::new(callee_mod)]);
        vm.immutables_cache
            .extend((0..vm.sub_modules.len().saturating_sub(vm.immutables_cache.len())).map(|_| OnceLock::new()));
        vm.active_reg_limit = 8;
        // 调用方实参区 regs[1..3)：arg0=10, arg1=20；regs[2] 同时是 callee 形参槽（param_base=2）
        vm.regs[1] = JsValue::int(10);
        vm.regs[2] = JsValue::int(20);

        let callee = vm.create_function_object(1, false, false, false, false);
        vm.push_bytecode_frame(
            callee,
            JsValue::undefined(),
            super::FrameArgs::RegRange { first: 1, count: 2 },
            None,
            None,
            JsValue::undefined(),
            super::FrameContinuation::None,
            0,
        )
        .expect("压帧成功");
        // 形参从 spill 实参区取源：regs[2]=arg0=10，regs[3]=arg1=20（不得被先写覆盖）
        assert_eq!(vm.regs[2], JsValue::int(10), "形参 a 应为实参 arg0");
        assert_eq!(vm.regs[3], JsValue::int(20), "形参 b 应为实参 arg1（不受先写覆盖）");
        // spill 实参区（arguments 对象源）保持原实参值
        let frame = vm.frames.last().expect("压帧后应有帧");
        let base = frame.arguments_base as usize;
        assert_eq!(vm.spill_stack[base], JsValue::int(10), "spill 实参区 arg0");
        assert_eq!(vm.spill_stack[base + 1], JsValue::int(20), "spill 实参区 arg1");
    }

    #[test]
    fn dispatch_new_expression_param_overlap_reads_spill_first() {
        // NEW 收敛路径同款重叠几何（实参源 regs[1..3) 与 callee 形参写入区 regs[2..4)
        // 重叠，first < param_base）：经 NEW_EXPRESSION 入口压帧，形参与 spill 实参区
        // （arguments 对象源）必须取原始实参值，不得先写后读串值。
        let mut vm = Vm::new();
        // sub_modules[1] = 构造器：2 个形参，param_base=2（与调用方实参槽 2 重叠）
        let mut ctor_mod = CompiledModule::new();
        ctor_mod.n_args = 2;
        ctor_mod.param_base = 2;
        ctor_mod.n_registers = 5;
        ctor_mod.bytecode = Arc::from(vec![opcode::encode(opcode::OpCode::RETURN, 0, 0, 0)]);
        vm.sub_modules = Arc::new(vec![Arc::new(CompiledModule::new()), Arc::new(ctor_mod)]);
        vm.immutables_cache
            .extend((0..vm.sub_modules.len().saturating_sub(vm.immutables_cache.len())).map(|_| OnceLock::new()));
        vm.active_reg_limit = 8;
        // NEW_EXPRESSION 指令：ext 低 8 位 = 实参个数 2，高 8 位 = 窗口 0（全量）
        vm.bytecode = Arc::from(vec![opcode::encode(opcode::OpCode::NEW_EXPRESSION, 0, 0, 0), 2]);
        vm.pc = 0;
        // 调用方实参区 regs[1..3)：arg0=10, arg1=20；regs[2] 同时是 callee 形参槽（param_base=2）
        vm.regs[1] = JsValue::int(10);
        vm.regs[2] = JsValue::int(20);
        vm.regs[5] = vm.create_function_object(1, false, false, false, false);

        vm.dispatch_new_expression(0, 5, 1).expect("NEW 压帧成功");

        let frame = vm.frames.last().expect("压帧后应有帧");
        // 形参从 spill 实参区取源：regs[2]=arg0=10，regs[3]=arg1=20（不得被先写覆盖）
        assert_eq!(vm.regs[2], JsValue::int(10), "形参 a 应为实参 arg0");
        assert_eq!(vm.regs[3], JsValue::int(20), "形参 b 应为实参 arg1（不受先写覆盖）");
        // spill 实参区（arguments 对象源）保持原实参值
        let base = frame.arguments_base as usize;
        assert_eq!(vm.spill_stack[base], JsValue::int(10), "spill 实参区 arg0");
        assert_eq!(vm.spill_stack[base + 1], JsValue::int(20), "spill 实参区 arg1");
        // NEW 帧契约：构造结果寄存器与 constructed_this 随帧携带
        assert_eq!(frame.construct_result_reg, Some(0), "构造结果写回 regs[0]");
        assert!(
            frame.constructed_this.is_some_and(|v| v.is_object()),
            "基类构造路径 constructed_this 应为新对象"
        );
        assert!(!frame.is_derived_constructor, "普通函数非 derived 构造器");
    }
}
