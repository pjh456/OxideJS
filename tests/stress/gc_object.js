var ITERATIONS = 50000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var obj = { a: i, b: i * 2 };
  sum += obj.a + obj.b;
}
sum
