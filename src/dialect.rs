//! `js` 方言：`td/JsOps.td` 经 `mlirformat tblgen` 生成 op（`js.gen.rs`）与 trait 表（`traits.gen.rs`），两个文件不手改，
//! 改 td 后跑 `scripts/regen.py`。这里只补生成不了的：两种类型、op 名到 [`Kind`] 的对照、装进 `MlirOpt`。
#[path = "dialect/js.gen.rs"]
mod js;
#[path = "dialect/traits.gen.rs"]
mod traits;

use mlirformat::cf::{Br, CondBr};
use mlirformat::{Error, Extended, MlirOpt, Op, Type};

pub use js::*;

/// js 的两种类型：值、绑定格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsType {
    /// `!js.value`：任意 JavaScript 值
    Value,
    /// `!js.cell`：一个参数 / `var` / `let` / `const`
    Cell,
}

impl JsType {
    const fn name(self) -> &'static str {
        match self {
            JsType::Value => "js.value",
            JsType::Cell => "js.cell",
        }
    }
}

impl From<JsType> for Type<'_> {
    fn from(t: JsType) -> Self {
        Type::Extended(Extended { name: t.name().into(), body: None, params: Vec::new() })
    }
}

impl JsDialect {
    /// 往 `opt` 里装 js 方言（op 的语法与 trait / 内存效应）。
    pub fn load(opt: &mut MlirOpt) {
        opt.load(JsDialect);
        traits::register(opt.infos_mut());
    }
}

/// 字节码生成认得的 op：js 的全部，加 `cf.br` / `cf.cond_br`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Func,
    Return,
    Number,
    String,
    Literal,
    FuncRef,
    Binary,
    Unary,
    Truthy,
    Var,
    Load,
    Store,
    GlobalGet,
    GlobalSet,
    GetProp,
    SetProp,
    GetElem,
    SetElem,
    Call,
    NewObject,
    NewArray,
    Br,
    CondBr,
}

impl Kind {
    const TABLE: [(Kind, &'static str); 23] = [
        (Kind::Func, FuncOp::NAME),
        (Kind::Return, ReturnOp::NAME),
        (Kind::Number, NumberOp::NAME),
        (Kind::String, StringOp::NAME),
        (Kind::Literal, LiteralOp::NAME),
        (Kind::FuncRef, FuncRefOp::NAME),
        (Kind::Binary, BinaryOp::NAME),
        (Kind::Unary, UnaryOp::NAME),
        (Kind::Truthy, TruthyOp::NAME),
        (Kind::Var, VarOp::NAME),
        (Kind::Load, LoadOp::NAME),
        (Kind::Store, StoreOp::NAME),
        (Kind::GlobalGet, GlobalGetOp::NAME),
        (Kind::GlobalSet, GlobalSetOp::NAME),
        (Kind::GetProp, GetPropOp::NAME),
        (Kind::SetProp, SetPropOp::NAME),
        (Kind::GetElem, GetElemOp::NAME),
        (Kind::SetElem, SetElemOp::NAME),
        (Kind::Call, CallOp::NAME),
        (Kind::NewObject, NewObjectOp::NAME),
        (Kind::NewArray, NewArrayOp::NAME),
        (Kind::Br, Br::NAME),
        (Kind::CondBr, CondBr::NAME),
    ];

    /// op 名。
    pub fn name(self) -> &'static str {
        Self::TABLE.iter().find(|(k, _)| *k == self).map(|(_, n)| *n).expect("every kind is in the table")
    }
}

impl TryFrom<&str> for Kind {
    type Error = Error;

    fn try_from(name: &str) -> Result<Self, Error> {
        Self::TABLE.iter().find(|(_, n)| *n == name).map(|(k, _)| *k).ok_or_else(|| Error(format!("`{name}` is not an op jspass lowers")))
    }
}
