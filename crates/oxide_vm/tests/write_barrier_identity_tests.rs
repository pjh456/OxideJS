//! 逃逸写屏障 identity 分裂判别钉（写屏障退直通后：原件与逃逸目标恒同一对象）。
//!
//! 覆盖形态：IIFE 纯 JS 逃逸写、flat/flatMap/splice/for-of/嵌套元素窗内访问器
//! 逃逸写、setter 深度、晋升压载下双引用一致性；对照钉钉住 native 盒双态一致
//! 与跨 reset 边界克隆收敛。期望值与 node 行为一致。
//!
//! 关键不变式：逃逸写（对象值写向全局/session 目标）之后，原件引用与逃逸目标
//! 引用指向同一对象，经任一路径的读写互见；执行期晋升与边界修复不引入第二份。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

fn vm_default() -> Vm {
    Vm::new()
}

/// 低 session GC 阈值 VM：执行期两档收集在少量分配内即可触发。
fn vm_with_threshold(bytes: usize) -> Vm {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(bytes);
    Vm::with_kernel_core(KernelCore::new(config))
}

/// 执行源码并取字符串完成值。
fn eval_string(vm: &mut Vm, source: &str) -> String {
    let result = vm.run(&Arc::new(compile(source))).expect("run");
    vm.lookup_str(result).expect("完成值应为字符串").to_string()
}

/// P1 最基础形态：IIFE 内 `g = inner; g[1] = 99`（无 builtin 无 getter），
/// 逃逸写后原件、逃逸目标值与 identity 三读一致。
#[test]
fn escape_write_iife_global_no_accessor() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var inner = [1, 2]; \
         g = inner; \
         g[1] = 99; \
         return inner[1] + '|' + g[1] + '|' + (inner === g); \
       })()",
    );
    assert_eq!(out, "99|99|true", "逃逸写后原件与逃逸目标应同一对象且值互见");
}

/// P2 flat 主形态：getter 内逃逸写后，flat 续读应见写入值而非原件旧值。
#[test]
fn escape_write_flat_accessor_readback() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var inner = [1, 2]; \
         Object.defineProperty(inner, 0, { configurable: true, get: function(){ g = inner; g[1] = 99; return 1; } }); \
         return JSON.stringify(inner.flat()); \
       })()",
    );
    assert_eq!(out, "[1,99]", "flat 经原件续读应见 getter 内逃逸写值");
}

/// P3 splice 移位 SET 形态：getter 逃逸写后，移位写经 setter 落原件，
/// 读回链（removed 值、移位后空洞、setter 捕获值）三者与 node 一致。
#[test]
fn escape_write_splice_shifted_readback() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var inner = [1, 2]; \
         var store = 1; \
         Object.defineProperty(inner, 0, { \
           configurable: true, \
           get: function(){ g = inner; g[1] = 99; return store; }, \
           set: function(v){ store = v; } \
         }); \
         var r = inner.splice(0, 1); \
         return JSON.stringify(r) + '|' + inner[1] + '|' + store; \
       })()",
    );
    assert_eq!(out, "[1]|undefined|99", "移位 SET 应经原件 setter 落值");
}

/// P4 setter 深度：`g[1] = 99` 经 setter 写 this 命名属性，原件与逃逸目标
/// 均应见 setter 写入。
#[test]
fn escape_write_setter_depth_original_visible() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var inner = [1, 2]; \
         Object.defineProperty(inner, 0, { configurable: true, get: function(){ g = inner; g[1] = 99; return 1; } }); \
         Object.defineProperty(inner, 1, { configurable: true, set: function(v){ this._t = v; } }); \
         inner.flat(); \
         return (inner._t === 99) + '|' + (g._t === 99); \
       })()",
    );
    assert_eq!(out, "true|true", "setter 写入应落在原件上且双引用互见");
}

/// P5 identity 面：逃逸写后大量分配触发执行期晋升，双引用经晋升档
/// 改写后仍同一对象（晋升不引入克隆分裂）。
#[test]
fn escape_write_identity_after_in_run_promotion() {
    let mut vm = vm_with_threshold(4096);
    let out = eval_string(
        &mut vm,
        "var inner = [1, 2]; \
         g = inner; \
         var big = []; \
         for (var i = 0; i < 50000; i++) big.push({ v: i }); \
         (inner === g) + '|' + inner[1] + '|' + g[1]",
    );
    assert_eq!(out, "true|2|2", "执行期晋升后双引用应仍同一对象且值一致");
    assert!(vm.session_gc_stats().total_collections > 0, "应触发执行期收集");
}

