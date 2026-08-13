var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += Math.floor(i * 1.5);
  sum += Math.abs(i - 50000);
  sum += Math.max(i, 0);
  sum += Math.min(i, 999999);
}
sum
