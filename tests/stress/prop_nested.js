var ITERATIONS = 50000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var obj = { nested: { deep: { value: i } } };
  sum += obj.nested.deep.value;
}
sum
