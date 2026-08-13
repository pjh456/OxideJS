var ITERATIONS = 20000;

var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  try {
    throw new Error("boom");
  } catch (e) {
    sum += e.message.length;
  }
}
sum
