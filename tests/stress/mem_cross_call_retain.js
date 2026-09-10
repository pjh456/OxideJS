// 跨调用长存活数据：全局 store 经 30000 次函数调用累积 30000 条记录，
// 记录在函数内构造、写出 global 后跨帧存活（session 堆）
var store = { records: [], total: 0 };
function logRecord(tag, value) {
  var r = { seq: store.total, tag: tag, value: value };
  store.records.push(r);
  store.total++;
}
for (var i = 0; i < 30000; i++) {
  logRecord("tag_" + (i % 16), "record_payload_" + i);
}
store.records.length
