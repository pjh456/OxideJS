//! IC 多态缓存（4 槽，FIFO 滚动写回）行为测试。
//!
//! `run()` 每次从 CodeForge 原始 bytecode 冷启动（IC 学习不跨 run 持久），
//! 因此全部断言基于**单次 run 内**的 miss 计数与迭代数的对比：多态 4 槽
//! 覆盖的 shape 只产生"每 shape 一次"的学习 miss，超出槽容量的轮换则
//! 每次访问都 miss（FIFO 逐访问滚动）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

/// 编译并单次运行，返回 (结果, hits, misses)。
fn run_once(source: &str) -> (String, u64, u64) {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let r = vm.run(&Arc::new(module)).expect("run").to_string();
    (r, vm.ic_hit_count(), vm.ic_miss_count())
}

/// 4 种 shape 轮换访问同一 `.x` 站点：每 shape 一次学习 miss，之后全命中。
#[test]
fn poly4_shape_rotation_only_learning_misses() {
    let (r, hits, misses) = run_once(
        r#"var a = { x: 1 }, b = { x: 2, y: 3 }, c = { x: 4, z: 5 }, d = { x: 6, y: 7, z: 8 };
           var sum = 0;
           for (var i = 0; i < 10000; i++) {
             var t = [a, b, c, d][i % 4];
             sum += t.x;
           }
           sum"#,
    );
    assert_eq!(r, "32500", "x 值 1/2/4/6 各 2500 次累加");
    assert_eq!(misses, 4, "4 shape 均被 4 槽缓存，仅首轮学习各 1 次 miss");
    assert!(hits >= 9990, "后续访问全部命中（实际命中 {hits}）");
}

/// 第 5 种 shape 挤掉 FIFO 最老槽：顺序轮换下每访问都 miss（缓存容量不足）。
#[test]
fn poly5_shape_rotation_evicts_oldest() {
    let (r, _hits, misses) = run_once(
        r#"var s = [
             { x: 1 }, { x: 2, y: 3 }, { x: 4, z: 5 }, { x: 6, y: 7, z: 8 }, { x: 9, a: 1, b: 2 }
           ];
           var sum = 0;
           for (var i = 0; i < 10000; i++) {
             sum += s[i % 5].x;
           }
           sum"#,
    );
    assert_eq!(r, "44000", "x 值 1/2/4/6/9 各 2000 次累加");
    assert!(
        misses > 5000,
        "5 shape 顺序轮换超出 4 槽容量，FIFO 逐访问滚动导致 miss 过半（实际 {misses}）"
    );
}

/// 单态连续访问：仅 1 次学习 miss，空槽 break 不越界。
#[test]
fn monomorphic_only_one_learning_miss() {
    let (r, _hits, misses) = run_once(
        r#"var a = { x: 1 };
           var sum = 0;
           for (var i = 0; i < 10000; i++) { sum += a.x; }
           sum"#,
    );
    assert_eq!(r, "10000");
    assert_eq!(misses, 1, "单态站点仅首轮学习 miss");
}

/// 空 IC（未学习）走慢路径并写回槽 0；首次访问即正确。
#[test]
fn cold_cache_first_access_resolves() {
    let (r, hits, misses) = run_once(
        r#"var a = { x: 7 };
           var sum = 0;
           for (var i = 0; i < 1000; i++) { sum += a.x; }
           sum"#,
    );
    assert_eq!(r, "7000");
    assert_eq!(misses, 1);
    assert_eq!(hits, 999, "学习后 999 次全命中");
}

/// accessor 属性走 has_prop_meta 慢路径，不经 IC 缓存；语义正确即可。
#[test]
fn accessor_property_semantics_preserved() {
    let (r, _hits, _misses) = run_once(
        r#"var o = { get x() { return 1; } };
           var sum = 0;
           for (var i = 0; i < 100; i++) { sum += o.x; }
           sum"#,
    );
    assert_eq!(r, "100", "getter 每次返回 1");
}

