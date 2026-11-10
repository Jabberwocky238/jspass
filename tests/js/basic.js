function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}
let s = 0;
for (let i = 0; i < 10; i++) {
  s += i;
}
console.log("sum", s, fib(20));
