// 字符串累积留存：40000 个互不相同的字符串驻留数组
// 内容由索引推导（无去重可合并），数组本身经顶层 var 驻留
var pool = [];
for (var i = 0; i < 40000; i++) {
  pool[i] = "str_" + i + "_" + (i * 7 % 997) + "_padding_padding_padding";
}
pool.length
