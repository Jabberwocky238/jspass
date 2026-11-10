//! jspass 的寄存器字节码：一个模块是常量池加若干函数，函数是寄存器数、参数数与指令流。
//!
//! ```text
//!   文件   "JSBC" 版本:u16 入口:u32 常量数:u32 常量… 函数数:u32 函数…
//!   常量   0:u8 f64 位:u64 | 1:u8 长度:u32 UTF-8…
//!   函数   名字:u32（常量号） 参数数:u16 寄存器数:u16 指令字节数:u32 指令…
//!   指令   操作码:u8 操作数…；寄存器 u16、常量号 / 函数号 / 跳转目标（函数内字节偏移）u32、实参个数 u8
//! ```
//!
//! 寄存器 0 是 `this`，1..=参数数是参数，其余开始时是 `undefined`。多字节数一律小端。
use core::fmt;

use mlirformat::Error;

use crate::{BinaryKind, LiteralKind, UnaryKind};

/// 文件头的魔数。
const MAGIC: &[u8; 4] = b"JSBC";
/// 格式版本：改了编码就加一。
const VERSION: u16 = 1;
/// 常量池里数的标记。
const TAG_NUMBER: u8 = 0;
/// 常量池里字符串的标记。
const TAG_STRING: u8 = 1;

/// 寄存器号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reg(pub u16);

/// 常量池下标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Const(pub u32);

/// 模块里函数的下标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Func(pub u32);

/// 函数内的字节偏移（跳转目标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Label(pub u32);

/// 常量池的一项。数按位比（`NaN` 等于 `NaN`，`0` 不等于 `-0`）。
#[derive(Debug, Clone)]
pub enum Constant {
    Number(f64),
    String(String),
}

impl PartialEq for Constant {
    fn eq(&self, o: &Self) -> bool {
        match (self, o) {
            (Constant::Number(a), Constant::Number(b)) => a.to_bits() == b.to_bits(),
            (Constant::String(a), Constant::String(b)) => a == b,
            _ => false,
        }
    }
}

/// 操作码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    LoadConst = 0,
    LoadLiteral = 1,
    Move = 2,
    Binary = 3,
    Unary = 4,
    GlobalGet = 5,
    GlobalSet = 6,
    GetProp = 7,
    SetProp = 8,
    GetElem = 9,
    SetElem = 10,
    Call = 11,
    NewObject = 12,
    NewArray = 13,
    FuncRef = 14,
    Jump = 15,
    JumpIfTrue = 16,
    JumpIfFalse = 17,
    Return = 18,
}

impl Opcode {
    const ALL: [Opcode; 19] = [
        Opcode::LoadConst,
        Opcode::LoadLiteral,
        Opcode::Move,
        Opcode::Binary,
        Opcode::Unary,
        Opcode::GlobalGet,
        Opcode::GlobalSet,
        Opcode::GetProp,
        Opcode::SetProp,
        Opcode::GetElem,
        Opcode::SetElem,
        Opcode::Call,
        Opcode::NewObject,
        Opcode::NewArray,
        Opcode::FuncRef,
        Opcode::Jump,
        Opcode::JumpIfTrue,
        Opcode::JumpIfFalse,
        Opcode::Return,
    ];
}

impl TryFrom<u8> for Opcode {
    type Error = Error;

    fn try_from(b: u8) -> Result<Self, Error> {
        Opcode::ALL.get(usize::from(b)).copied().ok_or_else(|| Error(format!("bad opcode {b}")))
    }
}

