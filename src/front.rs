//! 前端：oxc 读 JavaScript 成 AST，这里把 AST 降成 js 方言加 `cf` 分支，建进一个 mlirpass [`Body`]。
//!
//! 一个函数一个 `js.func`（脚本本身是 `@main`），入口块的参数是 `this` 与形参。顶层（`@main` 最外层）的 `var` / `let` /
//! `const` / 函数声明是全局对象的属性（`js.global_*`），其余绑定是函数里的 `js.var` 格。控制流直接降成块与
//! `cf.br` / `cf.cond_br`，条件经 `js.truthy` 变 `i1`。
//!
//! 不支持的写法（闭包捕获外层局部变量、`new`、类、`try` / `throw`、解构、生成器 / `async`……）报错，带行列。
use std::collections::BTreeMap;

use mlirformat::cf::{Br, CondBr};
use mlirformat::{Attribute, Error, Op, Region, Type, Value};
use mlirpass::{BlockId, Body, Heads, OpData, OpId, Rewriter, ValueId};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionBody, ArrowFunctionExpression, AssignmentExpression, AssignmentOperator, AssignmentTarget, BinaryOperator, BindingPattern, CallExpression, Expression, ForStatementInit, FormalParameters, Function, LogicalOperator, MemberExpression, ObjectPropertyKind, PropertyKey, PropertyKind, SimpleAssignmentTarget, Statement, SwitchStatement, UnaryOperator, UpdateExpression, UpdateOperator, VariableDeclaration, VariableDeclarationKind,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

use crate::{
    BinaryKind, BinaryOp, CallOp, FuncOp, FuncRefOp, GetElemOp, GetPropOp, GlobalGetOp, GlobalSetOp, JsType, LiteralKind, LiteralOp, LoadOp, NewArrayOp, NewObjectOp, NumberOp, ReturnOp, SetElemOp, SetPropOp, StoreOp, StringOp, TruthyOp, UnaryKind, UnaryOp, VarOp,
};

/// 脚本的入口函数名。
pub const MAIN: &str = "main";

impl TryFrom<BinaryOperator> for BinaryKind {
    type Error = BinaryOperator;

    fn try_from(op: BinaryOperator) -> Result<Self, BinaryOperator> {
        Ok(match op {
            BinaryOperator::Addition => BinaryKind::Add,
            BinaryOperator::Subtraction => BinaryKind::Sub,
            BinaryOperator::Multiplication => BinaryKind::Mul,
            BinaryOperator::Division => BinaryKind::Div,
            BinaryOperator::Remainder => BinaryKind::Rem,
            BinaryOperator::Exponential => BinaryKind::Exp,
            BinaryOperator::LessThan => BinaryKind::Lt,
            BinaryOperator::LessEqualThan => BinaryKind::Le,
            BinaryOperator::GreaterThan => BinaryKind::Gt,
            BinaryOperator::GreaterEqualThan => BinaryKind::Ge,
            BinaryOperator::Equality => BinaryKind::Eq,
            BinaryOperator::Inequality => BinaryKind::Ne,
            BinaryOperator::StrictEquality => BinaryKind::Seq,
            BinaryOperator::StrictInequality => BinaryKind::Sne,
            BinaryOperator::BitwiseAnd => BinaryKind::Band,
            BinaryOperator::BitwiseOR => BinaryKind::Bor,
            BinaryOperator::BitwiseXOR => BinaryKind::Bxor,
            BinaryOperator::ShiftLeft => BinaryKind::Shl,
            BinaryOperator::ShiftRight => BinaryKind::Shr,
            BinaryOperator::ShiftRightZeroFill => BinaryKind::Ushr,
            BinaryOperator::In | BinaryOperator::Instanceof => return Err(op),
        })
    }
}

impl TryFrom<UnaryOperator> for UnaryKind {
    type Error = UnaryOperator;

    fn try_from(op: UnaryOperator) -> Result<Self, UnaryOperator> {
        Ok(match op {
            UnaryOperator::UnaryNegation => UnaryKind::Neg,
            UnaryOperator::UnaryPlus => UnaryKind::Plus,
            UnaryOperator::LogicalNot => UnaryKind::Not,
            UnaryOperator::BitwiseNot => UnaryKind::Bnot,
            UnaryOperator::Typeof => UnaryKind::Typeof,
            UnaryOperator::Void => UnaryKind::Void,
            UnaryOperator::Delete => return Err(op),
        })
    }
}

impl From<UpdateOperator> for BinaryKind {
    fn from(op: UpdateOperator) -> Self {
        match op {
            UpdateOperator::Increment => BinaryKind::Add,
            UpdateOperator::Decrement => BinaryKind::Sub,
        }
    }
}

/// 一个名字绑到哪：函数里的格，或全局对象的属性。
#[derive(Debug, Clone, Copy)]
enum Binding<'s> {
    Cell(ValueId),
    Global(&'s str),
}

/// 赋值的目标。
#[derive(Debug, Clone, Copy)]
enum Place<'s> {
    Binding(Binding<'s>),
    Prop(ValueId, &'s str),
    Elem(ValueId, ValueId),
}

/// 一个函数的源：函数声明 / 表达式，或箭头函数。
#[derive(Clone, Copy)]
enum Source<'s> {
    Function(&'s Function<'s>),
    Arrow(&'s ArrowFunctionExpression<'s>),
}

impl<'s> Source<'s> {
    fn params(self) -> &'s FormalParameters<'s> {
        match self {
            Source::Function(f) => &f.params,
            Source::Arrow(a) => &a.params,
        }
    }
}

