var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += i * 2 + 3 - 1 / 2;
  sum += (i % 7) * 3;
  sum += i & 255;
  sum += i >> 2;
}
sum
