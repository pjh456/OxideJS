// 拼接 rope 链留存：对 200 字节字面量重复二元拼接，形成深 Cons 链
// 每链节点账目按拼接逻辑长度计，报告值高于实际字节足迹；
// 链自顶层 var 驻留，节点地址稳定不搬移
var piece =
  "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
var chain = "seed_seed_seed";
for (var i = 0; i < 1000; i++) {
  chain = chain + piece;
}
chain.length
