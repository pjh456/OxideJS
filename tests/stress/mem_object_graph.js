// 大型对象图留存：15 层满二叉树共 32767 个节点，顶层 root 整体驻留 session 堆
// 每节点 id 唯一（免意外共享），内节点带 left/right 子引用，叶节点带 leaf 标记
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
