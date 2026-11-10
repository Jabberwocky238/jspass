//! 字节码解释器：跑 [`Module`]，`console.log` 按 node 的格式写进输出。只为对拍与命令行 `run`，语义取 ECMAScript 里
//! 前端支持的那部分（数、字符串、布尔、`null` / `undefined`、对象、数组、函数）。
use std::cell::RefCell;
use std::fmt::Write as _;
use std::io::Write;
use std::rc::Rc;

use mlirformat::Error;

use crate::bytecode::{Const, Constant, Func, Insn, Label, Module, Reg};
use crate::{BinaryKind, LiteralKind, UnaryKind};

/// 调用深度的上限（超过就是 `RangeError`）。
const MAX_DEPTH: usize = 1000;
/// `console.log` 展开嵌套对象 / 数组的层数（node 的 `util.inspect` 缺省 `depth: 2`）。
const INSPECT_DEPTH: usize = 2;
/// 2^32：`ToInt32` / `ToUint32` 的模。
const TWO_32: f64 = 4_294_967_296.0;
/// 移位数只取低 5 位。
const SHIFT_MASK: u32 = 0x1f;

/// 内建函数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    ConsoleLog,
    MathFloor,
    MathCeil,
    MathRound,
    MathTrunc,
    MathSqrt,
    MathAbs,
    MathMax,
    MathMin,
    MathPow,
    ArrayPush,
    ArrayPop,
    ArrayJoin,
    StringCharCodeAt,
    NumberToFixed,
}

impl Native {
    fn name(self) -> &'static str {
        match self {
            Native::ConsoleLog => "log",
            Native::MathFloor => "floor",
            Native::MathCeil => "ceil",
            Native::MathRound => "round",
            Native::MathTrunc => "trunc",
            Native::MathSqrt => "sqrt",
            Native::MathAbs => "abs",
            Native::MathMax => "max",
            Native::MathMin => "min",
            Native::MathPow => "pow",
            Native::ArrayPush => "push",
            Native::ArrayPop => "pop",
            Native::ArrayJoin => "join",
            Native::StringCharCodeAt => "charCodeAt",
            Native::NumberToFixed => "toFixed",
        }
    }

    /// `Math` 上的函数。
    const MATH: [Native; 9] = [Native::MathFloor, Native::MathCeil, Native::MathRound, Native::MathTrunc, Native::MathSqrt, Native::MathAbs, Native::MathMax, Native::MathMin, Native::MathPow];
    /// 数组的方法。
    const ARRAY: [Native; 3] = [Native::ArrayPush, Native::ArrayPop, Native::ArrayJoin];
}

/// 普通对象：属性按插入次序。
#[derive(Debug, Default)]
pub struct Object {
    props: Vec<(Rc<str>, Value)>,
}

impl Object {
    fn get(&self, k: &str) -> Option<&Value> {
        self.props.iter().find(|(n, _)| &**n == k).map(|(_, v)| v)
    }

    fn set(&mut self, k: Rc<str>, v: Value) {
        match self.props.iter_mut().find(|(n, _)| *n == k) {
            Some(slot) => slot.1 = v,
            None => self.props.push((k, v)),
        }
    }
}

/// 一个 JavaScript 值。
#[derive(Debug, Clone)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    Str(Rc<str>),
    Object(Rc<RefCell<Object>>),
    Array(Rc<RefCell<Vec<Value>>>),
    Function(Func),
    Native(Native),
    /// 方法取出来时带上的接收者（`[].push` 之类）
    Bound(Native, Rc<Value>),
}

/// `typeof` 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeOf {
    Undefined,
    Object,
    Boolean,
    Number,
    String,
    Function,
}

impl TypeOf {
    fn name(self) -> &'static str {
        match self {
            TypeOf::Undefined => "undefined",
            TypeOf::Object => "object",
            TypeOf::Boolean => "boolean",
            TypeOf::Number => "number",
            TypeOf::String => "string",
            TypeOf::Function => "function",
        }
    }
}

