#!/usr/bin/env python3
"""从 td/JsOps.td 重新生成 js 方言：src/dialect/js.gen.rs（op 结构体）与 src/dialect/traits.gen.rs（trait / 内存效应 / 固有属性）。

    python3 scripts/regen.py --mlirformat <mlirformat 0.6.5 可执行文件> --mlirformat-src <mlirformat 0.6.5 源码> \
        --mlir-include <llvm-project 22.1.8 的 mlir/include>

记录经 `mlirformat tblgen --dump-json` 读，trait 名只留 mlirformat 的 `Trait` 枚举认得的（复用 mlirformat 的 scripts/traits.py）。
"""
import argparse
import importlib.util
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TD = ROOT / "td" / "JsOps.td"
OUT = ROOT / "src" / "dialect"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mlirformat", required=True)
    ap.add_argument("--mlirformat-src", required=True)
    ap.add_argument("--mlir-include", required=True)
    o = ap.parse_args()
    inc = ["-I", o.mlir_include, "-I", str(TD.parent)]
    subprocess.run([o.mlirformat, "tblgen", str(TD), *inc, "-o", str(OUT)], check=True)

    src = Path(o.mlirformat_src)
    spec = importlib.util.spec_from_file_location("traits", src / "scripts" / "traits.py")
    traits_py = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(traits_py)
    known = set(re.findall(r"^\s+(\w+),$", (src / "src" / "traits.gen.rs").read_text().split("traits! {", 1)[1].split("}", 1)[0], re.M))
    out = subprocess.run([o.mlirformat, "tblgen", "--dump-json", str(TD), *inc], capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(out.stderr.strip())
    ops = {}
    for name, traits, effects, inherent, *_ in traits_py.Records(json.loads(out.stdout)).ops():
        if name.startswith("js."):
            ops[name] = (sorted(t for t in traits if t in known), effects, inherent)
    lines = [
        "//! 由 `scripts/regen.py` 从 `td/JsOps.td` 生成，不手改。",
        "",
        "use mlirformat::{Effect, EffectKind, EffectOn, Effects, OpInfo, OpInfos, Trait};",
        "",
        "/// js 各 op 的 trait、接口、内存效应与固有属性。",
        "pub fn register(infos: &mut OpInfos) {",
    ]
    for name in sorted(ops):
        traits, effects, inherent = ops[name]
        ts = ", ".join(f"Trait::{t}" for t in traits)
        if effects is None:
            eff = "Effects::Unknown"
        elif effects == "custom":
            eff = "Effects::Custom"
        else:
            on = lambda x: "EffectOn::Op" if x == "Op" else "EffectOn::" + x
            es = ", ".join(f"Effect {{ kind: EffectKind::{k}, on: {on(w)}, resource: \"{r}\" }}" for k, w, r in effects)
            eff = f"Effects::Known(&[{es}])"
        attrs = ", ".join(f'"{a}"' for a in inherent)
        lines.append(f"    infos.insert(OpInfo {{ name: \"{name}\", traits: &[{ts}], effects: {eff}, inherent: &[{attrs}] }});")
    lines.append("}")
    (OUT / "traits.gen.rs").write_text("\n".join(lines) + "\n")
    print(f"{len(ops)} ops")


if __name__ == "__main__":
    main()
