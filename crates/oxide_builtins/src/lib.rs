//! OxideJS 内置对象层（builtins）。
//!
//! 本 crate 实现 ECMAScript 内置对象与全局函数的 native 实现，包括 Array、String、
//! Date、Object、Map、Set、JSON、Math、Number、RegExp、Symbol、TypedArray、
//! ArrayBuffer、DataView、Reflect、Function、Error 与全局函数（URI/escape 等）。
//! 每个内置方法以 `fn xxx<H: VmHost>(vm, args: &[u8]) -> NativeResult` 形式暴露，
//! 由上层（oxide_vm / oxide_api）注册为 JS 全局对象上的 native function。
//! 架构性延后的特性（Proxy、WeakMap、WeakSet、WeakRef、FinalizationRegistry、
//! SharedArrayBuffer、Atomics）在 `stubs` 中仅提供占位实现，调用时抛 TypeError。

/// Array 内置对象实现（目录模块：common 共享助手、from 构造器与静态方法、element 元素变更、iterate 高阶迭代、sort_iterator 排序与迭代协议、immutable ES2023 不可变方法族）。
pub mod array;
/// ArrayBuffer 内置对象实现（字节缓冲区与 byteLength/slice/isView）。
pub mod array_buffer;
/// BigInt 内置对象实现（constructor 与 prototype 的 toString）。
pub mod bigint;
/// Boolean 内置对象实现（constructor 与 prototype 的 valueOf/toString）。
pub mod boolean;
/// builtins 层日志宏（`builtins_error`/`builtins_info` 等），target 为 `oxide::builtins`。
pub mod builtins_log;
/// Console 全局对象实现（log/warn/error/info/debug/trace 方法）。
pub mod console;
/// DataView 内置对象实现（对 ArrayBuffer 的定点读写视图）。
pub mod data_view;
/// Date 内置对象实现（时间戳存取、get/set 系列与 to*String 系列）。
pub mod date;
/// DisposableStack/AsyncDisposableStack 内置对象实现（资源栈状态盒与 GC 追踪）。
pub mod disposable_stack;
/// Error 内置对象实现（各类 Error 构造函数与 stack/toString）。
pub mod error;
/// eval 全局函数实现（动态编译执行脚本/表达式）。
pub mod eval;
/// Function 内置对象实现（call/apply/bind/toString）。
pub mod function;
/// 全局函数模块（URI 编解码与 Annex B 的 escape/unescape）。
pub mod global;
/// 迭代器相关实现（Iterator 包装、for-of 底层迭代逻辑）。
pub mod iterator;
/// JSON 内置对象实现（parse/stringify）。
pub mod json;
/// Map 内置对象实现（键值集合与 entries/keys/values 迭代）。
pub mod map;
/// Math 内置对象实现（数学函数）。
pub mod math;
/// 模块命名空间与求值辅助（import 实现内部用）。
pub mod module;
/// Number 内置对象实现（constructor 与 toFixed/isInteger 等）。
pub mod number;
/// Object 内置对象实现（keys/create/assign/defineProperty 等静态方法）。
pub mod object;
/// Reflect 内置对象实现（属性操作镜像）。
pub mod reflect;
/// RegExp 内置对象实现（constructor/test/exec/toString）。
pub mod regexp;
/// Set 内置对象实现（值集合与 entries/values/keys 迭代）。
pub mod set;
/// String 内置对象实现（目录模块：common 共享工具、basic 基础方法、regex 正则与匹配）。
pub mod string;
/// 未实现特性的占位 native 实现（统一抛 TypeError）。
pub mod stubs;
/// Symbol 内置对象实现（constructor/for/keyFor/toString）。
pub mod symbol;
/// TypedArray base64/hex 编解码纯函数（FromBase64/FromHex 状态机与编码输出）。
pub mod ta_codec;
/// Temporal 内置对象实现（目录模块树：Now / Instant / PlainDate / PlainTime / PlainDateTime / PlainMonthDay / PlainYearMonth / ZonedDateTime / Duration）。
pub mod temporal;
/// TypedArray 内置对象实现（元素访问、fill/slice/subarray/set 等）。
pub mod typed_array;
