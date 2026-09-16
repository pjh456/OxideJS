//! 调用帧与完成值类型：帧续行方式、压帧实参来源、调用帧、for-in 迭代游标、
//! try/catch/finally 处理记录，以及 break/continue/return 完成值（含访问器）。

use oxide_types::value::JsValue;

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
