var ITERATIONS = 1000000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var s = "str" + i;
  sum += s.length;
}
sum
