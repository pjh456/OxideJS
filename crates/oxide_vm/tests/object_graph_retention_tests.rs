//! mem_object_graph 留存计数回归锚：逃逸写直通 + 测量点 promote 语义下的
//! 确定性账目。
//!
//! 15 层满二叉树（32767 节点）在「写路径直通（不引克隆）+ 根 promote 深克隆」
//! 晋升语义下的三个确定性计数。run 末 session 表只含直 session 分配对象，
//! 整树节点字面量驻留 epoch；promote_rooted 把顶层 var 寄存器根的 epoch 原件
//! 深克隆为 session 内唯一副本。晋升语义或根处理改动后，若以下任一数值变化，
//! 说明 session 留存账目已变——须对照 tests/stress/mem_object_graph.js 的基线
//! （benchmark_baseline.json）复核后再更新本锚。

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

/// 留存锚：逃逸写直通后 run 末 session 表只含直 session 分配的 2 枚对象
/// （makeNode 函数对象与其 prototype 子对象），整树节点字面量驻留 epoch，
/// 顶层 var 寄存器槽仍持有 epoch 原件（SET 路径不回写寄存器）；测量点
/// promote_rooted 把寄存器原件深克隆进 session（转发表去重，整树 32767），
/// full GC 后逻辑存活集（整树 + 函数 2）在 session 内恰一份、无双计。
/// epoch 侧 65534 的 2× 与节点无关：其中 32767 是每次 JS 调用无条件创建的
/// arguments 对象（CREATE_ARGUMENTS），另 32767 才是节点字面量。
#[test]
fn object_graph_retention_anchor() {
    let mut vm = run_module(OBJECT_GRAPH);

    assert_eq!(vm.epoch_object_count(), 65534, "run 末 epoch 计数");
    assert_eq!(vm.session_object_count(), 2, "run 末 session 直分配计数（函数对象 + prototype 子对象）");

    vm.promote_rooted_epoch_objects();
    vm.promote_session_epoch_refs();
    vm.collect_session_gc();
    assert_eq!(
        vm.session_object_count(),
        32769,
        "留存锚：整树根克隆 + 函数直分配 = 逻辑存活集 32769（单份，无双计）"
    );
}
