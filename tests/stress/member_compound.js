var ITERATIONS = 100000;

// 单态段：同一对象复合写，IC 读写全命中（写侧直写槽、零查表零回写）。
var t = { x: 0 };
for (var i = 0; i < ITERATIONS; i++) {
  t.x += 1;
}

// 多态段：4 种 shape 轮换复合写，4 槽缓存覆盖，仅学习 miss。
var objs = [{ x: 0 }, { x: 0, y: 1 }, { x: 0, z: 1 }, { x: 0, y: 1, z: 2 }];
var s = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var o = objs[i % 4];
  o.x += 1;
  s += o.x;
}
t.x + s
