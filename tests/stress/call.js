var ITERATIONS = 200;

function sumTo(n) {
  if (n <= 0) return 0;
  return n + sumTo(n - 1);
}
sumTo(ITERATIONS)
