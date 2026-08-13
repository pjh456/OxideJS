var ITERATIONS = 20000;

function Animal(name) {
  this.name = name;
}
Animal.prototype.getName = function () {
  return this.name;
};
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var a = new Animal("x");
  sum += a.getName().length;
}
sum
