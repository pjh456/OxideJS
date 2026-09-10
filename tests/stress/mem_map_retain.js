// 原生盒边持有：Map 持 20000 组字符串键 + 对象值，
// 键串与值对象（及其 payload 串）全部经 Map 原生盒边驻留
var m = new Map();
for (var i = 0; i < 20000; i++) {
  m.set("map_key_" + i, { id: i, data: "map_value_payload_" + i });
}
m.size