/// 一条指令。
#[derive(Debug, Clone, PartialEq)]
pub enum Insn {
    LoadConst { dst: Reg, k: Const },
    LoadLiteral { dst: Reg, kind: LiteralKind },
    Move { dst: Reg, src: Reg },
    Binary { kind: BinaryKind, dst: Reg, lhs: Reg, rhs: Reg },
    Unary { kind: UnaryKind, dst: Reg, src: Reg },
    GlobalGet { dst: Reg, name: Const },
    GlobalSet { name: Const, src: Reg },
    GetProp { dst: Reg, object: Reg, name: Const },
    SetProp { object: Reg, name: Const, src: Reg },
    GetElem { dst: Reg, object: Reg, key: Reg },
    SetElem { object: Reg, key: Reg, src: Reg },
    Call { dst: Reg, callee: Reg, this: Reg, args: Vec<Reg> },
    NewObject { dst: Reg },
    NewArray { dst: Reg, elements: Vec<Reg> },
    FuncRef { dst: Reg, func: Func },
    Jump { to: Label },
    JumpIfTrue { cond: Reg, to: Label },
    JumpIfFalse { cond: Reg, to: Label },
    Return { src: Reg },
}

impl Insn {
    /// 指令的操作码。
    pub fn opcode(&self) -> Opcode {
        match self {
            Insn::LoadConst { .. } => Opcode::LoadConst,
            Insn::LoadLiteral { .. } => Opcode::LoadLiteral,
            Insn::Move { .. } => Opcode::Move,
            Insn::Binary { .. } => Opcode::Binary,
            Insn::Unary { .. } => Opcode::Unary,
            Insn::GlobalGet { .. } => Opcode::GlobalGet,
            Insn::GlobalSet { .. } => Opcode::GlobalSet,
            Insn::GetProp { .. } => Opcode::GetProp,
            Insn::SetProp { .. } => Opcode::SetProp,
            Insn::GetElem { .. } => Opcode::GetElem,
            Insn::SetElem { .. } => Opcode::SetElem,
            Insn::Call { .. } => Opcode::Call,
            Insn::NewObject { .. } => Opcode::NewObject,
            Insn::NewArray { .. } => Opcode::NewArray,
            Insn::FuncRef { .. } => Opcode::FuncRef,
            Insn::Jump { .. } => Opcode::Jump,
            Insn::JumpIfTrue { .. } => Opcode::JumpIfTrue,
            Insn::JumpIfFalse { .. } => Opcode::JumpIfFalse,
            Insn::Return { .. } => Opcode::Return,
        }
    }

    /// 跳转指令的目标（回填用）。
    pub fn target_mut(&mut self) -> Option<&mut Label> {
        match self {
            Insn::Jump { to } | Insn::JumpIfTrue { to, .. } | Insn::JumpIfFalse { to, .. } => Some(to),
            _ => None,
        }
    }
}

/// 一个函数。
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// 名字（常量池里的字符串）
    pub name: Const,
    pub params: u16,
    pub regs: u16,
    /// 编码后的指令流
    pub code: Vec<u8>,
}

/// 一个模块：常量池、函数、入口函数。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Module {
    pub constants: Vec<Constant>,
    pub functions: Vec<Function>,
    pub entry: u32,
}

/// 往字节流里写。
#[derive(Default)]
pub struct Writer {
    pub bytes: Vec<u8>,
}