/// P5 值面对照钉：晋升压载下双引用读回值恒一致（压载只作收集触发器）。
#[test]
fn escape_write_value_consistent_under_in_run_pressure() {
    let mut vm = vm_with_threshold(4096);
    let out = eval_string(
        &mut vm,
        "var inner = [1, 2]; \
         g = inner; \
         var big = []; \
         for (var i = 0; i < 50000; i++) big.push({ v: i }); \
         inner[1] + '|' + g[1]",
    );
    assert_eq!(out, "2|2", "压载收集后双引用读回值应一致");
    assert!(vm.session_gc_stats().total_collections > 0, "应触发执行期收集");
}

/// P6 for-of 形态：被迭代数组的访问器逃逸写后，迭代续读与迭代后读回一致。
#[test]
fn escape_write_for_of_iterated_readback() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var inner = [1, 2]; \
         Object.defineProperty(inner, 0, { configurable: true, get: function(){ g = inner; g[1] = 99; return 1; } }); \
         var s = 0; \
         for (var x of inner) s += x; \
         return s + '|' + inner[1]; \
       })()",
    );
    assert_eq!(out, "100|99", "迭代续读应见逃逸写值");
}

/// P7 对照钉：Map native 盒直插（键值不经过写屏障），盒内值与原件双态
/// 恒一致——撤屏障克隆后该面不变。
#[test]
fn map_native_box_dual_state_consistent() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var inner = [1, 2]; \
         var m = new Map(); \
         m.set('k', inner); \
         var h = m.get('k'); \
         h[1] = 99; \
         return inner[1] + '|' + h[1] + '|' + (inner === h); \
       })()",
    );
    assert_eq!(out, "99|99|true", "native 盒读回与原件应双态一致");
}

/// P8 对照钉：跨 run 边界（reset 路径）session 对象的 epoch 子引用经边界
/// 修复克隆收敛——双引用跨边界恒同一对象（REPL 两轮语义的 test 形态）。
#[test]
fn cross_reset_boundary_clone_converges_identity() {
    let mut vm = vm_default();
    vm.run(&Arc::new(compile("globalThis.inner = [1, 2]; globalThis.g = globalThis.inner; 0")))
        .expect("run1");
    vm.reset();
    let out = eval_string(
        &mut vm,
        "globalThis.g[1] = 99; (globalThis.inner[1] === 99) + '|' + (globalThis.inner === globalThis.g)",
    );
    assert_eq!(out, "true|true", "跨 reset 边界后双引用应同一对象且值互见");
}

/// P9 嵌套元素窗：getter 逃逸写嵌套数组后，flat 递归展开经双路读回一致。
#[test]
fn escape_write_nested_element_window() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var nested = [7, 8]; \
         var inner = [nested]; \
         Object.defineProperty(inner, 0, { configurable: true, get: function(){ g = nested; g[1] = 99; return g; } }); \
         var r = inner.flat(); \
         return JSON.stringify(r) + '|' + nested[1]; \
       })()",
    );
    assert_eq!(out, "[7,99]|99", "嵌套元素窗读回应见逃逸写值");
}

/// P10 flatMap 形态：回调结果展开 + 嵌套元素 Get 双窗，逃逸写后经原件读回。
#[test]
fn escape_write_flat_map_accessor_readback() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var inner = [1, 2]; \
         Object.defineProperty(inner, 0, { configurable: true, get: function(){ g = inner; g[1] = 99; return 1; } }); \
         return JSON.stringify(inner.flatMap(function(v){ return [v]; })); \
       })()",
    );
    assert_eq!(out, "[1,99]", "flatMap 续读应见 getter 内逃逸写值");
}

/// P11 对照钉：concat arraylike 无访问器纯赋值窗（length/元素 Get 面），
/// 无逃逸写时结果与 node 逐字一致。
#[test]
fn concat_arraylike_no_accessor_control() {
    let mut vm = vm_default();
    let out = eval_string(
        &mut vm,
        "(function(){ \
         var inner = [1, 2]; \
         var o = { length: 2, 0: 1, 1: 2 }; \
         return JSON.stringify(inner.concat(o)); \
       })()",
    );
    assert_eq!(out, "[1,2,{\"0\":1,\"1\":2,\"length\":2}]", "无访问器窗行为应对照 node");
}
