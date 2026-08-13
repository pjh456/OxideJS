var ITERATIONS = 5000;

var obj = {};
for (var i = 0; i < 100; i++) {
  obj["k" + i] = i;
}
var sum = 0;
for (var j = 0; j < ITERATIONS; j++) {
  for (var k in obj) {
    sum += obj[k];
  }
}
sum
