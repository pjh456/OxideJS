var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += (i % 2 === 0) ? true : false;
  sum += i === 0 ? 0 : 1;
  sum += i > 0 ? 1 : 0;
}
sum
