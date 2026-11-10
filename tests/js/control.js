// if / else, while, do-while, for, break / continue, labels, switch
let out = [];
let i = 0;
while (true) {
  i++;
  if (i % 2 === 0) continue;
  if (i > 9) break;
  out.push(i);
}
console.log(out.join(" "));

let n = 0;
do { n += 3; } while (n < 10);
console.log("do", n);

outer: for (let a = 0; a < 4; a++) {
  for (let b = 0; b < 4; b++) {
    if (b === 2) continue outer;
    if (a === 3) break outer;
    console.log("pair", a, b);
  }
}

function kind(x) {
  switch (x) {
    case 1: return "one";
    case 2:
    case 3: return "few";
    default: return "many";
  }
}
console.log(kind(1), kind(2), kind(3), kind(7));

let fall = "";
switch (2) {
  case 1: fall += "a";
  case 2: fall += "b";
  case 3: fall += "c"; break;
  case 4: fall += "d";
}
console.log(fall);

let k = 0;
for (;;) { if (++k >= 5) break; }
console.log("k", k);