impl Writer {
    fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    fn u16(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn reg(&mut self, r: Reg) {
        self.u16(r.0);
    }

    fn regs(&mut self, rs: &[Reg]) -> Result<(), Error> {
        self.u8(u8::try_from(rs.len()).map_err(|_| Error(format!("{} operands, at most {}", rs.len(), u8::MAX)))?);
        for r in rs {
            self.reg(*r);
        }
        Ok(())
    }

    fn len(&mut self, n: usize) -> Result<(), Error> {
        self.u32(u32::try_from(n).map_err(|_| Error(format!("length {n} does not fit u32")))?);
        Ok(())
    }

    /// 编一条指令。
    pub fn insn(&mut self, insn: &Insn) -> Result<(), Error> {
        self.u8(insn.opcode() as u8);
        match insn {
            Insn::LoadConst { dst, k } => {
                self.reg(*dst);
                self.u32(k.0);
            }
            Insn::LoadLiteral { dst, kind } => {
                self.reg(*dst);
                self.u8(*kind as u8);
            }
            Insn::Move { dst, src } => {
                self.reg(*dst);
                self.reg(*src);
            }
            Insn::Binary { kind, dst, lhs, rhs } => {
                self.u8(*kind as u8);
                self.reg(*dst);
                self.reg(*lhs);
                self.reg(*rhs);
            }
            Insn::Unary { kind, dst, src } => {
                self.u8(*kind as u8);
                self.reg(*dst);
                self.reg(*src);
            }
            Insn::GlobalGet { dst, name } => {
                self.reg(*dst);
                self.u32(name.0);
            }
            Insn::GlobalSet { name, src } => {
                self.u32(name.0);
                self.reg(*src);
            }
            Insn::GetProp { dst, object, name } => {
                self.reg(*dst);
                self.reg(*object);
                self.u32(name.0);
            }
            Insn::SetProp { object, name, src } => {
                self.reg(*object);
                self.u32(name.0);
                self.reg(*src);
            }
            Insn::GetElem { dst, object, key } => {
                self.reg(*dst);
                self.reg(*object);
                self.reg(*key);
            }
            Insn::SetElem { object, key, src } => {
                self.reg(*object);
                self.reg(*key);
                self.reg(*src);
            }
            Insn::Call { dst, callee, this, args } => {
                self.reg(*dst);
                self.reg(*callee);
                self.reg(*this);
                self.regs(args)?;
            }
            Insn::NewObject { dst } => self.reg(*dst),
            Insn::NewArray { dst, elements } => {
                self.reg(*dst);
                self.regs(elements)?;
            }
            Insn::FuncRef { dst, func } => {
                self.reg(*dst);
                self.u32(func.0);
            }
            Insn::Jump { to } => self.u32(to.0),
            Insn::JumpIfTrue { cond, to } | Insn::JumpIfFalse { cond, to } => {
                self.reg(*cond);
                self.u32(to.0);
            }
            Insn::Return { src } => self.reg(*src),
        }
        Ok(())
    }
}

/// 从字节流里读。
pub struct Reader<'b> {
    bytes: &'b [u8],
    pub at: usize,
}

impl<'b> Reader<'b> {
    pub fn new(bytes: &'b [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let s = self.bytes.get(self.at..self.at + N).ok_or_else(|| Error(format!("truncated at byte {}", self.at)))?;
        self.at += N;
        Ok(s.try_into().expect("N bytes"))
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        self.take().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        self.take().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, Error> {
        self.take().map(u64::from_le_bytes)
    }

    fn reg(&mut self) -> Result<Reg, Error> {
        self.u16().map(Reg)
    }

    fn regs(&mut self) -> Result<Vec<Reg>, Error> {
        let n = self.u8()?;
        (0..n).map(|_| self.reg()).collect()
    }

    fn bytes(&mut self, n: usize) -> Result<&'b [u8], Error> {
        let s = self.bytes.get(self.at..self.at + n).ok_or_else(|| Error(format!("truncated at byte {}", self.at)))?;
        self.at += n;
        Ok(s)
    }

    fn kind<K: TryFrom<i64>>(&mut self, what: &str) -> Result<K, Error> {
        let b = self.u8()?;
        K::try_from(i64::from(b)).map_err(|_| Error(format!("bad {what} {b}")))
    }

