var ITERATIONS = 200;

function fib(n) {
  if (n <= 1) return n;
  return fib(n - 1) + fib(n - 2);
}
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += fib(15) % 10;
}
sum
