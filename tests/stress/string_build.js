var ITERATIONS = 20000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var s = "";
  for (var j = 0; j < 50; j++) {
    s += j;
  }
  sum += s.length;
}
sum
