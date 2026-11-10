//! JavaScript ⇒ 寄存器字节码，中间经 MLIR：oxc 读源码，前端降成 `js` 方言（`td/JsOps.td`）加 `cf` 分支，mlirpass 跑
//! 规范化与公共子表达式消除，再生成字节码。
//!
//! ```text
//!   JavaScript ──oxc──▶ AST ──Front──▶ js + cf ──mlirpass: canonicalize, cse──▶ Emitter ──▶ bytecode::Module ──▶ Vm
//! ```
mod bytecode;
mod dialect;
mod emit;
mod front;
mod vm;

use mlirformat::{Error, MlirOpt};
use mlirpass::{Body, Ctx, Interfaces, PassManager, Passes};

pub use bytecode::{Const, Constant, Func, Function, Insn, Label, Module, Opcode, Reg};
pub use dialect::*;
pub use emit::Emitter;
pub use front::{Front, MAIN, Script};
pub use vm::{JsNumber, Native, Value, Vm};

/// 前端之后、生成字节码之前跑的流水线。
pub const PIPELINE: &str = "builtin.module(canonicalize, cse, canonicalize)";

/// ⭐ 一套装好 js 方言的 mlirpass 上下文：读 JavaScript、跑流水线、出字节码。
pub struct Jspass {
    opt: MlirOpt,
    interfaces: Interfaces<'static>,
    passes: Passes,
}

impl Jspass {
    pub fn new() -> Self {
        let mut opt = MlirOpt::upstream();
        JsDialect::load(&mut opt);
        Self { opt, interfaces: Interfaces::upstream(), passes: Passes::upstream() }
    }

    pub fn opt(&self) -> &MlirOpt {
        &self.opt
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx::new(&self.opt, &self.interfaces)
    }

    /// 源码 ⇒ js 方言（还没跑流水线）。`file` 只用于报错。
    pub fn lower(&self, text: &str, file: &str) -> Result<Body<'_>, Error> {
        let mut body = Body::new(self.ctx());
        Script::new().lower(&mut body, text, file)?;
        Ok(body)
    }

    /// 读 MLIR 文本（js 方言）。
    pub fn parse<'s>(&'s self, text: &'s str) -> Result<Body<'s>, Error> {
        Body::parse(self.ctx(), text)
    }

    /// 跑 `pipeline`（缺省 [`PIPELINE`]）。
    pub fn optimize(&self, body: &mut Body<'_>, pipeline: Option<&str>) -> Result<(), Error> {
        let pm = PassManager::parse(&self.passes, pipeline.unwrap_or(PIPELINE))?;
        let root = body.root;
        pm.run(body, root)
    }

    /// IR 的文本。
    pub fn show(&self, body: &Body<'_>) -> String {
        body.show(&self.opt, false, |_| true)
    }

    /// 源码 ⇒ 字节码。
    pub fn compile(&self, text: &str, file: &str) -> Result<Module, Error> {
        let mut body = self.lower(text, file)?;
        self.optimize(&mut body, None)?;
        Emitter::emit(&body)
    }
}

impl Default for Jspass {
    fn default() -> Self {
        Self::new()
    }
}
