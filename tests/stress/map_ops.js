var ITERATIONS = 50000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var m = new Map();
  for (var j = 0; j < 10; j++) {
    m.set(j, j * 2);
  }
  sum += m.get(5);
}
sum
