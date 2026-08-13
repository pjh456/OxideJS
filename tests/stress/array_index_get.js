var ITERATIONS = 100000;

var arr = [];
for (var i = 0; i < 1000; i++) {
  arr.push(i);
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  sum += arr[500];
}
sum
