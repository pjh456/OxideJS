var ITERATIONS = 50000;

var arr = [];
for (var i = 0; i < ITERATIONS; i++) {
  arr.push(i);
}
var sum = 0;
while (arr.length > 0) {
  sum += arr.pop();
}
sum
