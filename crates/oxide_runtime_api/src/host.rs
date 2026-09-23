//! `VmHost` trait：builtins 依赖的 `Vm` 能力集合，trait 面向泛型而非对象安全。

use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::mem::Epoch;
use oxide_types::object::{Cell, JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// builtins 依赖的 `Vm` 能力集合。
///
/// 方法签名是 `Vm` 上同名固有方法的逐字节拷贝；`impl VmHost for Vm` 委托给
/// 它们。trait 刻意保持扁平且非对象安全——builtins 始终接受 `&mut impl VmHost`。
pub trait VmHost {
    // 寄存器访问
    fn reg(&self, idx: u8) -> JsValue;
    fn set_reg(&mut self, idx: u8, val: JsValue);

    /// 当前 native 调用 spill 溢出区的实参个数（寄存器窗口 253 之外的部分）。
    ///
    /// # 边界与前提
    /// - 仅在 native 实现内部、实参仍有效时读取；无溢出时为 0。
    fn native_overflow_count(&self) -> usize;
    /// 读 spill 溢出区第 `i` 个实参（0 基，相对溢出区起点）。
    ///
    /// # 边界与前提
    /// - `i` 必须小于 [`Self::native_overflow_count`]。
    fn native_overflow_at(&self, i: usize) -> JsValue;
    /// 当前 native 调用的完整实参个数（寄存器窗口 + spill 溢出区，不含 receiver）。
    ///
    /// 供支持大实参集的 builtin（如 `String.fromCodePoint`）遍历全部实参；
    /// 尚未改用本接口的 builtin 仍按 `args` 索引读寄存器。
    fn native_arg_count(&self, args: &[u8]) -> usize {
        args.len().saturating_sub(1) + self.native_overflow_count()
    }
    /// 第 `idx` 个实参（0 基，不含 receiver）：窗口内读寄存器，窗口外读 spill 溢出区。
    ///
    /// # 边界与前提
    /// - `idx` 必须小于 [`Self::native_arg_count`]。
    fn native_arg_at(&self, args: &[u8], idx: usize) -> JsValue {
        let window = args.len().saturating_sub(1);
        if idx < window {
            self.reg(args[idx + 1])
        } else {
            self.native_overflow_at(idx - window)
        }
    }
    /// 当前 native 调用是否以构造形态发起（NEW/SUPER native 臂与 `construct_with`
    /// native 臂置 true，普通调用入口置 false，调用结束即恢复）。
    ///
    /// # 边界与前提
    /// - 仅在 native 实现内部读取；构造器 builtin 据此判定向 receiver 物化，
    ///   不得用寄存器推断（native 调用与调用方共享寄存器文件，new.target
    ///   槽在类构造器帧内残留类构造器对象）。
    fn constructing_native(&self) -> bool;

    // 对象分配 / 字符串创建
    fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject;
    fn new_string(&mut self, s: &str) -> JsValue;
    /// move 接收 `String` 创建会话字符串，避免一次整串克隆。
    fn new_string_owned(&mut self, s: String) -> JsValue;
    /// 取 ASCII 单字符的永久字符串值：命中返回共享 perm 串（零分配、可指针
    /// 短路比较），非 ASCII 返回 `None` 由调用方回落普通字符串创建。
    ///
    /// # 注意事项
    /// `&self` 可借用期调用；`None` 回落 `new_string` 是 `&mut` 路径，须先结束
    /// 本次 `&self` 借用（返回值即时消费即可）。
    fn single_char(&self, ch: char) -> Option<JsValue> {
        if ch.is_ascii() {
            Some(JsValue::string(oxide_kernel::string_forge::single_char_ptr(ch as u8)))
        } else {
            None
        }
    }
    /// 借出字符串值的 UTF-16 单元序列（Flat 惰性编码 / FlatU16 直接借用 /
    /// Cons 扁平化缓存），生命周期绑定到 `&self` 借用。调用方须保证
    /// `val` 为字符串值。
    fn string_units(&self, val: JsValue) -> std::borrow::Cow<'_, [u16]>;
    /// 取单单元的永久字符串值：ASCII 单元命中共享 perm 串（零分配、可指针
    /// 短路），非 ASCII 返回 `None` 由调用方回落单元创建。
    fn single_unit(&self, u: u16) -> Option<JsValue> {
        if u < 0x80 {
            Some(JsValue::string(oxide_kernel::string_forge::single_char_ptr(u as u8)))
        } else {
            None
        }
    }
    /// 以单元序列创建会话字符串（智能路由：含孤立 surrogate 落 FlatU16）。
    fn new_string_units(&mut self, units: &[u16]) -> JsValue;
    /// 同 `new_string_units`，以 owned 单元序列接收，避免一次克隆。
    fn new_string_units_owned(&mut self, units: Vec<u16>) -> JsValue;
    /// 分配 BigInt 值（num_bigint::BigInt box 登记到 VM，返回携带指针的 `JsValue`）。
    fn new_bigint(&mut self, v: num_bigint::BigInt) -> JsValue;
    /// 读取 BigInt 值；调用方须保证 `val.is_bigint()`。
    fn bigint_value(&mut self, val: JsValue) -> &num_bigint::BigInt;

    // 内核访问器
    fn kernel_core(&self) -> &Arc<KernelCore>;
    fn session(&self) -> &KernelSession;
    fn epoch(&self) -> &Epoch;

    // 属性解析
    fn property_key_si(&mut self, val: JsValue) -> u32;
    /// ToPropertyKey 完整路径：键值 → 内部属性键 si。对象经 ToPrimitive(string
    /// hint)，转换异常以 `Err` 返回（调用方须取 `take_uncaught_value` 传播原异常）。
    /// 与 `property_key_si` 的区别：后者对转换失败退化为空键，不传播异常。
    fn to_property_key_si(&mut self, val: JsValue) -> Result<u32, String>;
    /// 字符串→键规范化：规范数字串（`"5"`）映射整数键，其余文本以 `encode_key`
    /// 形态入键空间（无 FFFD 的良形文本恒等，含 FFFD 与运行时单元路径同形态，
    /// 见 `string_key_units`）。
    /// 供建键入口（fromEntries/json/rest excluded）与 `property_key_si` 的字符串分支统一口径。
    fn string_key_si(&mut self, s: &str) -> u32;
    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue>;
    /// 存在性判定（规范 HasProperty）：自身与原型链任一层 P 为自有属性即存在；
    /// 数组元素区 hole 视同缺失，TypedArray 整数索引按视图长度判在界。
    fn has_property(&self, obj: &JsObject, prop_name_si: u32) -> bool;
    fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32>;

    // 属性访问
    fn ordinary_get(&mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue) -> Result<JsValue, String>;
    /// `strict` 为写方严格模式：写失败时严格抛错，sloppy 静默 no-op。内置调用方
    /// 一律传 true（内置写语义恒抛错，与调用上下文模式无关）。
    fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, strict: bool,
    ) -> Result<(), String>;

    // 属性定义
    fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String>;
    fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String>;
    fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue);
    /// 全局 builtin 属性 A 侧写/删成功后，把当前帧对应镜像槽反向同步为 `val`
    ///（维护"槽 = A 侧原始存储"不变式）。
    ///
    /// # 边界与前提
    /// - `obj` 非会话全局对象时立即 no-op（接收者指针判等，热路径零额外成本）。
    /// - 键未登记在活动模块 builtin 镜像名集时无槽可写，no-op。
    /// # 副作用
    /// - 写当前帧寄存器文件的镜像槽。
    fn sync_global_builtin_mirror(&mut self, obj: &JsObject, key_si: u32, val: JsValue);

    // 查找 / 强制转换
    fn lookup_str(&self, val: JsValue) -> Option<String>;
    fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String>;
    fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String>;

    // 调用基础设施
    fn call_function_sync(&mut self, callee: JsValue, receiver: JsValue, args: &[JsValue]) -> Result<JsValue, String>;
    /// 构造调用（Construct(C, args)）：native 构造器以新对象为 receiver 值传递调用，
    /// bytecode 构造器压构造帧执行（含 derived 构造器 super() 语义与 new.target
    /// 传播），返回值非对象时回退到新对象。
    ///
    /// # 边界与前提
    /// - `ctor` 必须为可构造值（箭头 / 非构造 native / 普通值在入口处拒绝，
    ///   返回 `Err`）；`args` 为完整实参列表。
    ///
    /// # 副作用
    /// - bytecode 构造器压帧内嵌 dispatch：调用方执行状态在帧边界保存/恢复
    ///   （寄存器窗口 / 表代际 / spill）。
    /// - 失败时已消费 `last_uncaught_value`，`Err` 直接携带原始抛出值
    ///   （保原值身份，不做文本降级）；调用方不得再取槽。
    fn construct_ctor(&mut self, ctor: JsValue, args: &[JsValue]) -> Result<JsValue, JsValue>;
    /// 带 newTarget 覆写的构造调用（Construct(C, args, newTarget)）：this 与
    /// new.target 均按 `new_target` 推导；不覆写时调用方传构造器本身（与
    /// `construct_ctor` 逐位等价）。
    ///
    /// # 边界与前提
    /// - `ctor` 必须为可构造值（入口拒绝，返回 `Err`）；`new_target` 的可构造性
    ///   由调用方入口校验；`args` 为完整实参列表。
    ///
    /// # 副作用
    /// - 同 `construct_ctor`：bytecode 构造器压帧内嵌 dispatch，失败时消费
    ///   `last_uncaught_value`，`Err` 直接携带原始抛出值（保原值身份，不做
    ///   文本降级）；调用方不得再取槽。
    fn construct_ctor_nt(&mut self, ctor: JsValue, new_target: JsValue, args: &[JsValue]) -> Result<JsValue, JsValue>;
    /// 取回在 String 展平调用边界上保留下来的原始抛出 JsValue，
    /// 使迭代器包装器能重新抛出原错误而非二次包装。
    fn take_uncaught_value(&mut self) -> Option<JsValue>;
    /// 恢复被忽略调用暂存的原始抛出值（与 [`Self::take_uncaught_value`] 配对）。
    ///
    /// # 注意事项
    /// 忽略调用（如 IteratorClose 的 `return()`）抛错时不得让自身值覆盖槽——
    /// 调用前暂存、调用后恢复，保证在途异常值跨忽略调用存活。
    fn restore_uncaught_value(&mut self, value: Option<JsValue>);
    /// 取回数组 length define 强转期用户代码抛出的原始异常。
    ///
    /// # 注意事项
    /// - 与 [`Self::take_uncaught_value`] 分离：后者可能残留其它操作忽略调用时
    ///   写入的值，本槽仅由 length 强转失败写入，入口据此区分「强转异常」与
    ///   「描述符收敛失败」。
    fn take_pending_length_exception(&mut self) -> Option<JsValue>;

    // 错误处理
    fn checked_object_ptr(&mut self, val: JsValue, error_msg: &str) -> Result<Option<*mut JsObject>, String>;
    /// 读当前分派指令位置（pc）。供有界强转调用方在强转前后采样：深度 0
    /// 用户回调抛错已就地展开时 pc 必已变化，调用方须立即停止执行后续步骤
    /// （强转结果为残值，继续即假值写或二次抛错）。
    fn pc(&self) -> usize;
    fn raise_type_error(&mut self, msg: &str) -> Result<(), String>;
    /// 恢复已捕获的原始异常值（强转失败载荷）并走异常展开，保原值 kind：
    /// 深度 0 置原值入异常通道就地展开到外围 catch；深度 >0 返回 kind 前缀
    /// 文本，由原生调用边界恢复为异常对象。
    ///
    /// # 边界与前提
    /// - `exc` 须为调用方已提取的原始异常值（如经 `take_uncaught_value`）；
    ///   本入口不重取 uncaught 槽。
    ///
    /// # 副作用
    /// - 深度 0 写 `exception_value`/`pending_error_kind`，pc 经展开改写。
    fn raise_captured(&mut self, exc: JsValue) -> Result<(), String>;
    fn error_message_text(&self, kind: &str, msg: &str) -> String;
    fn call_stack_function_names(&self) -> Vec<String>;
    fn promote_if_needed_for_write_ptr(&mut self, target_ptr: *mut JsObject, value: JsValue) -> JsValue;
    fn step_rng(&mut self);
    fn math_rng_value(&self) -> f64;
    fn sub_module_function_name(&self, gen: u32, sub_idx: u16) -> String;
    /// 取当前字节码帧 `cell_stack.last()[cell_idx]` 的共享 cell 指针。
    ///
    /// # 边界与前提
    /// - 仅在同一 VM 世代的字节码帧执行期读取：native 调用不压 `cell_stack`，故
    ///   builtin 内读到的是调用方模块帧；`cell_idx` 越界或槽为空返回 `None`。
    fn module_frame_cell(&self, cell_idx: u32) -> Option<*mut Cell>;
    /// 动态编译一个函数体（`Function` 构造器用）：把参数列表与函数体编译为可调用
    /// 函数对象。编译或解析失败返回 `Err`，由调用方转为 `SyntaxError`。
    fn create_dynamic_function(&mut self, params: &[String], body: &str) -> Result<JsValue, String>;
    /// 动态编译脚本（`eval` 字符串模式）：按脚本模式编译，var/函数声明落全局对象。
    /// 编译或解析失败返回 `Err`，由调用方转为 `SyntaxError`。
    fn create_dynamic_script(&mut self, code: &str) -> Result<JsValue, String>;
    /// `None` 表示无描述（`Symbol()`/`Symbol(undefined)`），`Some(desc)` 为字符串描述。
    fn symbol_intern(&mut self, desc: Option<String>) -> u32;
    fn symbol_description(&self, idx: u32) -> Option<&str>;
    fn symbol_lookup_global(&self, key: &str) -> Option<u32>;
    fn symbol_register_global(&mut self, key: String, idx: u32);
    fn symbol_key_for_id(&self, idx: u32) -> Option<String>;
}
