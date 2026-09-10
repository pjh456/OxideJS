// 数组增缩容量：填满 200000 元素后截回 1000，观测容量是否回落
// 堆账目按元素 Vec capacity 计——截短后若不收缩，留存账目仍反映大容量
var arr = [];
for (var i = 0; i < 200000; i++) {
  arr[i] = i * 3 + 1;
}
arr.length = 1000;
arr.length
