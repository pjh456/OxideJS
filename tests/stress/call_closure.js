var ITERATIONS = 20000;

function outer() {
  var x = 1;
  return function (n) {
    return x + n;
  };
}
var inner = outer();
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += inner(i);
}
sum
