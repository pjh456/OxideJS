var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var s = String(42) + String(3.14) + String(0) + String(true) + String(null);
  sum += s.length;
}
sum
