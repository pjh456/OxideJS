// 原生盒边持有：Set 持 20000 个对象，每个对象带 payload 字符串，
// 全部经 Set 原生盒边驻留
var s = new Set();
for (var i = 0; i < 20000; i++) {
  s.add({ id: i, data: "set_value_payload_" + i });
}
s.size
