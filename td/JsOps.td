//===- JsOps.td - the js dialect ---------------------------------*- tablegen -*-===//
//
// JavaScript as the front end reads it: one op per runtime operation, values of
// one dynamic type (`!js.value`), local bindings as cells (`!js.cell`). Control
// flow is `cf` (branches on `js.truthy`). Lowered by jspass to its register
// bytecode.
//
//===----------------------------------------------------------------------===//

#ifndef JS_OPS
#define JS_OPS

include "mlir/IR/OpBase.td"
include "mlir/IR/AttrTypeBase.td"
include "mlir/IR/EnumAttr.td"
include "mlir/IR/SymbolInterfaces.td"
include "mlir/Interfaces/ControlFlowInterfaces.td"
include "mlir/Interfaces/SideEffectInterfaces.td"

def Js_Dialect : Dialect {
  let name = "js";
  let cppNamespace = "::jspass::js";
  let summary = "JavaScript values, bindings and runtime operations";
  let description = [{
    One op per runtime operation of a script: literals, operators, property
    access, calls. Every value is a `!js.value`; a `let` / `var` / parameter is a
    `!js.cell` read with `js.load` and written with `js.store`.
  }];
  let dependentDialects = ["::mlir::cf::ControlFlowDialect"];
  let useDefaultTypePrinterParser = 1;
  let usePropertiesForAttributes = 1;
}

//===----------------------------------------------------------------------===//
// Types
//===----------------------------------------------------------------------===//

class Js_Type<string name, string typeMnemonic> : TypeDef<Js_Dialect, name> {
  let mnemonic = typeMnemonic;
}

def Js_Value : Js_Type<"Value", "value"> {
  let summary = "any JavaScript value";
}

def Js_Cell : Js_Type<"Cell", "cell"> {
  let summary = "a binding: a parameter, `var`, `let` or `const`";
}

//===----------------------------------------------------------------------===//
// Enums
//===----------------------------------------------------------------------===//

def Js_LiteralKind : I32EnumAttr<"LiteralKind", "a literal without a payload", [
    I32EnumAttrCase<"Undefined", 0, "undefined">,
    I32EnumAttrCase<"Null", 1, "null">,
    I32EnumAttrCase<"True", 2, "true">,
    I32EnumAttrCase<"False", 3, "false">,
  ]> {
  let cppNamespace = "::jspass::js";
}

def Js_BinaryKind : I32EnumAttr<"BinaryKind", "a binary operator", [
    I32EnumAttrCase<"Add", 0, "add">,
    I32EnumAttrCase<"Sub", 1, "sub">,
    I32EnumAttrCase<"Mul", 2, "mul">,
    I32EnumAttrCase<"Div", 3, "div">,
    I32EnumAttrCase<"Rem", 4, "rem">,
    I32EnumAttrCase<"Exp", 5, "exp">,
    I32EnumAttrCase<"Lt", 6, "lt">,
    I32EnumAttrCase<"Le", 7, "le">,
    I32EnumAttrCase<"Gt", 8, "gt">,
    I32EnumAttrCase<"Ge", 9, "ge">,
    I32EnumAttrCase<"Eq", 10, "eq">,
    I32EnumAttrCase<"Ne", 11, "ne">,
    I32EnumAttrCase<"StrictEq", 12, "seq">,
    I32EnumAttrCase<"StrictNe", 13, "sne">,
    I32EnumAttrCase<"BitAnd", 14, "band">,
    I32EnumAttrCase<"BitOr", 15, "bor">,
    I32EnumAttrCase<"BitXor", 16, "bxor">,
    I32EnumAttrCase<"Shl", 17, "shl">,
    I32EnumAttrCase<"Shr", 18, "shr">,
    I32EnumAttrCase<"UShr", 19, "ushr">,
  ]> {
  let cppNamespace = "::jspass::js";
}

def Js_UnaryKind : I32EnumAttr<"UnaryKind", "a unary operator", [
    I32EnumAttrCase<"Neg", 0, "neg">,
    I32EnumAttrCase<"Plus", 1, "plus">,
    I32EnumAttrCase<"Not", 2, "not">,
    I32EnumAttrCase<"BitNot", 3, "bnot">,
    I32EnumAttrCase<"TypeOf", 4, "typeof">,
    I32EnumAttrCase<"Void", 5, "void">,
  ]> {
  let cppNamespace = "::jspass::js";
}

//===----------------------------------------------------------------------===//
// Ops
//===----------------------------------------------------------------------===//

class Js_Op<string mnemonic, list<Trait> traits = []> : Op<Js_Dialect, mnemonic, traits>;

def Js_FuncOp : Js_Op<"func", [IsolatedFromAbove, Symbol]> {
  let summary = "a function: `this` and its parameters are the entry block's arguments";
  let description = [{
    The entry block takes `this`, then one `!js.value` per declared parameter;
    a call with fewer arguments fills the rest with `undefined`. `@main` is the
    script.
  }];
  let arguments = (ins SymbolNameAttr:$sym_name);
  let regions = (region AnyRegion:$body);
  let assemblyFormat = "$sym_name attr-dict-with-keyword $body";
}

