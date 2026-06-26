var ITERATIONS = 50000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += (+"42") + (+"3.14") + (+"0");
}
sum
