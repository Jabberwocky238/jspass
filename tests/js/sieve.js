function primes(limit) {
  const marks = [];
  for (let i = 0; i <= limit; i++) marks.push(true);
  const out = [];
  for (let p = 2; p <= limit; p++) {
    if (!marks[p]) continue;
    out.push(p);
    for (let q = p * p; q <= limit; q += p) marks[q] = false;
  }
  return out;
}
const ps = primes(100);
console.log(ps.length, ps.join(","));
let collatz = 27, steps = 0;
while (collatz !== 1) { collatz = collatz % 2 === 0 ? collatz / 2 : 3 * collatz + 1; steps++; }
console.log("collatz", steps);
let h = 0;
const text = "the quick brown fox";
for (let i = 0; i < text.length; i++) h = (h * 31 + text.charCodeAt(i)) | 0;
console.log("hash", h, h >>> 0);
