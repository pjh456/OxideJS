var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var obj = { x: i, y: i * 2, nested: { a: 1, b: 2 } };
  sum += obj.x + obj.y;
}
sum
