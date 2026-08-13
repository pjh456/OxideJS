var ITERATIONS = 100000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var s = "abc";
  sum += s.charAt(1) === "b";
  sum += s.charCodeAt(0);
}
sum
