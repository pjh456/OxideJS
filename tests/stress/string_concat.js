var ITERATIONS = 50000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var a = "1";
  var b = "2";
  var c = "3";
  var d = a + b + c + a + b + c;
  sum += d.length;
}
sum