/// 等着降的函数：符号名、源、外层函数里的局部名（用到就是闭包捕获，不支持）。
struct Pending<'s> {
    sym: String,
    source: Source<'s>,
    outer: Vec<&'s str>,
}

/// `break` / `continue` 的去处。
struct Jumps<'s> {
    label: Option<&'s str>,
    exit: BlockId,
    /// 循环的下一轮；`switch` 与带标签的块没有
    next: Option<BlockId>,
    /// 不带标签的 `break` 能不能跳出它（循环与 `switch` 能，带标签的块不能）
    breakable: bool,
}

/// 正在降的那个函数的状态。
struct Frame<'s> {
    /// 块作用域，外到内
    scopes: Vec<BTreeMap<&'s str, ValueId>>,
    jumps: Vec<Jumps<'s>>,
    /// 下一个循环 / `switch` 的标签
    label: Option<&'s str>,
    this: Option<ValueId>,
    /// 是不是 `@main`：它最外层的声明是全局的
    main: bool,
    outer: Vec<&'s str>,
}

/// ⭐ AST ⇒ js 方言：建在一个 [`Rewriter`] 上（插入点是当前块的末尾）。
pub struct Front<'r, 'a, 's> {
    rw: Rewriter<'r, 'a>,
    text: &'s str,
    file: &'s str,
    module: BlockId,
    pending: Vec<Pending<'s>>,
    symbols: BTreeMap<String, u32>,
    frame: Frame<'s>,
}

/// 读进来的 AST 与它的分配器。
pub struct Script {
    allocator: Allocator,
}

impl Script {
    pub fn new() -> Self {
        Self { allocator: Allocator::default() }
    }

    /// 读 `text`（文件名 `file` 只用于报错），降进 `body` 的模块里。
    pub fn lower(&self, body: &mut Body<'_>, text: &str, file: &str) -> Result<(), Error> {
        let ret = Parser::new(&self.allocator, text, SourceType::cjs()).parse();
        if let Some(d) = ret.diagnostics.first() {
            return Err(Error(format!("{file}: {d}")));
        }
        let program = &ret.program;
        let module = body.top();
        let mut listener = mlirpass::Quiet;
        let rw = Rewriter::new(body, &mut listener);
        let frame = Frame { scopes: Vec::new(), jumps: Vec::new(), label: None, this: None, main: true, outer: Vec::new() };
        let mut front = Front { rw, text, file, module, pending: Vec::new(), symbols: BTreeMap::new(), frame };
        front.symbols.insert(MAIN.to_owned(), 0);
        front.function(MAIN, None, &program.body, true)?;
        while let Some(p) = front.pending.pop() {
            front.frame = Frame { scopes: Vec::new(), jumps: Vec::new(), label: None, this: None, main: false, outer: p.outer };
            match p.source {
                Source::Function(f) => {
                    let body = f.body.as_ref().ok_or_else(|| front.unsupported(f.span, "a function without a body"))?;
                    front.function(&p.sym, Some(p.source), &body.statements, false)?;
                }
                Source::Arrow(a) => match &a.body {
                    ArrowFunctionBody::FunctionBody(b) => front.function(&p.sym, Some(p.source), &b.statements, false)?,
                    e => {
                        let e = e.as_expression().expect("an arrow's body is a block or an expression");
                        front.arrow_expression(&p.sym, p.source, e)?;
                    }
                },
            }
        }
        Ok(())
    }
}

impl Default for Script {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, 's> Front<'_, 'a, 's> {
    // ---- 报错 ----------------------------------------------------------------------------

    fn at(&self, span: Span) -> String {
        let before = &self.text[..(span.start as usize).min(self.text.len())];
        let line = before.matches('\n').count() + 1;
        let col = before.rfind('\n').map_or(before.len(), |i| before.len() - i - 1) + 1;
        format!("{}:{line}:{col}", self.file)
    }

    fn unsupported(&self, span: Span, what: &str) -> Error {
        Error(format!("{}: unsupported: {what}", self.at(span)))
    }

    // ---- 建 op ----------------------------------------------------------------------------

    fn value_ty() -> Type<'a> {
        JsType::Value.into()
    }