impl From<&Value> for TypeOf {
    fn from(v: &Value) -> Self {
        match v {
            Value::Undefined => TypeOf::Undefined,
            Value::Null | Value::Object(_) | Value::Array(_) => TypeOf::Object,
            Value::Bool(_) => TypeOf::Boolean,
            Value::Number(_) => TypeOf::Number,
            Value::Str(_) => TypeOf::String,
            Value::Function(_) | Value::Native(_) | Value::Bound(..) => TypeOf::Function,
        }
    }
}

impl From<f64> for Value {
    fn from(x: f64) -> Self {
        Value::Number(x)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.into())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s.into())
    }
}

impl From<LiteralKind> for Value {
    fn from(k: LiteralKind) -> Self {
        match k {
            LiteralKind::Undefined => Value::Undefined,
            LiteralKind::Null => Value::Null,
            LiteralKind::True => Value::Bool(true),
            LiteralKind::False => Value::Bool(false),
        }
    }
}

/// 数按 `Number.prototype.toString` 写。
pub struct JsNumber(pub f64);

impl std::fmt::Display for JsNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 == 0.0 {
            return f.write_str("0");
        }
        f.write_str(ryu_js::Buffer::new().format(self.0))
    }
}

impl Value {
    /// `ToBoolean`。
    pub fn truthy(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(x) => *x != 0.0 && !x.is_nan(),
            Value::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    /// `StringToNumber`。
    fn parse_number(s: &str) -> f64 {
        let t = s.trim();
        if t.is_empty() {
            return 0.0;
        }
        for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
            if let Some(d) = t.strip_prefix(prefix) {
                return if !d.is_empty() && d.chars().all(|c| c.is_digit(radix)) { d.chars().fold(0.0, |acc, c| acc * f64::from(radix) + f64::from(c.to_digit(radix).expect("a digit"))) } else { f64::NAN };
            }
        }
        let (sign, body) = match t.as_bytes()[0] {
            b'-' => (-1.0, &t[1..]),
            b'+' => (1.0, &t[1..]),
            _ => (1.0, t),
        };
        if body == "Infinity" {
            return sign * f64::INFINITY;
        }
        let ok = !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) && body.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.');
        if !ok {
            return f64::NAN;
        }
        body.parse::<f64>().map_or(f64::NAN, |x| sign * x)
    }

    /// `ToNumber`。
    pub fn number(&self) -> f64 {
        match self {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => f64::from(u8::from(*b)),
            Value::Number(x) => *x,
            Value::Str(s) => Self::parse_number(s),
            other => Self::parse_number(&other.string()),
        }
    }

    /// `ToString`。
    pub fn string(&self) -> String {
        match self {
            Value::Undefined => "undefined".into(),
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(x) => JsNumber(*x).to_string(),
            Value::Str(s) => s.to_string(),
            Value::Object(_) => "[object Object]".into(),
            Value::Array(a) => Self::join(&a.borrow(), ","),
            Value::Function(_) | Value::Native(_) | Value::Bound(..) => "function () { [native code] }".into(),
        }
    }

    fn join(items: &[Value], sep: &str) -> String {
        let mut out = String::new();
        for (i, v) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(sep);
            }
            if !matches!(v, Value::Undefined | Value::Null) {
                out.push_str(&v.string());
            }
        }
        out
    }

    /// `ToPrimitive`：对象都按字符串（普通对象的 `valueOf` 返回自己）。
    fn primitive(&self) -> Value {
        match self {
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::Native(_) | Value::Bound(..) => Value::from(self.string()),
            v => v.clone(),
        }
    }

    fn int32(&self) -> i32 {
        self.uint32() as i32
    }

    fn uint32(&self) -> u32 {
        let x = self.number();
        if x.is_finite() { x.trunc().rem_euclid(TWO_32) as u32 } else { 0 }
    }

    /// `IsStrictlyEqual`。
    pub fn strict_eq(&self, o: &Value) -> bool {
        match (self, o) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b),
            (Value::Array(a), Value::Array(b)) => Rc::ptr_eq(a, b),
            (Value::Function(a), Value::Function(b)) => a == b,
            (Value::Native(a), Value::Native(b)) => a == b,
            _ => false,
        }
    }

    /// `IsLooselyEqual`。
    pub fn loose_eq(&self, o: &Value) -> bool {
        if TypeOf::from(self) == TypeOf::from(o) && matches!(self, Value::Null) == matches!(o, Value::Null) {
            return self.strict_eq(o);
        }
        match (self, o) {
            (Value::Undefined | Value::Null, Value::Undefined | Value::Null) => true,
            (Value::Undefined | Value::Null, _) | (_, Value::Undefined | Value::Null) => false,
            (Value::Number(a), Value::Str(_)) => *a == o.number(),
            (Value::Str(_), Value::Number(b)) => self.number() == *b,
            (Value::Bool(_), _) => Value::Number(self.number()).loose_eq(o),
            (_, Value::Bool(_)) => self.loose_eq(&Value::Number(o.number())),
            (Value::Number(_) | Value::Str(_), _) => self.loose_eq(&o.primitive()),
            (_, Value::Number(_) | Value::Str(_)) => self.primitive().loose_eq(o),
            _ => false,
        }
    }

    /// `IsLessThan`：`None` 是有 NaN（比较结果 `undefined`）。
    fn less(&self, o: &Value) -> Option<bool> {
        let (a, b) = (self.primitive(), o.primitive());
        if let (Value::Str(x), Value::Str(y)) = (&a, &b) {
            return Some(x.encode_utf16().lt(y.encode_utf16()));
        }
        let (x, y) = (a.number(), b.number());
        if x.is_nan() || y.is_nan() { None } else { Some(x < y) }
    }

    fn binary(&self, kind: BinaryKind, o: &Value) -> Value {
        match kind {
            BinaryKind::Add => {
                let (a, b) = (self.primitive(), o.primitive());
                if matches!(a, Value::Str(_)) || matches!(b, Value::Str(_)) {
                    Value::from(a.string() + &b.string())
                } else {
                    Value::Number(a.number() + b.number())
                }
            }
            BinaryKind::Sub => Value::Number(self.number() - o.number()),
            BinaryKind::Mul => Value::Number(self.number() * o.number()),
            BinaryKind::Div => Value::Number(self.number() / o.number()),
            BinaryKind::Rem => Value::Number(self.number() % o.number()),
            BinaryKind::Exp => {
                let (b, e) = (self.number(), o.number());
                Value::Number(if e.is_nan() || (b.abs() == 1.0 && e.is_infinite()) { f64::NAN } else { b.powf(e) })
            }
            BinaryKind::Lt => Value::Bool(self.less(o) == Some(true)),
            BinaryKind::Gt => Value::Bool(o.less(self) == Some(true)),
            BinaryKind::Le => Value::Bool(o.less(self) == Some(false)),
            BinaryKind::Ge => Value::Bool(self.less(o) == Some(false)),
            BinaryKind::Eq => Value::Bool(self.loose_eq(o)),
            BinaryKind::Ne => Value::Bool(!self.loose_eq(o)),
            BinaryKind::Seq => Value::Bool(self.strict_eq(o)),
            BinaryKind::Sne => Value::Bool(!self.strict_eq(o)),
            BinaryKind::Band => Value::Number(f64::from(self.int32() & o.int32())),
            BinaryKind::Bor => Value::Number(f64::from(self.int32() | o.int32())),
            BinaryKind::Bxor => Value::Number(f64::from(self.int32() ^ o.int32())),
            BinaryKind::Shl => Value::Number(f64::from(self.int32().wrapping_shl(o.uint32() & SHIFT_MASK))),
            BinaryKind::Shr => Value::Number(f64::from(self.int32() >> (o.uint32() & SHIFT_MASK))),
            BinaryKind::Ushr => Value::Number(f64::from(self.uint32() >> (o.uint32() & SHIFT_MASK))),
        }
    }

    fn unary(&self, kind: UnaryKind) -> Value {
        match kind {
            UnaryKind::Neg => Value::Number(-self.number()),
            UnaryKind::Plus => Value::Number(self.number()),
            UnaryKind::Not => Value::Bool(!self.truthy()),
            UnaryKind::Bnot => Value::Number(f64::from(!self.int32())),
            UnaryKind::Typeof => Value::from(TypeOf::from(self).name()),
            UnaryKind::Void => Value::Undefined,
        }
    }

    /// 读属性：`o[key]`（`o.name` 的名字也作键）。
    fn get(&self, key: &Value) -> Result<Value, Error> {
        let o = self;
        match o {
            Value::Undefined | Value::Null => Err(Vm::type_error(&format!("Cannot read properties of {} (reading '{}')", o.string(), key.string()))),
            Value::Array(a) => {
                if let Some(i) = key.index() {
                    return Ok(a.borrow().get(i).cloned().unwrap_or(Value::Undefined));
                }
                let k = key.string();
                if k == "length" {
                    return Ok(Value::Number(a.borrow().len() as f64));
                }
                Ok(Native::ARRAY.iter().find(|n| n.name() == k).map_or(Value::Undefined, |n| Value::Bound(*n, Rc::new(o.clone()))))
            }
            Value::Str(s) => {
                if let Some(i) = key.index() {
                    return Ok(s.encode_utf16().nth(i).map_or(Value::Undefined, |u| Value::from(String::from_utf16_lossy(&[u]))));
                }
                match &*key.string() {
                    "length" => Ok(Value::Number(s.encode_utf16().count() as f64)),
                    "charCodeAt" => Ok(Value::Bound(Native::StringCharCodeAt, Rc::new(o.clone()))),
                    _ => Ok(Value::Undefined),
                }
            }
            Value::Number(_) if key.string() == Native::NumberToFixed.name() => Ok(Value::Bound(Native::NumberToFixed, Rc::new(o.clone()))),
            Value::Object(obj) => Ok(obj.borrow().get(&key.string()).cloned().unwrap_or(Value::Undefined)),
            _ => Ok(Value::Undefined),
        }
    }

    /// 写属性：`o[key] = v`。
    fn set(&self, key: &Value, v: Value) -> Result<(), Error> {
        let o = self;
        match o {
            Value::Undefined | Value::Null => Err(Vm::type_error(&format!("Cannot set properties of {} (setting '{}')", o.string(), key.string()))),
            Value::Array(a) => {
                let Some(i) = key.index() else {
                    return Err(Vm::type_error("only index properties of arrays are supported"));
                };
                let mut a = a.borrow_mut();
                if i >= a.len() {
                    a.resize(i + 1, Value::Undefined);
                }
                a[i] = v;
                Ok(())
            }
            Value::Object(obj) => {
                obj.borrow_mut().set(key.string().into(), v);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// 数组下标：非负整数且是规范写法的键。
    fn index(&self) -> Option<usize> {
        match self {
            Value::Number(x) if *x >= 0.0 && x.fract() == 0.0 && *x < TWO_32 - 1.0 => Some(*x as usize),
            Value::Str(s) => s.parse::<usize>().ok().filter(|i| i.to_string() == **s),
            _ => None,
        }
    }
}

/// `console.log` 的写法（node 的 `util.inspect`：一行放得下的形式）。
struct Inspect<'v>(&'v Value, usize, &'v Module);

impl Inspect<'_> {
    fn key(k: &str) -> String {
        let ident = k.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$') && k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$');
        if ident { k.to_owned() } else { Self::quote(k) }
    }

    fn quote(s: &str) -> String {
        let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
        let mut out = String::from(q);
        for c in s.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\\' => out.push_str("\\\\"),
                c if c == q => {
                    out.push('\\');
                    out.push(c);
                }
                c => out.push(c),
            }
        }
        out.push(q);
        out
    }
}

