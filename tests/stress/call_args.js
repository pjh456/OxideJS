var ITERATIONS = 20000;

function add(a, b, c, d) {
  return a + b + c + d;
}
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += add(i, i + 1, i + 2, i + 3);
}
sum
