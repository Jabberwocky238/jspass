//! 对拍：`tests/js/*.js` 编成字节码、编码再解码、解释执行，输出与同名 `.out`（node 跑出来的）逐字节比；
//! 跑完流水线的 IR 打成文本再读回来，出的字节码要和直接出的一样。
use std::path::Path;

use jspass::{Emitter, Jspass, Module, Vm};

#[test]
fn scripts_match_node() {
    let js = Jspass::new();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/js");
    let mut cases: Vec<_> = std::fs::read_dir(&dir).expect("tests/js").filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "js")).collect();
    cases.sort();
    assert!(!cases.is_empty(), "no cases in {}", dir.display());
    let mut failed = Vec::new();
    for path in &cases {
        let name = path.file_name().expect("a file").to_string_lossy().into_owned();
        let src = std::fs::read_to_string(path).expect("read the case");
        let want = std::fs::read_to_string(path.with_extension("out")).expect("the case's .out");
        let mut body = js.lower(&src, &name).unwrap_or_else(|e| panic!("{name}: {e}"));
        js.optimize(&mut body, None).unwrap_or_else(|e| panic!("{name}: {e}"));
        let direct = Emitter::emit(&body).unwrap_or_else(|e| panic!("{name}: {e}"));
        let text = js.show(&body);
        let reread = js.parse(&text).unwrap_or_else(|e| panic!("{name}: printed IR does not read back: {e}\n{text}"));
        assert!(Emitter::emit(&reread).unwrap_or_else(|e| panic!("{name}: {e}")) == direct, "{name}: bytecode differs after a text round trip");
        let bytes = direct.encode().unwrap_or_else(|e| panic!("{name}: {e}"));
        let module = Module::try_from(bytes.as_slice()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(module == direct, "{name}: bytecode differs after encode / decode");
        let mut out = Vec::new();
        let r = Vm::new(&module, &mut out).and_then(|mut vm| vm.run());
        let got = String::from_utf8(out).expect("utf-8 output");
        if let Err(e) = r {
            failed.push(format!("{name}: {e}\n--- got so far\n{got}"));
        } else if got != want {
            failed.push(format!("{name}:\n--- want\n{want}--- got\n{got}"));
        }
    }
    assert!(failed.is_empty(), "{} of {} cases differ from node:\n{}", failed.len(), cases.len(), failed.join("\n"));
}
