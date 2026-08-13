var ITERATIONS = 20000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var obj = { valueOf: function () { return 42; }, toString: function () { return "42"; } };
  sum += obj + 1;
}
sum
