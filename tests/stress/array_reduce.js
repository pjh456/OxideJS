var ITERATIONS = 2000;

var base = [];
for (var i = 0; i < 100; i++) {
  base.push(i);
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  var out = base.reduce(function (acc, x) { return acc + x; }, 0);
  sum += out;
}
sum
