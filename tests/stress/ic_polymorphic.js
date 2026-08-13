var ITERATIONS = 100000;

var a = { x: 1, y: 2, z: 3 };
var b = { x: 4, y: 5, z: 6 };
var c = { x: 7, y: 8, z: 9 };
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var t = (i % 3 === 0) ? a : (i % 3 === 1) ? b : c;
  sum += t.x + t.y + t.z;
}
sum
