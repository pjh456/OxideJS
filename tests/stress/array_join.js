var ITERATIONS = 20000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var arr = [];
  for (var j = 0; j < 10; j++) {
    arr.push(j);
  }
  var joined = arr.join(",");
  sum += joined.length;
}
sum
