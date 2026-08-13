var ITERATIONS = 20000;

var obj = { value: 1, add: function (n) { return this.value + n; } };
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += obj.add(i);
}
sum
