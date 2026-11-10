//! `jspass` 命令行。
//!
//! ```text
//!   jspass lower   <a.js>            js 方言（前端刚出来的）
//!   jspass opt     <a.js|a.mlir>     跑完流水线的 js 方言（--pass-pipeline=... 换流水线）
//!   jspass compile <a.js> [-o a.jsbc] 字节码文件（缺省 a.jsbc）
//!   jspass dis     <a.js|a.jsbc>     反汇编
//!   jspass run     <a.js|a.jsbc>     解释执行
//! ```
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jspass::{Emitter, Jspass, Module, Vm};
use mlirformat::Error;

/// 字节码文件的扩展名。
const BYTECODE_EXT: &str = "jsbc";
/// MLIR 文本的扩展名。
const MLIR_EXT: &str = "mlir";
/// 换流水线的选项前缀。
const PIPELINE_FLAG: &str = "--pass-pipeline=";
/// 指定输出的选项。
const OUTPUT_FLAG: &str = "-o";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Lower,
    Opt,
    Compile,
    Dis,
    Run,
}

impl TryFrom<&str> for Command {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self, Error> {
        Ok(match s {
            "lower" => Command::Lower,
            "opt" => Command::Opt,
            "compile" => Command::Compile,
            "dis" => Command::Dis,
            "run" => Command::Run,
            other => return Err(Error(format!("unknown command `{other}` (lower | opt | compile | dis | run)"))),
        })
    }
}

/// 输入文件按扩展名分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Js,
    Mlir,
    Bytecode,
}

impl From<&Path> for Input {
    fn from(p: &Path) -> Self {
        match p.extension().and_then(|e| e.to_str()) {
            Some(BYTECODE_EXT) => Input::Bytecode,
            Some(MLIR_EXT) => Input::Mlir,
            _ => Input::Js,
        }
    }
}

struct Cli {
    command: Command,
    input: PathBuf,
    output: Option<PathBuf>,
    pipeline: Option<String>,
}

impl TryFrom<&[String]> for Cli {
    type Error = Error;

    fn try_from(args: &[String]) -> Result<Self, Error> {
        let mut it = args.iter();
        let command = Command::try_from(it.next().ok_or_else(|| Error("usage: jspass <lower|opt|compile|dis|run> <file> [-o out] [--pass-pipeline=...]".into()))?.as_str())?;
        let (mut input, mut output, mut pipeline) = (None, None, None);
        while let Some(a) = it.next() {
            if let Some(p) = a.strip_prefix(PIPELINE_FLAG) {
                pipeline = Some(p.to_owned());
            } else if a == OUTPUT_FLAG {
                output = Some(PathBuf::from(it.next().ok_or_else(|| Error("-o needs a file".into()))?));
            } else {
                input = Some(PathBuf::from(a));
            }
        }
        Ok(Cli { command, input: input.ok_or_else(|| Error("no input file".into()))?, output, pipeline })
    }
}

impl Cli {
    fn read(&self) -> Result<String, Error> {
        std::fs::read_to_string(&self.input).map_err(|e| Error(format!("{}: {e}", self.input.display())))
    }

    fn file(&self) -> String {
        self.input.display().to_string()
    }

    /// 字节码：`.jsbc` 直接读，`.js` 现编。
    fn module(&self, js: &Jspass) -> Result<Module, Error> {
        match Input::from(self.input.as_path()) {
            Input::Bytecode => {
                let bytes = std::fs::read(&self.input).map_err(|e| Error(format!("{}: {e}", self.input.display())))?;
                Module::try_from(bytes.as_slice())
            }
            Input::Mlir => {
                let text = self.read()?;
                let mut body = js.parse(&text)?;
                js.optimize(&mut body, self.pipeline.as_deref())?;
                Emitter::emit(&body)
            }
            Input::Js => js.compile(&self.read()?, &self.file()),
        }
    }

    fn run(&self) -> Result<(), Error> {
        let js = Jspass::new();
        let mut stdout = std::io::stdout().lock();
        match self.command {
            Command::Lower => {
                let body = js.lower(&self.read()?, &self.file())?;
                write!(stdout, "{}", js.show(&body)).map_err(|e| Error(e.to_string()))?;
            }
            Command::Opt => {
                let text = self.read()?;
                let mut body = match Input::from(self.input.as_path()) {
                    Input::Mlir => js.parse(&text)?,
                    _ => js.lower(&text, &self.file())?,
                };
                js.optimize(&mut body, self.pipeline.as_deref())?;
                write!(stdout, "{}", js.show(&body)).map_err(|e| Error(e.to_string()))?;
            }
            Command::Compile => {
                let m = self.module(&js)?;
                let out = self.output.clone().unwrap_or_else(|| self.input.with_extension(BYTECODE_EXT));
                std::fs::write(&out, m.encode()?).map_err(|e| Error(format!("{}: {e}", out.display())))?;
            }
            Command::Dis => write!(stdout, "{}", self.module(&js)?).map_err(|e| Error(e.to_string()))?,
            Command::Run => {
                let m = self.module(&js)?;
                Vm::new(&m, &mut stdout)?.run()?;
            }
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match Cli::try_from(args.as_slice()).and_then(|c| c.run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jspass: {e}");
            ExitCode::FAILURE
        }
    }
}