def Js_ReturnOp : Js_Op<"return", [Pure, Terminator, ReturnLike]> {
  let summary = "returns from the enclosing `js.func`";
  let arguments = (ins Js_Value:$value);
  let assemblyFormat = "$value attr-dict";
}

def Js_NumberOp : Js_Op<"number", [Pure, ConstantLike]> {
  let summary = "a number literal";
  let arguments = (ins F64Attr:$value);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$value attr-dict";
}

def Js_StringOp : Js_Op<"string", [Pure, ConstantLike]> {
  let summary = "a string literal";
  let arguments = (ins StrAttr:$value);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$value attr-dict";
}

def Js_LiteralOp : Js_Op<"literal", [Pure, ConstantLike]> {
  let summary = "`undefined`, `null`, `true` or `false`";
  let arguments = (ins Js_LiteralKind:$kind);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$kind attr-dict";
}

def Js_FuncRefOp : Js_Op<"func_ref", [Pure]> {
  let summary = "the function object of a `js.func`";
  let arguments = (ins FlatSymbolRefAttr:$callee);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$callee attr-dict";
}

def Js_BinaryOp : Js_Op<"binary"> {
  let summary = "a binary operator (may call `valueOf` / `toString`)";
  let arguments = (ins Js_BinaryKind:$kind, Js_Value:$lhs, Js_Value:$rhs);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$kind $lhs `,` $rhs attr-dict";
}

def Js_UnaryOp : Js_Op<"unary"> {
  let summary = "a unary operator";
  let arguments = (ins Js_UnaryKind:$kind, Js_Value:$operand);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$kind $operand attr-dict";
}

def Js_TruthyOp : Js_Op<"truthy", [Pure]> {
  let summary = "`ToBoolean`: what a branch on the value takes";
  let arguments = (ins Js_Value:$value);
  let results = (outs I1:$result);
  let assemblyFormat = "$value attr-dict";
}

def Js_VarOp : Js_Op<"var"> {
  let summary = "a binding of the function (`undefined` until stored)";
  let arguments = (ins StrAttr:$name);
  let results = (outs Res<Js_Cell, "", [MemAlloc<DefaultResource>]>:$cell);
  let assemblyFormat = "$name attr-dict";
}

def Js_LoadOp : Js_Op<"load"> {
  let summary = "reads a binding";
  let arguments = (ins Arg<Js_Cell, "", [MemRead<DefaultResource>]>:$cell);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$cell attr-dict";
}

def Js_StoreOp : Js_Op<"store"> {
  let summary = "writes a binding";
  let arguments = (ins Js_Value:$value, Arg<Js_Cell, "", [MemWrite<DefaultResource>]>:$cell);
  let assemblyFormat = "$value `,` $cell attr-dict";
}

def Js_GlobalGetOp : Js_Op<"global_get"> {
  let summary = "reads a property of the global object (`ReferenceError` if absent)";
  let arguments = (ins StrAttr:$name);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$name attr-dict";
}

def Js_GlobalSetOp : Js_Op<"global_set"> {
  let summary = "writes a property of the global object";
  let arguments = (ins StrAttr:$name, Js_Value:$value);
  let assemblyFormat = "$name `,` $value attr-dict";
}

def Js_GetPropOp : Js_Op<"get_prop"> {
  let summary = "`object.name`";
  let arguments = (ins Js_Value:$object, StrAttr:$name);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$object `[` $name `]` attr-dict";
}

def Js_SetPropOp : Js_Op<"set_prop"> {
  let summary = "`object.name = value`";
  let arguments = (ins Js_Value:$object, StrAttr:$name, Js_Value:$value);
  let assemblyFormat = "$object `[` $name `]` `,` $value attr-dict";
}

def Js_GetElemOp : Js_Op<"get_elem"> {
  let summary = "`object[key]`";
  let arguments = (ins Js_Value:$object, Js_Value:$key);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$object `[` $key `]` attr-dict";
}

def Js_SetElemOp : Js_Op<"set_elem"> {
  let summary = "`object[key] = value`";
  let arguments = (ins Js_Value:$object, Js_Value:$key, Js_Value:$value);
  let assemblyFormat = "$object `[` $key `]` `,` $value attr-dict";
}

def Js_CallOp : Js_Op<"call"> {
  let summary = "calls `callee` with `this` and the arguments";
  let arguments = (ins Js_Value:$callee, Js_Value:$this_arg, Variadic<Js_Value>:$args);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "$callee `(` $args `)` `this` $this_arg attr-dict";
}

def Js_NewObjectOp : Js_Op<"new_object"> {
  let summary = "`{}`";
  let results = (outs Js_Value:$result);
  let assemblyFormat = "attr-dict";
}

def Js_NewArrayOp : Js_Op<"new_array"> {
  let summary = "`[a, b, ...]`";
  let arguments = (ins Variadic<Js_Value>:$elements);
  let results = (outs Js_Value:$result);
  let assemblyFormat = "`[` $elements `]` attr-dict";
}

#endif // JS_OPS
