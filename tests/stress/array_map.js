var ITERATIONS = 2000;

var base = [];
for (var i = 0; i < 100; i++) {
  base.push(i);
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  var out = base.map(function (x) { return x + 1; });
  sum += out[0];
}
sum
