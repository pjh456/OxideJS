var ITERATIONS = 20000;

var s = "hello world, hello engine, hello rust";
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var r = s.replace("hello", "hi");
  sum += r.length;
}
sum
