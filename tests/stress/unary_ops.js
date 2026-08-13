var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += -i;
  sum += ~i;
  sum += !i;
  sum += !(i > 0);
}
sum
