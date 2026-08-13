var ITERATIONS = 3000;

var base = [];
for (var i = 0; i < 100; i++) {
  base.push(i);
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  var out = base.slice();
  sum += out.length;
}
sum
