# jspass

JavaScript to register bytecode, through MLIR, in Rust: [oxc](https://oxc.rs) reads the source, a front end lowers it
to the `js` dialect (defined in ODS, `td/JsOps.td`) plus `cf` branches, [mlirpass](https://github.com/Jabberwocky238/mlirpass)
runs a pipeline over it, and an emitter turns the result into a compact register bytecode. A small interpreter runs the
bytecode, so every case can be checked against node.

```text
  JavaScript ──oxc──▶ AST ──Front──▶ js + cf ──mlirpass: canonicalize, cse──▶ Emitter ──▶ .jsbc ──▶ Vm
```

## Use

```sh
cargo run -- lower   a.js              # the js dialect, as the front end builds it
cargo run -- opt     a.js              # after the pipeline (--pass-pipeline=... to change it; a.mlir reads IR)
cargo run -- compile a.js -o a.jsbc    # bytecode file
cargo run -- dis     a.js              # disassembly (a.jsbc too)
cargo run -- run     a.js              # interpret (a.jsbc too)
```

```rust
let js = jspass::Jspass::new();
let module = js.compile(source, "a.js")?;     // lower + PIPELINE + emit
let bytes = module.encode()?;                  // .jsbc
jspass::Vm::new(&module, &mut std::io::stdout())?.run()?;
```

## The `js` dialect

`td/JsOps.td` is the source of truth; `src/dialect/js.gen.rs` (the ops, by `mlirformat tblgen`) and
`src/dialect/traits.gen.rs` (traits, memory effects, inherent attributes) are generated from it and not edited by hand:

```sh
python3 scripts/regen.py --mlirformat <mlirformat 0.6.5> --mlirformat-src <mlirformat 0.6.5 source> \
    --mlir-include <llvm-project 22.1.8>/mlir/include
```

Two types: `!js.value` (any value) and `!js.cell` (a binding). Ops: `js.func` (entry block arguments: `this`, then the
parameters; `@main` is the script), `js.return`, literals (`js.number`, `js.string`, `js.literal`), `js.func_ref`,
`js.binary` / `js.unary` (operator as an enum attribute), `js.truthy` (to `i1`, for `cf.cond_br`), bindings (`js.var`,
`js.load`, `js.store`, with alloc / read / write effects so DCE and CSE see them), globals (`js.global_get` / `_set`),
properties (`js.get_prop` / `js.set_prop` / `js.get_elem` / `js.set_elem`), `js.call`, `js.new_object`,
`js.new_array`.

```mlir
js.func @fib {
^bb0(%this: !js.value, %n: !js.value):
  %c2 = js.number 2.000000e+00
  %lt = js.binary lt %n, %c2
  %b = js.truthy %lt
  cf.cond_br %b, ^bb1, ^bb2
  ...
}
```

Top-level declarations of the script are properties of the global object; everything else is a `js.var` cell.

## Bytecode

Register machine (`src/bytecode.rs`): register 0 is `this`, 1..=params the parameters. A file is a constant pool
(numbers, strings) and functions (name, parameter and register counts, instructions); jump targets are byte offsets in
the function. 19 opcodes: `LoadConst`, `LoadLiteral`, `Move`, `Binary`, `Unary`, `GlobalGet` / `Set`, `GetProp` /
`SetProp`, `GetElem` / `SetElem`, `Call`, `NewObject`, `NewArray`, `FuncRef`, `Jump`, `JumpIfTrue` / `False`, `Return`.

## What is supported

`var` / `let` / `const`, functions (declarations, expressions, arrows) that do not capture outer locals, `if`, `while`,
`do`, `for`, `switch`, labels, `break` / `continue`, every arithmetic, bitwise, relational, equality and logical
operator, `?:`, `??`, compound assignment, `++` / `--`, template literals, object and array literals, property access,
method calls with `this`. The interpreter has `console.log` (node's single-line `util.inspect` form), `Math`, array
`push` / `pop` / `join`, `charCodeAt`, `toFixed`.

Not yet: closures over locals, `new` and classes, `try` / `throw`, destructuring, spread, generators and `async`,
`in` / `instanceof` / `delete`, getters and setters.

## Tests

`cargo test --test run` compiles every `tests/js/*.js`, checks that the optimized IR prints and reads back to the same
bytecode and that encoding round-trips, runs it, and compares the output byte for byte with the `.out` beside it, which
is node's (`node a.js > a.out`).

## License

MIT
