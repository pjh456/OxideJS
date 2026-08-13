var ITERATIONS = 20000;

var s = "a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q,r,s,t,u,v,w,x,y,z";
var sum = 0;
for (var i = 0; i < ITERATIONS; i++) {
  var parts = s.split(",");
  sum += parts.length;
}
sum
