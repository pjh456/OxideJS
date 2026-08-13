var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  if (i % 2 === 0) {
    sum += 1;
  } else if (i % 3 === 0) {
    sum += 2;
  } else {
    sum += 3;
  }
}
sum
