// 函数 replacer 场景：global 正则多匹配 + 长源串，回调第 4 参为原字符串。
// 每匹配整串复制的消除在此路径可测（m×n 字节拷贝 → 0）。
var ITERATIONS = 2000;

var s = "";
for (var i = 0; i < 50; i++) {
  s += "ab" + i + "c";
}
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var r = s.replace(/b/g, function (m) {
    return "X";
  });
  sum += r.length;
}
sum
