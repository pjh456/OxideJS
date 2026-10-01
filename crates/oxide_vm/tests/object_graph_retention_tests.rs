//! mem_object_graph 留存计数回归锚：逃逸写直通 + 测量点 promote 语义下的
//! 确定性账目。
//!
//! 15 层满二叉树（32767 节点）在「写路径直通（不引克隆）+ 根 promote 深克隆」
//! 晋升语义下的确定性计数。对象分配统一入口 Box 化后全部对象入统一表，
//! run 末统一表含整树节点字面量 + 每次 JS 调用无条件创建的 arguments 对象 +
//! 直 session 分配（函数对象与 prototype 子对象）；promote_rooted 对 session
//! 对象全空转（统一表后无 epoch 原件可晋升）。晋升语义或根处理改动后，若以下
//! 任一数值变化，说明 session 留存账目已变——须对照 tests/stress/mem_object_graph.js
//! 的基线（benchmark_baseline.json）复核后再更新本锚。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

/// 与 tests/stress/mem_object_graph.js 同源的 15 层满二叉树。
const OBJECT_GRAPH: &str = r#"
function makeNode(depth, base) {
  if (depth === 0) {
    return { id: base, leaf: 1 };
  }
  return {
    id: base,
    depth: depth,
    left: makeNode(depth - 1, base * 2),
    right: makeNode(depth - 1, base * 2 + 1),
  };
}

var root = makeNode(14, 1);
root.left.id + root.right.right.id
"#;

fn run_module(source: &str) -> Vm {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("run");
    assert_eq!(result.to_string(), "9", "root.left.id(2) + root.right.right.id(7)");
    vm
}

/// 留存锚：对象分配统一入口 Box 化后 run 末统一表含全部对象——整树节点
/// 字面量 32767 + 每次 JS 调用无条件创建的 arguments 对象 32767 + 直 session
/// 分配 2（makeNode 函数对象与其 prototype 子对象）= 65536。promote_rooted
/// 对 session 对象全空转（统一表后无 epoch 原件可晋升）；full GC 后逻辑存活集
/// （整树 + 函数 2）在 session 内恰一份、无双计。
#[test]
fn object_graph_retention_anchor() {
    let mut vm = run_module(OBJECT_GRAPH);

    assert_eq!(vm.session_object_count(), 65536, "run 末统一表计数（节点 + arguments + 直分配）");

    vm.promote_rooted_epoch_objects();
    vm.promote_session_epoch_refs();
    vm.collect_session_gc();
    assert_eq!(
        vm.session_object_count(),
        32769,
        "留存锚：整树根克隆 + 函数直分配 = 逻辑存活集 32769（单份，无双计）"
    );
}
