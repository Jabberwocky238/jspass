//! 字节码生成：流水线跑完的 js 方言（js op 加 `cf.br` / `cf.cond_br`）⇒ [`Module`]。
//!
//! 寄存器：`this` 与形参占 0..=参数数，其余每个 `!js.value` / `!js.cell` 值一个寄存器（不复用）；`js.truthy` 不出指令，
//! 它的 `i1` 就是操作数的寄存器，分支按真值跳。块参数在跳过去的边上用 `Move` 传（目标与来源有重叠时经临时寄存器）。
use std::collections::BTreeMap;

use mlirformat::{Attribute, Const as ConstAttr, Error, Scalar};
use mlirpass::{BlockId, Body, Heads, OpId, Symbol, ValueId};

use crate::bytecode::{Const, Constant, Func, Function, Insn, Label, Module, Reg, Writer};
use crate::{BinaryOp, FuncOp, FuncRefOp, GetPropOp, GlobalGetOp, GlobalSetOp, Kind, LiteralOp, NumberOp, SetPropOp, StringOp, UnaryOp, MAIN};

/// MLIR 字符串字面量里的转义（`llvm::printEscapedString` 的反过来）。
struct Escaped<'s>(&'s str);

impl TryFrom<Escaped<'_>> for String {
    type Error = Error;

    fn try_from(e: Escaped<'_>) -> Result<Self, Error> {
        let mut out = Vec::with_capacity(e.0.len());
        let mut it = e.0.bytes();
        while let Some(b) = it.next() {
            if b != b'\\' {
                out.push(b);
                continue;
            }
            match it.next() {
                Some(b'\\') => out.push(b'\\'),
                Some(b'"') => out.push(b'"'),
                Some(b'n') => out.push(b'\n'),
                Some(b't') => out.push(b'\t'),
                Some(h) => {
                    let l = it.next().ok_or_else(|| Error(format!("bad escape in {:?}", e.0)))?;
                    let hex = [h, l];
                    let s = core::str::from_utf8(&hex).map_err(|_| Error(format!("bad escape in {:?}", e.0)))?;
                    out.push(u8::from_str_radix(s, 16).map_err(|_| Error(format!("bad escape in {:?}", e.0)))?);
                }
                None => return Err(Error(format!("bad escape in {:?}", e.0))),
            }
        }
        String::from_utf8(out).map_err(|err| Error(format!("string attribute: {err}")))
    }
}

/// 跳转指令等着回填的目标块。
struct Fixup {
    insn: usize,
    block: BlockId,
}

/// ⭐ 整个模块的字节码生成：常量池去重、函数编号。
pub struct Emitter<'b, 'a> {
    body: &'b Body<'a>,
    module: Module,
    strings: BTreeMap<String, Const>,
    numbers: BTreeMap<u64, Const>,
    funcs: BTreeMap<String, Func>,
}

/// 一个函数的字节码生成。
struct FnEmitter<'e, 'b, 'a> {
    em: &'e mut Emitter<'b, 'a>,
    regs: BTreeMap<ValueId, Reg>,
    next: u16,
    insns: Vec<Insn>,
    starts: BTreeMap<BlockId, usize>,
    fixups: Vec<Fixup>,
}

impl<'b, 'a> Emitter<'b, 'a> {
    /// `body` 的模块里每个 `js.func` 一个函数，`@main` 是入口。
    pub fn emit(body: &'b Body<'a>) -> Result<Module, Error> {
        let mut em = Emitter { body, module: Module::default(), strings: BTreeMap::new(), numbers: BTreeMap::new(), funcs: BTreeMap::new() };
        let funcs: Vec<OpId> = body.ops(body.top()).filter(|o| Kind::try_from(&*body[*o].name).is_ok_and(|k| k == Kind::Func)).collect();
        for (i, f) in funcs.iter().enumerate() {
            let name = em.name(*f)?;
            em.funcs.insert(name, Func(u32::try_from(i).map_err(|_| Error("too many functions".into()))?));
        }
        em.module.entry = em.funcs.get(MAIN).ok_or_else(|| Error(format!("no @{MAIN}")))?.0;
        for f in funcs {
            let func = FnEmitter::emit(&mut em, f)?;
            em.module.functions.push(func);
        }
        Ok(em.module)
    }

