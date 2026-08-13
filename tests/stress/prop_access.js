var ITERATIONS = 100000;

var obj = { a: 1, b: 2, c: 3, d: 4 };
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += obj.a + obj.b + obj.c + obj.d;
}
sum
