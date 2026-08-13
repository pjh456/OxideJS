var ITERATIONS = 5000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var arr = [];
  for (var j = 0; j < 50; j++) {
    arr.push(j);
  }
  sum += arr.length;
}
sum
