var ITERATIONS = 20000;

var s = "hello world this is a string that gets trimmed  ";
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += s.trim().length;
}
sum
