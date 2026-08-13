var ITERATIONS = 20000;

var obj = { a: 1, b: 2, c: 3 };
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  delete obj.a;
  obj.a = i;
  sum += obj.a;
}
sum
