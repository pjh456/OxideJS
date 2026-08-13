var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var n = i & 1;
  sum += n == 0;
  sum += n != 1;
  sum += n === 0;
  sum += n !== 1;
  sum += n < 2;
  sum += n >= 0;
}
sum
