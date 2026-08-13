var ITERATIONS = 50000;

var obj = {};
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  obj["key"] = i;
  sum += obj["key"];
}
sum
