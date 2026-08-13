var ITERATIONS = 50000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  try {
    sum += 1;
  } catch (e) {
    sum += 2;
  }
}
sum