    /// 读一条指令。
    pub fn insn(&mut self) -> Result<Insn, Error> {
        Ok(match Opcode::try_from(self.u8()?)? {
            Opcode::LoadConst => Insn::LoadConst { dst: self.reg()?, k: Const(self.u32()?) },
            Opcode::LoadLiteral => Insn::LoadLiteral { dst: self.reg()?, kind: self.kind("literal")? },
            Opcode::Move => Insn::Move { dst: self.reg()?, src: self.reg()? },
            Opcode::Binary => Insn::Binary { kind: self.kind("binary operator")?, dst: self.reg()?, lhs: self.reg()?, rhs: self.reg()? },
            Opcode::Unary => Insn::Unary { kind: self.kind("unary operator")?, dst: self.reg()?, src: self.reg()? },
            Opcode::GlobalGet => Insn::GlobalGet { dst: self.reg()?, name: Const(self.u32()?) },
            Opcode::GlobalSet => Insn::GlobalSet { name: Const(self.u32()?), src: self.reg()? },
            Opcode::GetProp => Insn::GetProp { dst: self.reg()?, object: self.reg()?, name: Const(self.u32()?) },
            Opcode::SetProp => Insn::SetProp { object: self.reg()?, name: Const(self.u32()?), src: self.reg()? },
            Opcode::GetElem => Insn::GetElem { dst: self.reg()?, object: self.reg()?, key: self.reg()? },
            Opcode::SetElem => Insn::SetElem { object: self.reg()?, key: self.reg()?, src: self.reg()? },
            Opcode::Call => Insn::Call { dst: self.reg()?, callee: self.reg()?, this: self.reg()?, args: self.regs()? },
            Opcode::NewObject => Insn::NewObject { dst: self.reg()? },
            Opcode::NewArray => Insn::NewArray { dst: self.reg()?, elements: self.regs()? },
            Opcode::FuncRef => Insn::FuncRef { dst: self.reg()?, func: Func(self.u32()?) },
            Opcode::Jump => Insn::Jump { to: Label(self.u32()?) },
            Opcode::JumpIfTrue => Insn::JumpIfTrue { cond: self.reg()?, to: Label(self.u32()?) },
            Opcode::JumpIfFalse => Insn::JumpIfFalse { cond: self.reg()?, to: Label(self.u32()?) },
            Opcode::Return => Insn::Return { src: self.reg()? },
        })
    }
}

impl Function {
    /// 解码指令流：每条指令和它的字节偏移。
    pub fn insns(&self) -> Result<Vec<(Label, Insn)>, Error> {
        let mut r = Reader::new(&self.code);
        let mut out = Vec::new();
        while !r.done() {
            let at = Label(u32::try_from(r.at).map_err(|_| Error("function too long".into()))?);
            out.push((at, r.insn()?));
        }
        Ok(out)
    }
}

impl Module {
    /// 常量池里的字符串。
    pub fn string(&self, k: Const) -> Result<&str, Error> {
        match self.constants.get(k.0 as usize) {
            Some(Constant::String(s)) => Ok(s),
            other => Err(Error(format!("constant {} is not a string: {other:?}", k.0))),
        }
    }

    /// 编成文件。
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut w = Writer::default();
        w.bytes.extend_from_slice(MAGIC);
        w.u16(VERSION);
        w.u32(self.entry);
        w.len(self.constants.len())?;
        for c in &self.constants {
            match c {
                Constant::Number(x) => {
                    w.u8(TAG_NUMBER);
                    w.u64(x.to_bits());
                }
                Constant::String(s) => {
                    w.u8(TAG_STRING);
                    w.len(s.len())?;
                    w.bytes.extend_from_slice(s.as_bytes());
                }
            }
        }
        w.len(self.functions.len())?;
        for f in &self.functions {
            w.u32(f.name.0);
            w.u16(f.params);
            w.u16(f.regs);
            w.len(f.code.len())?;
            w.bytes.extend_from_slice(&f.code);
        }
        Ok(w.bytes)
    }
}

