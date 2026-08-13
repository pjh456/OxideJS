var ITERATIONS = 1500;

var s = "";
for (var i = 0; i < 50; i++) {
  s += "ab";
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  for (var k of s) {
    sum += k.charCodeAt(0);
  }
}
sum