    fn v(id: ValueId) -> Value<'a> {
        Value::from(id)
    }

    /// 建一条单结果的 typed op，返回结果。
    fn emit(&mut self, op: &impl mlirpass::TypedOp) -> ValueId {
        let o = self.rw.emit(op);
        self.rw.body[o].results[0]
    }

    fn effect(&mut self, op: &impl mlirpass::TypedOp) {
        self.rw.emit(op);
    }

    fn number(&mut self, x: f64) -> ValueId {
        let attr = mlirformat::Const::scalar(mlirformat::Scalar::Float(mlirformat::ApFloat::from_f64(mlirformat::FloatKind::F64, x).unwrap_or_else(|| mlirformat::ApFloat::nan(mlirformat::FloatKind::F64))), Type::Float(mlirformat::FloatKind::F64)).into();
        self.emit(&NumberOp::new(attr, Self::value_ty()))
    }

    fn string(&mut self, s: &str) -> ValueId {
        self.emit(&StringOp::new(Attribute::str(s), Self::value_ty()))
    }

    fn literal(&mut self, kind: LiteralKind) -> ValueId {
        self.emit(&LiteralOp::new(kind, Self::value_ty()))
    }

    fn binary(&mut self, kind: BinaryKind, l: ValueId, r: ValueId) -> ValueId {
        self.emit(&BinaryOp::new(Self::v(l), Self::v(r), kind, Self::value_ty(), Self::value_ty(), Self::value_ty()))
    }

    fn unary(&mut self, kind: UnaryKind, x: ValueId) -> ValueId {
        self.emit(&UnaryOp::new(Self::v(x), kind, Self::value_ty(), Self::value_ty()))
    }

    fn cell(&mut self, name: &str) -> ValueId {
        self.emit(&VarOp::new(Attribute::str(name), JsType::Cell.into()))
    }

    fn load(&mut self, cell: ValueId) -> ValueId {
        self.emit(&LoadOp::new(Self::v(cell), JsType::Cell.into(), Self::value_ty()))
    }

    fn store(&mut self, v: ValueId, cell: ValueId) {
        self.effect(&StoreOp::new(Self::v(v), Self::v(cell), Self::value_ty(), JsType::Cell.into()));
    }

    fn func_ref(&mut self, sym: &str) -> ValueId {
        self.emit(&FuncRefOp::new(Attribute::SymbolRef(vec![Self::quoted(sym).into()]), Self::value_ty()))
    }

    /// 符号名：不是裸标识符（`$x`）的加引号。
    fn quoted(sym: &str) -> String {
        let bare = sym.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && sym.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.'));
        if bare { sym.to_owned() } else { format!("\"{sym}\"") }
    }

    fn get_prop(&mut self, o: ValueId, name: &str) -> ValueId {
        self.emit(&GetPropOp::new(Self::v(o), Attribute::str(name), Self::value_ty(), Self::value_ty()))
    }

    fn set_prop(&mut self, o: ValueId, name: &str, v: ValueId) {
        self.effect(&SetPropOp::new(Self::v(o), Self::v(v), Attribute::str(name), Self::value_ty(), Self::value_ty()));
    }

    fn get_elem(&mut self, o: ValueId, k: ValueId) -> ValueId {
        self.emit(&GetElemOp::new(Self::v(o), Self::v(k), Self::value_ty(), Self::value_ty(), Self::value_ty()))
    }

    fn set_elem(&mut self, o: ValueId, k: ValueId, v: ValueId) {
        self.effect(&SetElemOp::new(Self::v(o), Self::v(k), Self::v(v), Self::value_ty(), Self::value_ty(), Self::value_ty()));
    }

    fn global_get(&mut self, name: &str) -> ValueId {
        self.emit(&GlobalGetOp::new(Attribute::str(name), Self::value_ty()))
    }

    fn global_set(&mut self, name: &str, v: ValueId) {
        self.effect(&GlobalSetOp::new(Self::v(v), Attribute::str(name), Self::value_ty()));
    }

    fn truthy(&mut self, v: ValueId) -> ValueId {
        self.emit(&TruthyOp::new(Self::v(v), Self::value_ty(), Type::int(1)))
    }

    // ---- 块与分支 --------------------------------------------------------------------------

    fn region(&self) -> mlirpass::RegionId {
        let ip = self.rw.ip.expect("an insertion point");
        self.rw.body[ip.block].parent.expect("a block in a region")
    }

    /// 在当前函数里新开一块（插入点不动）。
    fn block(&mut self) -> BlockId {
        let r = self.region();
        self.rw.body.add_block(r, &[])
    }

    fn current(&self) -> BlockId {
        self.rw.ip.expect("an insertion point").block
    }

    fn br(&mut self, to: BlockId) {
        self.rw.create(OpData::build(Br::NAME).successors(&[to]), &[]);
    }

    fn cond_br(&mut self, cond: ValueId, t: BlockId, f: BlockId) {
        let c = self.truthy(cond);
        self.rw.create(OpData::build(CondBr::NAME).operands(&[c]).successors(&[t, f]).segments(&[1, 0, 0]), &[]);
    }

    /// 当前块结束了（`return` / `break` / 分支之后）：后面的语句进一块没人跳到的新块，规范化时删掉。
    fn dead(&mut self) {
        let b = self.block();
        self.rw.set_end(b);
    }

    fn terminated(&self, b: BlockId) -> bool {
        self.rw.body.last_op(b).is_some_and(|o| crate::Kind::try_from(&*self.rw.body[o].name).is_ok_and(|k| matches!(k, crate::Kind::Br | crate::Kind::CondBr | crate::Kind::Return)))
    }

    // ---- 函数 ----------------------------------------------------------------------------

    /// 一个不和已有函数重名的符号。
    fn fresh(&mut self, name: Option<&str>) -> String {
        let base = name.unwrap_or("anonymous").to_owned();
        let n = self.symbols.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { base } else { format!("{base}.{}", *n - 1) }
    }

    /// 外层函数里能见到的局部名（嵌套函数用到它们就是捕获）。
    fn locals(&self) -> Vec<&'s str> {
        let mut v = self.frame.outer.clone();
        v.extend(self.frame.scopes.iter().flat_map(|s| s.keys().copied()));
        v
    }

    /// 排一个嵌套函数等着降，返回它的函数对象。
    fn nested(&mut self, name: Option<&str>, source: Source<'s>) -> ValueId {
        let sym = self.fresh(name);
        let outer = self.locals();
        let f = self.func_ref(&sym);
        self.pending.push(Pending { sym, source, outer });
        f
    }

    /// 建 `js.func @sym`：入口块是 `this` 与形参，插入点移到入口块。
    fn open(&mut self, sym: &str, params: usize) -> Result<Vec<ValueId>, Error> {
        let (op, entry) = self.rw.body.container(&FuncOp::new(Attribute::str(sym), Region { blocks: Vec::new() }))?;
        self.rw.body.insert(op, self.module, None);
        let args = (0..=params).map(|_| self.rw.body.add_arg(entry, Self::value_ty())).collect();
        self.rw.set_end(entry);
        Ok(args)
    }

    /// 函数收尾：没终结的块补 `return undefined`。
    fn close(&mut self, func: OpId) {
        let region = self.rw.body[func].regions[0];
        for b in self.rw.body[region].blocks.clone() {
            if !self.terminated(b) {
                self.rw.set_end(b);
                let u = self.literal(LiteralKind::Undefined);
                self.effect(&ReturnOp::new(Self::v(u), Self::value_ty()));
            }
        }
    }

    fn function(&mut self, sym: &str, source: Option<Source<'s>>, statements: &'s [Statement<'s>], main: bool) -> Result<(), Error> {
        let params = source.map(Source::params);
        let n = params.map_or(0, |p| p.items.len());
        if let Some(p) = params.and_then(|p| p.rest.as_ref()) {
            return Err(self.unsupported(p.span, "rest parameters"));
        }
        let args = self.open(sym, n)?;
        let func = self.rw.body[self.rw.body[self.current()].parent.expect("in a region")].parent;
        self.frame.main = main;
        self.frame.this = match source {
            Some(Source::Arrow(_)) => None,
            _ => Some(args[0]),
        };
        self.frame.scopes.push(BTreeMap::new());
        if let Some(params) = params {
            for (p, arg) in params.items.iter().zip(&args[1..]) {
                if p.initializer.is_some() {
                    return Err(self.unsupported(p.span, "default parameters"));
                }
                let name = Self::ident(&p.pattern).ok_or_else(|| self.unsupported(p.span, "destructuring parameters"))?;
                let c = self.cell(name);
                self.store(*arg, c);
                self.frame.scopes[0].insert(name, c);
            }
        }
        self.hoist(statements);
        for s in statements {
            self.statement(s)?;
        }
        self.close(func);
        Ok(())
    }

    /// 表达式体的箭头函数：`(x) => x * 2`。
    fn arrow_expression(&mut self, sym: &str, source: Source<'s>, e: &'s Expression<'s>) -> Result<(), Error> {
        let params = source.params();
        let args = self.open(sym, params.items.len())?;
        let func = self.rw.body[self.rw.body[self.current()].parent.expect("in a region")].parent;
        self.frame.scopes.push(BTreeMap::new());
        for (p, arg) in params.items.iter().zip(&args[1..]) {
            let name = Self::ident(&p.pattern).ok_or_else(|| self.unsupported(p.span, "destructuring parameters"))?;
            let c = self.cell(name);
            self.store(*arg, c);
            self.frame.scopes[0].insert(name, c);
        }
        let v = self.expr(e)?;
        self.effect(&ReturnOp::new(Self::v(v), Self::value_ty()));
        self.dead();
        self.close(func);
        Ok(())
    }

    fn ident<'p>(p: &'p BindingPattern<'s>) -> Option<&'s str> {
        match p {
            BindingPattern::BindingIdentifier(id) => Some(id.name.as_str()),
            _ => None,
        }
    }

    /// 提升：函数体里（不进嵌套函数）的 `var` 与函数声明，在入口先绑好。
    fn hoist(&mut self, statements: &'s [Statement<'s>]) {
        let mut vars = Vec::new();
        let mut funcs = Vec::new();
        Self::scan(statements, &mut vars, &mut funcs);
        for name in vars {
            if self.frame.main {
                if !funcs.iter().any(|f: &&Function| f.id.as_ref().is_some_and(|i| i.name.as_str() == name)) {
                    let u = self.literal(LiteralKind::Undefined);
                    self.global_set(name, u);
                }
            } else if !self.frame.scopes[0].contains_key(name) {
                let c = self.cell(name);
                self.frame.scopes[0].insert(name, c);
            }
        }
        for f in funcs {
            let name = f.id.as_ref().expect("a declaration has a name").name.as_str();
            let v = self.nested(Some(name), Source::Function(f));
            if self.frame.main {
                self.global_set(name, v);
            } else {
                let c = if let Some(c) = self.frame.scopes[0].get(name) {
                    *c
                } else {
                    let c = self.cell(name);
                    self.frame.scopes[0].insert(name, c);
                    c
                };
                self.store(v, c);
            }
        }
    }

    fn scan(statements: &'s [Statement<'s>], vars: &mut Vec<&'s str>, funcs: &mut Vec<&'s Function<'s>>) {
        for s in statements {
            Self::scan_one(s, vars, funcs);
        }
    }

    fn scan_decl(d: &'s VariableDeclaration<'s>, vars: &mut Vec<&'s str>) {
        if d.kind == VariableDeclarationKind::Var {
            vars.extend(d.declarations.iter().filter_map(|x| Self::ident(&x.id)));
        }
    }

    fn scan_one(s: &'s Statement<'s>, vars: &mut Vec<&'s str>, funcs: &mut Vec<&'s Function<'s>>) {
        match s {
            Statement::VariableDeclaration(d) => Self::scan_decl(d, vars),
            Statement::FunctionDeclaration(f) => funcs.push(f),
            Statement::BlockStatement(b) => Self::scan(&b.body, vars, funcs),
            Statement::IfStatement(i) => {
                Self::scan_one(&i.consequent, vars, funcs);
                if let Some(a) = &i.alternate {
                    Self::scan_one(a, vars, funcs);
                }
            }
            Statement::WhileStatement(w) => Self::scan_one(&w.body, vars, funcs),
            Statement::DoWhileStatement(w) => Self::scan_one(&w.body, vars, funcs),
            Statement::ForStatement(f) => {
                if let Some(ForStatementInit::VariableDeclaration(d)) = &f.init {
                    Self::scan_decl(d, vars);
                }
                Self::scan_one(&f.body, vars, funcs);
            }
            Statement::LabeledStatement(l) => Self::scan_one(&l.body, vars, funcs),
            Statement::SwitchStatement(sw) => sw.cases.iter().for_each(|c| Self::scan(&c.consequent, vars, funcs)),
            _ => {}
        }
    }

    // ---- 名字 ----------------------------------------------------------------------------

    /// 现在是不是在 `@main` 的最外层（声明成全局）。
    fn global_level(&self) -> bool {
        self.frame.main && self.frame.scopes.len() == 1
    }

    fn lookup(&self, name: &'s str, span: Span) -> Result<Binding<'s>, Error> {
        if let Some(c) = self.frame.scopes.iter().rev().find_map(|s| s.get(name)) {
            return Ok(Binding::Cell(*c));
        }
        if self.frame.outer.contains(&name) {
            return Err(self.unsupported(span, &format!("a closure capturing `{name}`")));
        }
        Ok(Binding::Global(name))
    }

    fn declare(&mut self, name: &'s str, init: Option<ValueId>, var: bool) {
        if self.global_level() {
            let v = init.unwrap_or_else(|| self.literal(LiteralKind::Undefined));
            self.global_set(name, v);
            return;
        }
        let cell = if var {
            self.frame.scopes.iter().rev().find_map(|s| s.get(name)).copied()
        } else {
            None
        };
        let cell = cell.unwrap_or_else(|| {
            let c = self.cell(name);
            self.frame.scopes.last_mut().expect("a scope").insert(name, c);
            c
        });
        if let Some(v) = init {
            self.store(v, cell);
        } else if !var {
            let u = self.literal(LiteralKind::Undefined);
            self.store(u, cell);
        }
    }

    fn read(&mut self, p: Place<'s>) -> ValueId {
        match p {
            Place::Binding(Binding::Cell(c)) => self.load(c),
            Place::Binding(Binding::Global(n)) => self.global_get(n),
            Place::Prop(o, n) => self.get_prop(o, n),
            Place::Elem(o, k) => self.get_elem(o, k),
        }
    }

    fn write(&mut self, p: Place<'s>, v: ValueId) {
        match p {
            Place::Binding(Binding::Cell(c)) => self.store(v, c),
            Place::Binding(Binding::Global(n)) => self.global_set(n, v),
            Place::Prop(o, n) => self.set_prop(o, n, v),
            Place::Elem(o, k) => self.set_elem(o, k, v),
        }
    }

    fn member_place(&mut self, m: &'s MemberExpression<'s>) -> Result<Place<'s>, Error> {
        match m {
            MemberExpression::StaticMemberExpression(s) => {
                let o = self.expr(&s.object)?;
                Ok(Place::Prop(o, s.property.name.as_str()))
            }
            MemberExpression::ComputedMemberExpression(c) => {
                let o = self.expr(&c.object)?;
                let k = self.expr(&c.expression)?;
                Ok(Place::Elem(o, k))
            }
            MemberExpression::PrivateFieldExpression(p) => Err(self.unsupported(p.span, "private fields")),
        }
    }

    fn simple_place(&mut self, t: &'s SimpleAssignmentTarget<'s>) -> Result<Place<'s>, Error> {
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = t {
            return Ok(Place::Binding(self.lookup(id.name.as_str(), id.span)?));
        }
        match t.as_member_expression() {
            Some(m) => self.member_place(m),
            None => Err(self.unsupported(t.span(), "this assignment target")),
        }
    }

    // ---- 语句 ----------------------------------------------------------------------------

    fn statements(&mut self, ss: &'s [Statement<'s>]) -> Result<(), Error> {
        self.frame.scopes.push(BTreeMap::new());
        for s in ss {
            self.statement(s)?;
        }
        self.frame.scopes.pop();
        Ok(())
    }

    fn statement(&mut self, s: &'s Statement<'s>) -> Result<(), Error> {
        let label = self.frame.label.take();
        match s {
            Statement::ExpressionStatement(e) => {
                self.expr(&e.expression)?;
            }
            Statement::VariableDeclaration(d) => self.var_decl(d)?,
            Statement::FunctionDeclaration(_) | Statement::EmptyStatement(_) => {}
            Statement::BlockStatement(b) => self.statements(&b.body)?,
            Statement::ReturnStatement(r) => {
                let v = match &r.argument {
                    Some(e) => self.expr(e)?,
                    None => self.literal(LiteralKind::Undefined),
                };
                self.effect(&ReturnOp::new(Self::v(v), Self::value_ty()));
                self.dead();
            }
            Statement::IfStatement(i) => {
                let c = self.expr(&i.test)?;
                let (then, join) = (self.block(), self.block());
                let other = if i.alternate.is_some() { self.block() } else { join };
                self.cond_br(c, then, other);
                self.rw.set_end(then);
                self.scoped(&i.consequent)?;
                self.br(join);
                if let Some(a) = &i.alternate {
                    self.rw.set_end(other);
                    self.scoped(a)?;
                    self.br(join);
                }
                self.sink(join);
            }
            Statement::WhileStatement(w) => {
                let (head, body, exit) = (self.block(), self.block(), self.block());
                self.br(head);
                self.rw.set_end(head);
                let c = self.expr(&w.test)?;
                self.cond_br(c, body, exit);
                self.rw.set_end(body);
                self.frame.jumps.push(Jumps { label, exit, next: Some(head), breakable: true });
                self.scoped(&w.body)?;
                self.frame.jumps.pop();
                self.br(head);
                self.sink(exit);
            }
            Statement::DoWhileStatement(w) => {
                let (body, test, exit) = (self.block(), self.block(), self.block());
                self.br(body);
                self.rw.set_end(body);
                self.frame.jumps.push(Jumps { label, exit, next: Some(test), breakable: true });
                self.scoped(&w.body)?;
                self.frame.jumps.pop();
                self.br(test);
                self.rw.set_end(test);
                let c = self.expr(&w.test)?;
                self.cond_br(c, body, exit);
                self.sink(exit);
            }
            Statement::ForStatement(f) => {
                self.frame.scopes.push(BTreeMap::new());
                match &f.init {
                    Some(ForStatementInit::VariableDeclaration(d)) => self.var_decl(d)?,
                    Some(e) => {
                        self.expr(e.as_expression().expect("a for init is a declaration or an expression"))?;
                    }
                    None => {}
                }
                let (head, body, step, exit) = (self.block(), self.block(), self.block(), self.block());
                self.br(head);
                self.rw.set_end(head);
                match &f.test {
                    Some(t) => {
                        let c = self.expr(t)?;
                        self.cond_br(c, body, exit);
                    }
                    None => self.br(body),
                }
                self.rw.set_end(body);
                self.frame.jumps.push(Jumps { label, exit, next: Some(step), breakable: true });
                self.scoped(&f.body)?;
                self.frame.jumps.pop();
                self.br(step);
                self.rw.set_end(step);
                if let Some(u) = &f.update {
                    self.expr(u)?;
                }
                self.br(head);
                self.sink(exit);
                self.frame.scopes.pop();
            }
            Statement::SwitchStatement(sw) => self.switch(sw, label)?,
            Statement::LabeledStatement(l) => {
                let name = l.label.name.as_str();
                if matches!(l.body, Statement::WhileStatement(_) | Statement::DoWhileStatement(_) | Statement::ForStatement(_) | Statement::SwitchStatement(_)) {
                    self.frame.label = Some(name);
                    self.statement(&l.body)?;
                } else {
                    let exit = self.block();
                    self.frame.jumps.push(Jumps { label: Some(name), exit, next: None, breakable: false });
                    self.statement(&l.body)?;
                    self.frame.jumps.pop();
                    self.br(exit);
                    self.sink(exit);
                }
            }
            Statement::BreakStatement(b) => {
                let want = b.label.as_ref().map(|l| l.name.as_str());
                let to = self.frame.jumps.iter().rev().find(|j| match want { Some(w) => j.label == Some(w), None => j.breakable }).map(|j| j.exit);
                let to = to.ok_or_else(|| self.unsupported(b.span, "break outside a loop or switch"))?;
                self.br(to);
                self.dead();
            }
            Statement::ContinueStatement(c) => {
                let want = c.label.as_ref().map(|l| l.name.as_str());
                let to = self.frame.jumps.iter().rev().find(|j| j.next.is_some() && want.is_none_or(|w| j.label == Some(w))).and_then(|j| j.next);
                let to = to.ok_or_else(|| self.unsupported(c.span, "continue outside a loop"))?;
                self.br(to);
                self.dead();
            }
            other => return Err(self.unsupported(other.span(), "this statement")),
        }
        Ok(())
    }

    /// 一条语句自成块作用域（`if` / 循环体）。
    fn scoped(&mut self, s: &'s Statement<'s>) -> Result<(), Error> {
        self.frame.scopes.push(BTreeMap::new());
        let r = self.statement(s);
        self.frame.scopes.pop();
        r
    }

    /// 跳到 `join` 并接着往 `join` 里写（`join` 挪到函数的最后，块的次序跟着源码走）。
    fn sink(&mut self, join: BlockId) {
        let r = self.region();
        let blocks = &mut self.rw.body[r].blocks;
        if let Some(i) = blocks.iter().position(|b| *b == join) {
            let b = blocks.remove(i);
            blocks.push(b);
        }
        self.rw.set_end(join);
    }

    fn var_decl(&mut self, d: &'s VariableDeclaration<'s>) -> Result<(), Error> {
        let var = d.kind == VariableDeclarationKind::Var;
        if !matches!(d.kind, VariableDeclarationKind::Var | VariableDeclarationKind::Let | VariableDeclarationKind::Const) {
            return Err(self.unsupported(d.span, "this declaration kind"));
        }
        for x in &d.declarations {
            let name = Self::ident(&x.id).ok_or_else(|| self.unsupported(x.span, "destructuring"))?;
            let init = match &x.init {
                Some(e) => Some(self.named(e, name)?),
                None => None,
            };
            if var && init.is_none() {
                continue;
            }
            self.declare(name, init, var);
        }
        Ok(())
    }

    /// `switch`：按 `===` 逐个比，命中跳进对应的 case 体，体之间顺次落下去。
    fn switch(&mut self, sw: &'s SwitchStatement<'s>, label: Option<&'s str>) -> Result<(), Error> {
        let d = self.expr(&sw.discriminant)?;
        let exit = self.block();
        let bodies: Vec<BlockId> = sw.cases.iter().map(|_| self.block()).collect();
        let mut default = None;
        for (i, c) in sw.cases.iter().enumerate() {
            match &c.test {
                Some(t) => {
                    let v = self.expr(t)?;
                    let eq = self.binary(BinaryKind::Seq, d, v);
                    let next = self.block();
                    self.cond_br(eq, bodies[i], next);
                    self.rw.set_end(next);
                }
                None => default = Some(bodies[i]),
            }
        }
        self.br(default.unwrap_or(exit));
        self.frame.jumps.push(Jumps { label, exit, next: None, breakable: true });
        self.frame.scopes.push(BTreeMap::new());
        for (i, c) in sw.cases.iter().enumerate() {
            self.rw.set_end(bodies[i]);
            for s in &c.consequent {
                self.statement(s)?;
            }
            self.br(bodies.get(i + 1).copied().unwrap_or(exit));
        }
        self.frame.scopes.pop();
        self.frame.jumps.pop();
        self.sink(exit);
        Ok(())
    }

    // ---- 表达式 --------------------------------------------------------------------------

    /// 绑到名字上的表达式：匿名函数取这个名字（`NamedEvaluation`：`const f = () => 1` 的 `f.name` 是 `"f"`）。
    fn named(&mut self, e: &'s Expression<'s>, name: &'s str) -> Result<ValueId, Error> {
        match e.without_parentheses() {
            Expression::FunctionExpression(f) if f.id.is_none() && !f.generator && !f.r#async => Ok(self.nested(Some(name), Source::Function(f))),
            Expression::ArrowFunctionExpression(a) if !a.r#async => Ok(self.nested(Some(name), Source::Arrow(a))),
            e => self.expr(e),
        }
    }

    fn expr(&mut self, e: &'s Expression<'s>) -> Result<ValueId, Error> {
        Ok(match e {
            Expression::NumericLiteral(n) => self.number(n.value),
            Expression::StringLiteral(s) => {
                let text = s.value.as_str().ok_or_else(|| self.unsupported(s.span, "a string with a lone surrogate"))?;
                self.string(text)
            }
            Expression::BooleanLiteral(b) => self.literal(if b.value { LiteralKind::True } else { LiteralKind::False }),
            Expression::NullLiteral(_) => self.literal(LiteralKind::Null),
            Expression::TemplateLiteral(t) => {
                let cooked = |i: usize| t.quasis[i].value.cooked.and_then(|c| c.as_str());
                let first = cooked(0).ok_or_else(|| self.unsupported(t.span, "this template"))?;
                let mut acc = self.string(first);
                for (i, x) in t.expressions.iter().enumerate() {
                    let v = self.expr(x)?;
                    let s = self.string("");
                    let v = self.binary(BinaryKind::Add, s, v);
                    acc = self.binary(BinaryKind::Add, acc, v);
                    let q = cooked(i + 1).ok_or_else(|| self.unsupported(t.span, "this template"))?;
                    if !q.is_empty() {
                        let q = self.string(q);
                        acc = self.binary(BinaryKind::Add, acc, q);
                    }
                }
                acc
            }
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                match self.lookup(name, id.span)? {
                    Binding::Cell(c) => self.load(c),
                    Binding::Global(_) if name == "undefined" => self.literal(LiteralKind::Undefined),
                    Binding::Global(_) if name == "NaN" => self.number(f64::NAN),
                    Binding::Global(_) if name == "Infinity" => self.number(f64::INFINITY),
                    Binding::Global(n) => self.global_get(n),
                }
            }
            Expression::ThisExpression(t) => match self.frame.this {
                Some(v) => v,
                None => return Err(self.unsupported(t.span, "`this` in an arrow function")),
            },
            Expression::ParenthesizedExpression(p) => self.expr(&p.expression)?,
            Expression::SequenceExpression(s) => {
                let mut last = None;
                for x in &s.expressions {
                    last = Some(self.expr(x)?);
                }
                last.expect("a sequence has an expression")
            }
            Expression::BinaryExpression(b) => {
                let kind = BinaryKind::try_from(b.operator).map_err(|_| self.unsupported(b.span, "`in` / `instanceof`"))?;
                let l = self.expr(&b.left)?;
                let r = self.expr(&b.right)?;
                self.binary(kind, l, r)
            }
            Expression::UnaryExpression(u) => {
                let kind = UnaryKind::try_from(u.operator).map_err(|_| self.unsupported(u.span, "`delete`"))?;
                let x = self.expr(&u.argument)?;
                self.unary(kind, x)
            }
            Expression::LogicalExpression(l) => {
                let tmp = self.cell("");
                let a = self.expr(&l.left)?;
                self.store(a, tmp);
                let (rhs, join) = (self.block(), self.block());
                match l.operator {
                    LogicalOperator::And => self.cond_br(a, rhs, join),
                    LogicalOperator::Or => self.cond_br(a, join, rhs),
                    LogicalOperator::Coalesce => {
                        let n = self.literal(LiteralKind::Null);
                        let nullish = self.binary(BinaryKind::Eq, a, n);
                        self.cond_br(nullish, rhs, join);
                    }
                }
                self.rw.set_end(rhs);
                let b = self.expr(&l.right)?;
                self.store(b, tmp);
                self.br(join);
                self.sink(join);
                self.load(tmp)
            }
            Expression::ConditionalExpression(c) => {
                let tmp = self.cell("");
                let t = self.expr(&c.test)?;
                let (yes, no, join) = (self.block(), self.block(), self.block());
                self.cond_br(t, yes, no);
                self.rw.set_end(yes);
                let a = self.expr(&c.consequent)?;
                self.store(a, tmp);
                self.br(join);
                self.rw.set_end(no);
                let b = self.expr(&c.alternate)?;
                self.store(b, tmp);
                self.br(join);
                self.sink(join);
                self.load(tmp)
            }
            Expression::AssignmentExpression(a) => self.assign(a)?,
            Expression::UpdateExpression(u) => self.update(u)?,
            Expression::CallExpression(c) => self.call(c)?,
            Expression::ArrayExpression(a) => {
                let mut elems = Vec::with_capacity(a.elements.len());
                for x in &a.elements {
                    match x {
                        ArrayExpressionElement::SpreadElement(s) => return Err(self.unsupported(s.span, "spread")),
                        ArrayExpressionElement::Elision(s) => return Err(self.unsupported(s.span, "array holes")),
                        x => elems.push(Self::v(self.expr(x.as_expression().expect("an element is an expression"))?)),
                    }
                }
                self.emit(&NewArrayOp::new(elems, Self::value_ty(), Self::value_ty()))
            }
            Expression::ObjectExpression(o) => {
                let obj = self.emit(&NewObjectOp::new(Self::value_ty()));
                for p in &o.properties {
                    let ObjectPropertyKind::ObjectProperty(p) = p else {
                        return Err(self.unsupported(p.span(), "spread properties"));
                    };
                    if p.kind != PropertyKind::Init || p.method {
                        return Err(self.unsupported(p.span, "getters, setters and methods"));
                    }
                    let v = match &p.key {
                        PropertyKey::StaticIdentifier(id) => self.named(&p.value, id.name.as_str())?,
                        _ => self.expr(&p.value)?,
                    };
                    match &p.key {
                        PropertyKey::StaticIdentifier(id) => self.set_prop(obj, id.name.as_str(), v),
                        PropertyKey::StringLiteral(s) if !p.computed => {
                            let k = s.value.as_str().ok_or_else(|| self.unsupported(s.span, "a key with a lone surrogate"))?;
                            self.set_prop(obj, k, v);
                        }
                        PropertyKey::PrivateIdentifier(id) => return Err(self.unsupported(id.span, "private names")),
                        k => {
                            let key = self.expr(k.as_expression().expect("a key is an expression"))?;
                            self.set_elem(obj, key, v);
                        }
                    }
                }
                obj
            }
            Expression::FunctionExpression(f) => {
                if f.generator || f.r#async {
                    return Err(self.unsupported(f.span, "generators and async functions"));
                }
                self.nested(f.id.as_ref().map(|i| i.name.as_str()), Source::Function(f))
            }
            Expression::ArrowFunctionExpression(a) => {
                if a.r#async {
                    return Err(self.unsupported(a.span, "async functions"));
                }
                self.nested(None, Source::Arrow(a))
            }
            e => match e.as_member_expression() {
                Some(m) => {
                    let p = self.member_place(m)?;
                    self.read(p)
                }
                None => return Err(self.unsupported(e.span(), "this expression")),
            },
        })
    }

    fn assign(&mut self, a: &'s AssignmentExpression<'s>) -> Result<ValueId, Error> {
        let (AssignmentTarget::AssignmentTargetIdentifier(_) | AssignmentTarget::StaticMemberExpression(_) | AssignmentTarget::ComputedMemberExpression(_)) = &a.left else {
            return Err(self.unsupported(a.span, "destructuring assignment"));
        };
        let place = self.simple_place(a.left.as_simple_assignment_target().expect("a simple target"))?;
        let v = match a.operator {
            AssignmentOperator::Assign => match &a.left {
                AssignmentTarget::AssignmentTargetIdentifier(id) => self.named(&a.right, id.name.as_str())?,
                _ => self.expr(&a.right)?,
            },
            op => match op.to_binary_operator() {
                Some(b) => {
                    let kind = BinaryKind::try_from(b).map_err(|_| self.unsupported(a.span, "this operator"))?;
                    let old = self.read(place);
                    let r = self.expr(&a.right)?;
                    self.binary(kind, old, r)
                }
                None => return Err(self.unsupported(a.span, "logical assignment")),
            },
        };
        self.write(place, v);
        Ok(v)
    }

    fn update(&mut self, u: &'s UpdateExpression<'s>) -> Result<ValueId, Error> {
        let place = self.simple_place(&u.argument)?;
        let old = self.read(place);
        let old = self.unary(UnaryKind::Plus, old);
        let one = self.number(1.0);
        let new = self.binary(u.operator.into(), old, one);
        self.write(place, new);
        Ok(if u.prefix { new } else { old })
    }

    fn call(&mut self, c: &'s CallExpression<'s>) -> Result<ValueId, Error> {
        if c.optional {
            return Err(self.unsupported(c.span, "optional calls"));
        }
        let (callee, this) = if let Some(m) = c.callee.as_member_expression() {
            match self.member_place(m)? {
                p @ (Place::Prop(o, _) | Place::Elem(o, _)) => (self.read(p), o),
                Place::Binding(_) => unreachable!("a member is a property"),
            }
        } else {
            let f = self.expr(&c.callee)?;
            (f, self.literal(LiteralKind::Undefined))
        };
        let mut args = Vec::with_capacity(c.arguments.len());
        for a in &c.arguments {
            match a {
                Argument::SpreadElement(s) => return Err(self.unsupported(s.span, "spread arguments")),
                a => args.push(Self::v(self.expr(a.as_expression().expect("an argument is an expression"))?)),
            }
        }
        Ok(self.emit(&CallOp::new(Self::v(callee), Self::v(this), args, Self::value_ty(), Self::value_ty(), Self::value_ty(), Self::value_ty())))
    }
}