impl TryFrom<&[u8]> for Module {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        if r.bytes(MAGIC.len())? != MAGIC {
            return Err(Error("not a jspass bytecode file".into()));
        }
        let version = r.u16()?;
        if version != VERSION {
            return Err(Error(format!("bytecode version {version}, this jspass reads {VERSION}")));
        }
        let entry = r.u32()?;
        let constants = (0..r.u32()?)
            .map(|_| match r.u8()? {
                TAG_NUMBER => Ok(Constant::Number(f64::from_bits(r.u64()?))),
                TAG_STRING => {
                    let n = r.u32()? as usize;
                    let s = core::str::from_utf8(r.bytes(n)?).map_err(|e| Error(format!("constant string: {e}")))?;
                    Ok(Constant::String(s.to_owned()))
                }
                t => Err(Error(format!("bad constant tag {t}"))),
            })
            .collect::<Result<_, Error>>()?;
        let functions = (0..r.u32()?)
            .map(|_| {
                let (name, params, regs) = (Const(r.u32()?), r.u16()?, r.u16()?);
                let n = r.u32()? as usize;
                Ok(Function { name, params, regs, code: r.bytes(n)?.to_vec() })
            })
            .collect::<Result<_, Error>>()?;
        Ok(Module { constants, functions, entry })
    }
}

/// 反汇编：`Module` 的 `Display`。
impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, c) in self.constants.iter().enumerate() {
            match c {
                Constant::Number(x) => writeln!(f, "k{i} = {x:?}")?,
                Constant::String(s) => writeln!(f, "k{i} = {s:?}")?,
            }
        }
        for (i, func) in self.functions.iter().enumerate() {
            let name = self.string(func.name).unwrap_or("?");
            let entry = if i as u32 == self.entry { " (entry)" } else { "" };
            writeln!(f, "\nfn{i} {name}{entry}: params {}, regs {}", func.params, func.regs)?;
            match func.insns() {
                Ok(insns) => {
                    for (at, insn) in insns {
                        writeln!(f, "  {:>5}  {insn}", at.0)?;
                    }
                }
                Err(e) => writeln!(f, "  <{e}>")?,
            }
        }
        Ok(())
    }
}

impl fmt::Display for Reg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

struct Regs<'r>(&'r [Reg]);

impl fmt::Display for Regs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, r) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{r}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Insn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Insn::LoadConst { dst, k } => write!(f, "{dst} = k{}", k.0),
            Insn::LoadLiteral { dst, kind } => write!(f, "{dst} = {kind:?}"),
            Insn::Move { dst, src } => write!(f, "{dst} = {src}"),
            Insn::Binary { kind, dst, lhs, rhs } => write!(f, "{dst} = {kind:?} {lhs}, {rhs}"),
            Insn::Unary { kind, dst, src } => write!(f, "{dst} = {kind:?} {src}"),
            Insn::GlobalGet { dst, name } => write!(f, "{dst} = global[k{}]", name.0),
            Insn::GlobalSet { name, src } => write!(f, "global[k{}] = {src}", name.0),
            Insn::GetProp { dst, object, name } => write!(f, "{dst} = {object}.k{}", name.0),
            Insn::SetProp { object, name, src } => write!(f, "{object}.k{} = {src}", name.0),
            Insn::GetElem { dst, object, key } => write!(f, "{dst} = {object}[{key}]"),
            Insn::SetElem { object, key, src } => write!(f, "{object}[{key}] = {src}"),
            Insn::Call { dst, callee, this, args } => write!(f, "{dst} = call {callee}({}) this {this}", Regs(args)),
            Insn::NewObject { dst } => write!(f, "{dst} = {{}}"),
            Insn::NewArray { dst, elements } => write!(f, "{dst} = [{}]", Regs(elements)),
            Insn::FuncRef { dst, func } => write!(f, "{dst} = fn{}", func.0),
            Insn::Jump { to } => write!(f, "jump {}", to.0),
            Insn::JumpIfTrue { cond, to } => write!(f, "jump {} if {cond}", to.0),
            Insn::JumpIfFalse { cond, to } => write!(f, "jump {} unless {cond}", to.0),
            Insn::Return { src } => write!(f, "return {src}"),
        }
    }
}
