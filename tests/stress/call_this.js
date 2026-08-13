var ITERATIONS = 20000;

var obj = {
  value: 10,
  getValue: function () { return this.value; },
  setValue: function (v) { this.value = v; return this; }
};
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  obj.setValue(i);
  sum += obj.getValue();
}
sum
