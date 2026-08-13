var ITERATIONS = 100000;

var arr = [];
for (var i = 0; i < 1000; i++) {
  arr.push(0);
}
for (var j = 0; j < ITERATIONS; j++) {
  arr[500] = j;
}
arr[500]
