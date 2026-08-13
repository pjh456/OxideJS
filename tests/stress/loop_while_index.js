var ITERATIONS = 3000;

var arr = [];
for (var i = 0; i < 100; i++) {
  arr.push(i);
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  var k = 0;
  while (k < arr.length) {
    sum += arr[k];
    k++;
  }
}
sum