/// `__proto__` 赋值走原型设置路径，不写 IC；原型属性读语义正确。
#[test]
fn proto_assignment_not_cached_semantics_preserved() {
    let (r, _hits, _misses) = run_once(
        r#"var p = { m: 42 };
           var o = {};
           o.__proto__ = p;
           var sum = 0;
           for (var i = 0; i < 100; i++) { sum += o.m; }
           sum"#,
    );
    assert_eq!(r, "4200", "__proto__ 赋值建立原型链后经 IC 原型路径读到 42");
}

/// 原型链 depth>0 多态：两个不同 proto shape 轮换，各槽独立 depth。
#[test]
fn proto_polymorphic_two_protos_learning_misses() {
    let (r, _hits, misses) = run_once(
        r#"var b1 = { v: 1 }, b2 = { v: 2, w: 3 };
           var o1 = Object.create(b1), o2 = Object.create(b2);
           var sum = 0;
           for (var i = 0; i < 10000; i++) {
             var o = (i % 2 === 0) ? o1 : o2;
             sum += o.v;
           }
           sum"#,
    );
    assert_eq!(r, "15000");
    assert_eq!(misses, 2, "两个 proto shape 各学习一次，后续 depth>0 命中");
}

/// `rerun()` 清空全部 IC 扩展字：清除后访问重新学习（对照 regression_rerun_clears_ic_cache）。
#[test]
fn rerun_clears_polymorphic_slots() {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(
        &allocator,
        r#"var a = { x: 1 }, b = { x: 2, y: 3 };
           var sum = 0;
           for (var i = 0; i < 1000; i++) {
             var t = (i % 2 === 0) ? a : b;
             sum += t.x;
           }
           sum"#,
    )
    .expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    vm.run(&Arc::new(module)).expect("run1");
    let learned = vm.ic_miss_count();
    // run() 每次冷启动，此处直接验证 rerun 的可重入性：清除后仍能正常执行。
    assert_eq!(format!("{}", vm.rerun().expect("rerun")), "1500");
    let after_rerun = vm.ic_miss_count();
    assert!(
        after_rerun >= learned,
        "rerun 清零多态槽后重新学习（miss 不降反增，实际 {after_rerun} ≥ {learned}）"
    );
}

/// member 复合写（obj.x++）多 shape 轮换：读侧与写侧都经 4 槽缓存命中，
/// 每 shape 仅读侧学习 1 次 miss（写侧复用读侧条目直写）。
#[test]
fn member_compound_polymorphic_read_write_hits() {
    let (r, _hits, misses) = run_once(
        r#"var a = { x: 1 }, b = { x: 2, y: 3 };
           for (var i = 0; i < 10000; i++) {
             var t = (i % 2 === 0) ? a : b;
             t.x += 1;
           }
           a.x + b.x"#,
    );
    assert_eq!(r, "10003", "两对象 x 各自增 5000 次（从 1/2 起）");
    assert_eq!(misses, 2, "两 shape 各 1 次读侧学习 miss，写侧全命中");
}

/// 非对象 receiver（字符串）经 IC primitive 分支：手动跳越全部扩展字后语义正确。
#[test]
fn primitive_receiver_skips_ic_ext_words() {
    let (r, _hits, _misses) = run_once(
        r#"var s = "abc";
           var total = 0;
           for (var i = 0; i < 100; i++) { total += s.length; }
           total + s.length"#,
    );
    assert_eq!(r, "303", "字符串 length 经 primitive 分支返回 3，扩展字跳越无错位");
}

/// 非对象 receiver（symbol）经 IC primitive 分支走 Symbol.prototype 链：
/// toString/description 均经原型解析，扩展字跳越无错位。
#[test]
fn symbol_primitive_receiver_walks_symbol_proto() {
    let (r, _hits, _misses) = run_once(
        r#"var s = Symbol('x');
           var ok = 0;
           for (var i = 0; i < 100; i++) {
             ok += (s.toString() === 'Symbol(x)') ? 1 : 0;
           }
           ok + ((s.description === 'x') ? 1 : 0)"#,
    );
    assert_eq!(r, "101", "symbol 原始值 toString/description 经 Symbol.prototype 解析");
}