    fn name(&self, func: OpId) -> Result<String, Error> {
        let f = self.body.head::<FuncOp>(func)?;
        Ok(Symbol::defined(&f.sym_name)?.to_owned())
    }

    fn string(&mut self, s: String) -> Const {
        if let Some(k) = self.strings.get(&s) {
            return *k;
        }
        let k = Const(self.module.constants.len() as u32);
        self.module.constants.push(Constant::String(s.clone()));
        self.strings.insert(s, k);
        k
    }

    /// 字符串属性（StrAttr）进常量池。
    fn string_attr(&mut self, a: &Attribute<'_>) -> Result<Const, Error> {
        match a {
            Attribute::Str { lit, .. } => Ok(self.string(String::try_from(Escaped(lit))?)),
            a => Err(Error(format!("{a} is not a string attribute"))),
        }
    }

    fn number(&mut self, x: f64) -> Const {
        *self.numbers.entry(x.to_bits()).or_insert_with(|| {
            let k = Const(self.module.constants.len() as u32);
            self.module.constants.push(Constant::Number(x));
            k
        })
    }
}

impl<'e, 'b, 'a> FnEmitter<'e, 'b, 'a> {
    fn emit(em: &'e mut Emitter<'b, 'a>, func: OpId) -> Result<Function, Error> {
        let body = em.body;
        let name = em.name(func)?;
        let name = em.string(name);
        let region = body[func].regions[0];
        let blocks = body[region].blocks.to_vec();
        let entry = *blocks.first().ok_or_else(|| Error("a js.func without a body".into()))?;
        let params = u16::try_from(body[entry].args.len().saturating_sub(1)).map_err(|_| Error("too many parameters".into()))?;
        let mut fe = FnEmitter { em, regs: BTreeMap::new(), next: 0, insns: Vec::new(), starts: BTreeMap::new(), fixups: Vec::new() };
        for a in &body[entry].args {
            fe.reg(*a)?;
        }
        for (i, b) in blocks.iter().enumerate() {
            fe.starts.insert(*b, fe.insns.len());
            for op in body.ops(*b) {
                fe.op(op, blocks.get(i + 1).copied())?;
            }
        }
        let code = fe.encode()?;
        Ok(Function { name, params, regs: fe.next, code })
    }

    fn reg(&mut self, v: ValueId) -> Result<Reg, Error> {
        if let Some(r) = self.regs.get(&v) {
            return Ok(*r);
        }
        let r = Reg(self.next);
        self.next = self.next.checked_add(1).ok_or_else(|| Error("more than 65535 registers".into()))?;
        self.regs.insert(v, r);
        Ok(r)
    }

    fn temp(&mut self) -> Result<Reg, Error> {
        let r = Reg(self.next);
        self.next = self.next.checked_add(1).ok_or_else(|| Error("more than 65535 registers".into()))?;
        Ok(r)
    }

    fn operand(&mut self, op: OpId, i: usize) -> Result<Reg, Error> {
        let v = self.em.body[op].operands[i];
        self.regs.get(&v).copied().ok_or_else(|| Error(format!("`{}`: operand {i} used before it is defined", self.em.body[op].name)))
    }

    fn result(&mut self, op: OpId) -> Result<Reg, Error> {
        let v = self.em.body[op].results[0];
        self.reg(v)
    }

    fn jump(&mut self, insn: Insn, block: BlockId) {
        self.fixups.push(Fixup { insn: self.insns.len(), block });
        self.insns.push(insn);
    }

