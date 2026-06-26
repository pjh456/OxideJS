var ITERATIONS = 10000;

var obj = {};
for (var i = 0; i < ITERATIONS; i++) {
  obj["key" + i] = i;
}
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += obj["key" + i];
}
sum
