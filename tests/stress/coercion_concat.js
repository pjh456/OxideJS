var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var s = "42" + i;
  sum += s.length;
}
sum
