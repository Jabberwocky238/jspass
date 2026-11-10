function add(a, b) { return a + b; }
var twice = function (f, x) { return f(f(x)); };
const inc = (x) => x + 1;
const sq = (x) => { return x * x; };
console.log(add(2, 3), twice(inc, 5), twice(sq, 3), add(1), add);
function counter() { count = count + 1; return count; }
var count = 0;
counter(); counter();
console.log("count", count);
const obj = { n: 41, get: function () { return this.n + 1; } };
console.log(obj.get());
function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); }
console.log(fact(10), fact(20));
function hoisted() { return later(); }
function later() { return "later"; }
console.log(hoisted());
function local() { var x = 1; if (true) { var x = 2; } let y = 1; { let y = 2; } return x * 10 + y; }
console.log(local());
function noReturn() {}
console.log(noReturn(), inc);
