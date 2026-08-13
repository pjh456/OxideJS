var ITERATIONS = 100000;

var obj = { a: 1, b: 2, c: 3 };
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var x = obj["a"];
  sum += x;
}
sum
