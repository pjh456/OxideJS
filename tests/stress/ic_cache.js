var ITERATIONS = 10000;

var objs = [];
for (var i = 0; i < ITERATIONS; i++) {
  objs.push({ x: i });
  if (i % 3 === 0) objs[i].y = i;
  if (i % 5 === 0) objs[i].z = i;
}
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  sum += objs[i].x;
}
sum
