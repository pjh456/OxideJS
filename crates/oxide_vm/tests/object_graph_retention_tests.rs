//! mem_object_graph 留存计数回归锚：克隆式晋升双计的数值基线。
//!
//! 15 层满二叉树（32767 节点）在「逃逸写屏障克隆 + 测量点 promote」晋升语义下
//! 的三个确定性计数。晋升语义或陈旧顶层寄存器根处理改动后，若以下任一数值
//! 变化，说明双计机制已变——须对照 tests/stress/mem_object_graph.js 的基线
//! （benchmark_baseline.json）复核后再更新本锚。

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
    let result = vm.run(&module).expect("run");
    assert_eq!(result.to_string(), "9", "root.left.id(2) + root.right.right.id(7)");
    vm
}

/// 留存锚：run 末屏障已把整树克隆进 session（全局镜像，32769 = 整树 + 2），
/// 顶层 var 寄存器槽仍持有 epoch 原件（SET 路径不回写寄存器）；测量点
/// promote_rooted 再克隆寄存器原件一份——两份互异副本在 full GC 后均根可达。
/// epoch 侧 65534 的 2× 与节点无关：其中 32767 是每次 JS 调用无条件创建的
/// arguments 对象（CREATE_ARGUMENTS），另 32767 才是节点字面量。
#[test]
fn object_graph_retention_anchor() {
    let mut vm = run_module(OBJECT_GRAPH);

    assert_eq!(vm.epoch_object_count(), 65534, "run 末 epoch 计数");
    assert_eq!(vm.session_object_count(), 32769, "run 末 session 屏障克隆计数");

    vm.promote_rooted_epoch_objects();
    vm.promote_session_epoch_refs();
    vm.collect_session_gc();
    assert_eq!(vm.session_object_count(), 65536, "留存锚：克隆式晋升双计 = 2 × 32769（逻辑存活集 32769）");
}