    /// 跳到 `to` 的边：块参数的 `Move`，再跳（`to` 紧跟着就不跳）。
    fn edge(&mut self, args: &[ValueId], to: BlockId, next: Option<BlockId>) -> Result<(), Error> {
        let body = self.em.body;
        let dsts: Vec<Reg> = body[to].args.iter().map(|a| self.reg(*a)).collect::<Result<_, _>>()?;
        let srcs: Vec<Reg> = args.iter().map(|a| self.regs.get(a).copied().ok_or_else(|| Error("block argument used before it is defined".into()))).collect::<Result<_, _>>()?;
        let overlap = dsts.iter().enumerate().any(|(i, d)| srcs.iter().enumerate().any(|(j, s)| i != j && s == d));
        if overlap {
            let temps: Vec<Reg> = srcs.iter().map(|_| self.temp()).collect::<Result<_, _>>()?;
            for (t, s) in temps.iter().zip(&srcs) {
                self.insns.push(Insn::Move { dst: *t, src: *s });
            }
            for (d, t) in dsts.iter().zip(&temps) {
                self.insns.push(Insn::Move { dst: *d, src: *t });
            }
        } else {
            for (d, s) in dsts.iter().zip(&srcs) {
                if d != s {
                    self.insns.push(Insn::Move { dst: *d, src: *s });
                }
            }
        }
        if next != Some(to) {
            self.jump(Insn::Jump { to: Label(0) }, to);
        }
        Ok(())
    }

    fn op(&mut self, op: OpId, next: Option<BlockId>) -> Result<(), Error> {
        let body = self.em.body;
        let kind = Kind::try_from(&*body[op].name)?;
        let insn = match kind {
            Kind::Func => return Err(Error("nested js.func".into())),
            Kind::Number => {
                let c = ConstAttr::try_from(&body.head::<NumberOp>(op)?.value).map_err(|()| Error("js.number: not a float attribute".into()))?;
                let x = match c.values.first() {
                    Some(Scalar::Float(f)) => f.to_f64(),
                    _ => return Err(Error("js.number without a float".into())),
                };
                let k = self.em.number(x);
                Insn::LoadConst { dst: self.result(op)?, k }
            }
            Kind::String => {
                let k = self.em.string_attr(&body.head::<StringOp>(op)?.value)?;
                Insn::LoadConst { dst: self.result(op)?, k }
            }
            Kind::Literal => Insn::LoadLiteral { dst: self.result(op)?, kind: body.head::<LiteralOp>(op)?.kind },
            Kind::FuncRef => {
                let f = body.head::<FuncRefOp>(op)?;
                let name = Symbol::name(&f.callee)?;
                let func = *self.em.funcs.get(name).ok_or_else(|| Error(format!("no function @{name}")))?;
                Insn::FuncRef { dst: self.result(op)?, func }
            }
            Kind::Binary => Insn::Binary { kind: body.head::<BinaryOp>(op)?.kind, lhs: self.operand(op, 0)?, rhs: self.operand(op, 1)?, dst: self.result(op)? },
            Kind::Unary => Insn::Unary { kind: body.head::<UnaryOp>(op)?.kind, src: self.operand(op, 0)?, dst: self.result(op)? },
            Kind::Truthy => {
                let r = self.operand(op, 0)?;
                self.regs.insert(body[op].results[0], r);
                return Ok(());
            }
            Kind::Var => {
                self.result(op)?;
                return Ok(());
            }
            Kind::Load => Insn::Move { src: self.operand(op, 0)?, dst: self.result(op)? },
            Kind::Store => Insn::Move { dst: self.operand(op, 1)?, src: self.operand(op, 0)? },
            Kind::GlobalGet => {
                let name = self.em.string_attr(&body.head::<GlobalGetOp>(op)?.name)?;
                Insn::GlobalGet { dst: self.result(op)?, name }
            }
            Kind::GlobalSet => {
                let name = self.em.string_attr(&body.head::<GlobalSetOp>(op)?.name)?;
                Insn::GlobalSet { name, src: self.operand(op, 0)? }
            }
            Kind::GetProp => {
                let name = self.em.string_attr(&body.head::<GetPropOp>(op)?.name)?;
                Insn::GetProp { object: self.operand(op, 0)?, name, dst: self.result(op)? }
            }
            Kind::SetProp => {
                let name = self.em.string_attr(&body.head::<SetPropOp>(op)?.name)?;
                Insn::SetProp { object: self.operand(op, 0)?, name, src: self.operand(op, 1)? }
            }
            Kind::GetElem => Insn::GetElem { object: self.operand(op, 0)?, key: self.operand(op, 1)?, dst: self.result(op)? },
            Kind::SetElem => Insn::SetElem { object: self.operand(op, 0)?, key: self.operand(op, 1)?, src: self.operand(op, 2)? },
            Kind::Call => {
                let args = (2..body[op].operands.len()).map(|i| self.operand(op, i)).collect::<Result<_, _>>()?;
                Insn::Call { callee: self.operand(op, 0)?, this: self.operand(op, 1)?, args, dst: self.result(op)? }
            }
            Kind::NewObject => Insn::NewObject { dst: self.result(op)? },
            Kind::NewArray => {
                let elements = (0..body[op].operands.len()).map(|i| self.operand(op, i)).collect::<Result<_, _>>()?;
                Insn::NewArray { elements, dst: self.result(op)? }
            }
            Kind::Return => Insn::Return { src: self.operand(op, 0)? },
            Kind::Br => {
                let to = body[op].successors[0];
                let args = body[op].operands.to_vec();
                return self.edge(&args, to, next);
            }
            Kind::CondBr => {
                let (t, f) = (body[op].successors[0], body[op].successors[1]);
                let ops = body[op].operands.to_vec();
                let nt = body[t].args.len();
                let (targs, fargs) = (&ops[1..=nt], &ops[1 + nt..]);
                let cond = self.operand(op, 0)?;
                if targs.is_empty() && fargs.is_empty() {
                    if next == Some(t) {
                        self.jump(Insn::JumpIfFalse { cond, to: Label(0) }, f);
                    } else {
                        self.jump(Insn::JumpIfTrue { cond, to: Label(0) }, t);
                        if next != Some(f) {
                            self.jump(Insn::Jump { to: Label(0) }, f);
                        }
                    }
                    return Ok(());
                }
                // 带块参数：假边的 Move 放到一个只有这条边的落脚点后面
                let at = self.insns.len();
                self.insns.push(Insn::JumpIfFalse { cond, to: Label(0) });
                self.edge(targs, t, None)?;
                let landing = self.insns.len();
                self.edge(fargs, f, next)?;
                self.patch_local(at, landing);
                return Ok(());
            }
        };
        self.insns.push(insn);
        Ok(())
    }