impl std::fmt::Display for Inspect<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Inspect(v, depth, m) = *self;
        match v {
            Value::Str(s) if depth > 0 => f.write_str(&Self::quote(s)),
            Value::Str(s) => f.write_str(s),
            Value::Number(x) if *x == 0.0 && x.is_sign_negative() => f.write_str("-0"),
            Value::Array(a) => {
                let a = a.borrow();
                if a.is_empty() {
                    return f.write_str("[]");
                }
                if depth > INSPECT_DEPTH {
                    return f.write_str("[Array]");
                }
                f.write_str("[ ")?;
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", Inspect(x, depth + 1, m))?;
                }
                f.write_str(" ]")
            }
            Value::Object(o) => {
                let o = o.borrow();
                if o.props.is_empty() {
                    return f.write_str("{}");
                }
                if depth > INSPECT_DEPTH {
                    return f.write_str("[Object]");
                }
                f.write_str("{ ")?;
                for (i, (k, x)) in o.props.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}: {}", Self::key(k), Inspect(x, depth + 1, m))?;
                }
                f.write_str(" }")
            }
            Value::Function(func) => {
                let name = m.functions.get(func.0 as usize).and_then(|x| m.string(x.name).ok()).unwrap_or("anonymous");
                if name.starts_with("anonymous") { f.write_str("[Function (anonymous)]") } else { write!(f, "[Function: {}]", name.split('.').next().unwrap_or(name)) }
            }
            Value::Native(n) | Value::Bound(n, _) => write!(f, "[Function: {}]", n.name()),
            v => f.write_str(&v.string()),
        }
    }
}

