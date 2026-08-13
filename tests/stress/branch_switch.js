var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  switch (i % 4) {
    case 0: sum += 0; break;
    case 1: sum += 1; break;
    case 2: sum += 2; break;
    default: sum += 3; break;
  }
}
sum