    /// 函数内的跳转（不是跳到块）。生成时跳转目标都记成指令号，编码时换成字节偏移。
    fn patch_local(&mut self, at: usize, to: usize) {
        if let Some(l) = self.insns[at].target_mut() {
            *l = Label(u32::try_from(to).expect("insn index"));
        }
    }

    fn encode(&mut self) -> Result<Vec<u8>, Error> {
        for f in &self.fixups {
            let to = *self.starts.get(&f.block).ok_or_else(|| Error("jump to a block of another function".into()))?;
            if let Some(l) = self.insns[f.insn].target_mut() {
                *l = Label(u32::try_from(to).expect("insn index"));
            }
        }
        // 指令号 ⇒ 字节偏移：操作数定长，先量一遍
        let mut offsets = Vec::with_capacity(self.insns.len() + 1);
        let mut w = Writer::default();
        for i in &self.insns {
            offsets.push(u32::try_from(w.bytes.len()).map_err(|_| Error("function too long".into()))?);
            w.insn(i)?;
        }
        offsets.push(u32::try_from(w.bytes.len()).map_err(|_| Error("function too long".into()))?);
        let mut w = Writer::default();
        for mut i in self.insns.drain(..) {
            if let Some(l) = i.target_mut() {
                *l = Label(offsets[l.0 as usize]);
            }
            w.insn(&i)?;
        }
        Ok(w.bytes)
    }
}
