var ITERATIONS = 10000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var obj = {};
  for (var j = 0; j < 20; j++) {
    obj["k" + j] = j;
  }
  sum += Object.keys(obj).length;
}
sum