/// 解码后的一个函数：指令与各自的字节偏移（升序，跳转按偏移二分找）。
struct Decoded {
    at: Vec<Label>,
    insns: Vec<Insn>,
    params: u16,
    regs: u16,
}

/// ⭐ 解释器。
pub struct Vm<'m, 'o> {
    module: &'m Module,
    funcs: Vec<Decoded>,
    constants: Vec<Value>,
    globals: Object,
    out: &'o mut dyn Write,
    depth: usize,
}

impl<'m, 'o> Vm<'m, 'o> {
    pub fn new(module: &'m Module, out: &'o mut dyn Write) -> Result<Self, Error> {
        let funcs = module
            .functions
            .iter()
            .map(|f| {
                let (at, insns) = f.insns()?.into_iter().unzip();
                Ok(Decoded { at, insns, params: f.params, regs: f.regs })
            })
            .collect::<Result<_, Error>>()?;
        let constants = module
            .constants
            .iter()
            .map(|c| match c {
                Constant::Number(x) => Value::Number(*x),
                Constant::String(s) => Value::from(s.as_str()),
            })
            .collect();
        let mut globals = Object::default();
        let mut console = Object::default();
        console.set("log".into(), Value::Native(Native::ConsoleLog));
        globals.set("console".into(), Value::Object(Rc::new(RefCell::new(console))));
        let mut math = Object::default();
        for n in Native::MATH {
            math.set(n.name().into(), Value::Native(n));
        }
        math.set("PI".into(), Value::Number(std::f64::consts::PI));
        math.set("E".into(), Value::Number(std::f64::consts::E));
        globals.set("Math".into(), Value::Object(Rc::new(RefCell::new(math))));
        globals.set("globalThis".into(), Value::Undefined);
        Ok(Self { module, funcs, constants, globals, out, depth: 0 })
    }

