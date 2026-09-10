// 闭包/upvalue 长链留存：10000 层闭包，每层同时捕获前层闭包与自身 payload 对象，
// 自 head 沿 prev 链回溯第一层时全部驻留（函数对象、upvalue cell、payload 串）
var head = null;
for (var i = 0; i < 10000; i++) {
  (function (prev, payload) {
    head = function () {
      return [prev, payload];
    };
  })(head, { id: i, data: "closure_payload_padding_" + i });
}
var cur = head;
var hops = 0;
while (cur !== null) {
  cur = cur()[0];
  hops++;
}
hops === 10000
