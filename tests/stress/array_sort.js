var ITERATIONS = 500;

var base = [];
for (var i = 0; i < 100; i++) {
  base.push(100 - i);
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  var out = base.slice();
  out.sort(function (a, b) { return a - b; });
  sum += out[0];
}
sum
