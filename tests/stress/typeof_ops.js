var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += (typeof i).length;
  sum += (typeof "str").length;
  sum += (typeof null).length;
  sum += (typeof {}).length;
}
sum
