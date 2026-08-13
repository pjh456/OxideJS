var ITERATIONS = 50000;

var s = "The quick brown fox jumps over the lazy dog. 0123456789 abcdefghijklmnopqrstuvwxyz";
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var sub = s.substring(4, 15);
  sum += sub.length;
}
sum
