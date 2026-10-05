var ITERATIONS = 3000;

var arr = [];
for (var i = 0; i < 100; i++) {
  arr.push("item" + i);
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  for (var k = 0; k < 100; k++) {
    sum += arr[k].length;
  }
}
sum
