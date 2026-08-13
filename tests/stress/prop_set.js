var ITERATIONS = 100000;

var obj = { x: 0, y: 0 };
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  obj.x = i;
  obj.y = i * 2;
  sum += obj.x + obj.y;
}
sum
