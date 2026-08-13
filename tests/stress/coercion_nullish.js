var ITERATIONS = 100000;

var obj = { value: 5 };
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += obj.value ?? 0;
  sum += obj.missing ?? 42;
  var x = (i % 2 === 0) ? 1 : null;
  sum += x ?? 7;
}
sum
