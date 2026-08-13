var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += typeof i;
  sum += typeof "str";
  sum += typeof null;
  sum += typeof {}.a;
}
sum
