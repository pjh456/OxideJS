var ITERATIONS = 50000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var s = new Set();
  for (var j = 0; j < 10; j++) {
    s.add(j);
  }
  sum += s.size;
}
sum
