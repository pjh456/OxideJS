var ITERATIONS = 50000;

var s = "Mixed CASE String with UPPER and lower parts";
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += s.toUpperCase().length + s.toLowerCase().length;
}
sum