    /// 跑入口函数。
    pub fn run(&mut self) -> Result<Value, Error> {
        self.call(&Value::Function(Func(self.module.entry)), Value::Undefined, &[])
    }

    fn constant(&self, k: Const) -> Result<&Value, Error> {
        self.constants.get(k.0 as usize).ok_or_else(|| Error(format!("no constant {}", k.0)))
    }

    fn name(&self, k: Const) -> Result<Rc<str>, Error> {
        match self.constant(k)? {
            Value::Str(s) => Ok(s.clone()),
            v => Err(Error(format!("constant {} is {v:?}, not a name", k.0))),
        }
    }

    fn type_error(what: &str) -> Error {
        Error(format!("TypeError: {what}"))
    }

    fn log(&mut self, args: &[Value]) -> Result<Value, Error> {
        let mut line = String::new();
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                line.push(' ');
            }
            write!(line, "{}", Inspect(a, 0, self.module)).expect("writing to a String");
        }
        line.push('\n');
        self.out.write_all(line.as_bytes()).map_err(|e| Error(format!("console.log: {e}")))?;
        Ok(Value::Undefined)
    }

    fn native(&mut self, n: Native, this: &Value, args: &[Value]) -> Result<Value, Error> {
        let arg = |i: usize| args.get(i).map_or(f64::NAN, Value::number);
        Ok(match n {
            Native::ConsoleLog => return self.log(args),
            Native::MathFloor => Value::Number(arg(0).floor()),
            Native::MathCeil => Value::Number(arg(0).ceil()),
            Native::MathTrunc => Value::Number(arg(0).trunc()),
            Native::MathRound => {
                let x = arg(0);
                let r = x.floor();
                Value::Number(if x - r >= 0.5 { r + 1.0 } else { r })
            }
            Native::MathSqrt => Value::Number(arg(0).sqrt()),
            Native::MathAbs => Value::Number(arg(0).abs()),
            Native::MathPow => Value::Number(arg(0)).binary(BinaryKind::Exp, &Value::Number(arg(1))),
            Native::MathMax => Value::Number(args.iter().map(Value::number).fold(f64::NEG_INFINITY, |a, b| if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) })),
            Native::MathMin => Value::Number(args.iter().map(Value::number).fold(f64::INFINITY, |a, b| if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) })),
            Native::ArrayPush | Native::ArrayPop | Native::ArrayJoin => {
                let Value::Array(a) = this else {
                    return Err(Self::type_error("not an array"));
                };
                let mut a = a.borrow_mut();
                match n {
                    Native::ArrayPush => {
                        a.extend(args.iter().cloned());
                        Value::Number(a.len() as f64)
                    }
                    Native::ArrayPop => a.pop().unwrap_or(Value::Undefined),
                    _ => {
                        let sep = args.first().filter(|s| !matches!(s, Value::Undefined)).map_or_else(|| ",".to_owned(), Value::string);
                        Value::from(Value::join(&a, &sep))
                    }
                }
            }
            Native::StringCharCodeAt => {
                let s = this.string();
                let i = args.first().map_or(0.0, Value::number);
                let i = if i.is_nan() { 0.0 } else { i.trunc() };
                if i < 0.0 { Value::Number(f64::NAN) } else { s.encode_utf16().nth(i as usize).map_or(Value::Number(f64::NAN), |u| Value::Number(f64::from(u))) }
            }
            Native::NumberToFixed => {
                let digits = args.first().map_or(0.0, Value::number);
                let digits = if digits.is_nan() { 0 } else { digits.trunc() as usize };
                Value::from(format!("{:.*}", digits, this.number()))
            }
        })
    }

    /// 调一个值。
    pub fn call(&mut self, callee: &Value, this: Value, args: &[Value]) -> Result<Value, Error> {
        let f = match callee {
            Value::Function(f) => *f,
            Value::Native(n) => return self.native(*n, &this, args),
            Value::Bound(n, recv) => return self.native(*n, recv, args),
            v => return Err(Self::type_error(&format!("{} is not a function", Inspect(v, 1, self.module)))),
        };
        if self.depth >= MAX_DEPTH {
            return Err(Error("RangeError: Maximum call stack size exceeded".into()));
        }
        self.depth += 1;
        let r = self.frame(f, this, args);
        self.depth -= 1;
        r
    }

    fn frame(&mut self, f: Func, this: Value, args: &[Value]) -> Result<Value, Error> {
        let d = self.funcs.get(f.0 as usize).ok_or_else(|| Error(format!("no function {}", f.0)))?;
        let mut regs = vec![Value::Undefined; usize::from(d.regs).max(1)];
        regs[0] = this;
        for (i, a) in args.iter().take(usize::from(d.params)).enumerate() {
            regs[i + 1] = a.clone();
        }
        let mut pc = 0;
        loop {
            let d = &self.funcs[f.0 as usize];
            let insn = d.insns.get(pc).ok_or_else(|| Error(format!("fell off function {}", f.0)))?.clone();
            pc += 1;
            let r = |x: Reg| regs[usize::from(x.0)].clone();
            let jump = |to: Label| d.at.binary_search(&to).map_err(|_| Error(format!("jump into the middle of an instruction: {}", to.0)));
            match insn {
                Insn::LoadConst { dst, k } => regs[usize::from(dst.0)] = self.constant(k)?.clone(),
                Insn::LoadLiteral { dst, kind } => regs[usize::from(dst.0)] = kind.into(),
                Insn::Move { dst, src } => regs[usize::from(dst.0)] = r(src),
                Insn::Binary { kind, dst, lhs, rhs } => regs[usize::from(dst.0)] = r(lhs).binary(kind, &r(rhs)),
                Insn::Unary { kind, dst, src } => regs[usize::from(dst.0)] = r(src).unary(kind),
                Insn::GlobalGet { dst, name } => {
                    let n = self.name(name)?;
                    regs[usize::from(dst.0)] = self.globals.get(&n).cloned().ok_or_else(|| Error(format!("ReferenceError: {n} is not defined")))?;
                }
                Insn::GlobalSet { name, src } => {
                    let n = self.name(name)?;
                    self.globals.set(n, r(src));
                }
                Insn::GetProp { dst, object, name } => {
                    let k = Value::Str(self.name(name)?);
                    regs[usize::from(dst.0)] = r(object).get(&k)?;
                }
                Insn::SetProp { object, name, src } => r(object).set(&Value::Str(self.name(name)?), r(src))?,
                Insn::GetElem { dst, object, key } => regs[usize::from(dst.0)] = r(object).get(&r(key))?,
                Insn::SetElem { object, key, src } => r(object).set(&r(key), r(src))?,
                Insn::Call { dst, callee, this, args } => {
                    let argv: Vec<Value> = args.iter().map(|a| r(*a)).collect();
                    regs[usize::from(dst.0)] = self.call(&r(callee), r(this), &argv)?;
                }
                Insn::NewObject { dst } => regs[usize::from(dst.0)] = Value::Object(Rc::default()),
                Insn::NewArray { dst, elements } => regs[usize::from(dst.0)] = Value::Array(Rc::new(RefCell::new(elements.iter().map(|e| r(*e)).collect()))),
                Insn::FuncRef { dst, func } => regs[usize::from(dst.0)] = Value::Function(func),
                Insn::Jump { to } => pc = jump(to)?,
                Insn::JumpIfTrue { cond, to } => {
                    if r(cond).truthy() {
                        pc = jump(to)?;
                    }
                }
                Insn::JumpIfFalse { cond, to } => {
                    if !r(cond).truthy() {
                        pc = jump(to)?;
                    }
                }
                Insn::Return { src } => return Ok(r(src)),
            }
        }
    }
}
