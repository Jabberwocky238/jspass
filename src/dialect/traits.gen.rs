//! 由 `scripts/regen.py` 从 `td/JsOps.td` 生成，不手改。

use mlirformat::{Effect, EffectKind, EffectOn, Effects, OpInfo, OpInfos, Trait};

/// js 各 op 的 trait、接口、内存效应与固有属性。
pub fn register(infos: &mut OpInfos) {
    infos.insert(OpInfo { name: "js.binary", traits: &[], effects: Effects::Unknown, inherent: &["kind"] });
    infos.insert(OpInfo { name: "js.call", traits: &[], effects: Effects::Unknown, inherent: &[] });
    infos.insert(OpInfo { name: "js.func", traits: &[Trait::IsIsolatedFromAbove, Trait::SymbolOpInterface], effects: Effects::Unknown, inherent: &["sym_name"] });
    infos.insert(OpInfo { name: "js.func_ref", traits: &[Trait::AlwaysSpeculatableImplTrait, Trait::ConditionallySpeculatable, Trait::MemoryEffectOpInterface], effects: Effects::Known(&[]), inherent: &["callee"] });
    infos.insert(OpInfo { name: "js.get_elem", traits: &[], effects: Effects::Unknown, inherent: &[] });
    infos.insert(OpInfo { name: "js.get_prop", traits: &[], effects: Effects::Unknown, inherent: &["name"] });
    infos.insert(OpInfo { name: "js.global_get", traits: &[], effects: Effects::Unknown, inherent: &["name"] });
    infos.insert(OpInfo { name: "js.global_set", traits: &[], effects: Effects::Unknown, inherent: &["name"] });
    infos.insert(OpInfo { name: "js.literal", traits: &[Trait::AlwaysSpeculatableImplTrait, Trait::ConditionallySpeculatable, Trait::ConstantLike, Trait::MemoryEffectOpInterface], effects: Effects::Known(&[]), inherent: &["kind"] });
    infos.insert(OpInfo { name: "js.load", traits: &[Trait::MemoryEffectOpInterface], effects: Effects::Known(&[Effect { kind: EffectKind::Read, on: EffectOn::Operand(0), resource: "DefaultResource" }]), inherent: &[] });
    infos.insert(OpInfo { name: "js.new_array", traits: &[], effects: Effects::Unknown, inherent: &[] });
    infos.insert(OpInfo { name: "js.new_object", traits: &[], effects: Effects::Unknown, inherent: &[] });
    infos.insert(OpInfo { name: "js.number", traits: &[Trait::AlwaysSpeculatableImplTrait, Trait::ConditionallySpeculatable, Trait::ConstantLike, Trait::MemoryEffectOpInterface], effects: Effects::Known(&[]), inherent: &["value"] });
    infos.insert(OpInfo { name: "js.return", traits: &[Trait::AlwaysSpeculatableImplTrait, Trait::ConditionallySpeculatable, Trait::IsTerminator, Trait::MemoryEffectOpInterface, Trait::RegionBranchTerminatorOpInterface, Trait::ReturnLike], effects: Effects::Known(&[]), inherent: &[] });
    infos.insert(OpInfo { name: "js.set_elem", traits: &[], effects: Effects::Unknown, inherent: &[] });
    infos.insert(OpInfo { name: "js.set_prop", traits: &[], effects: Effects::Unknown, inherent: &["name"] });
    infos.insert(OpInfo { name: "js.store", traits: &[Trait::MemoryEffectOpInterface], effects: Effects::Known(&[Effect { kind: EffectKind::Write, on: EffectOn::Operand(1), resource: "DefaultResource" }]), inherent: &[] });
    infos.insert(OpInfo { name: "js.string", traits: &[Trait::AlwaysSpeculatableImplTrait, Trait::ConditionallySpeculatable, Trait::ConstantLike, Trait::MemoryEffectOpInterface], effects: Effects::Known(&[]), inherent: &["value"] });
    infos.insert(OpInfo { name: "js.truthy", traits: &[Trait::AlwaysSpeculatableImplTrait, Trait::ConditionallySpeculatable, Trait::MemoryEffectOpInterface], effects: Effects::Known(&[]), inherent: &[] });
    infos.insert(OpInfo { name: "js.unary", traits: &[], effects: Effects::Unknown, inherent: &["kind"] });
    infos.insert(OpInfo { name: "js.var", traits: &[Trait::MemoryEffectOpInterface], effects: Effects::Known(&[Effect { kind: EffectKind::Allocate, on: EffectOn::Result(0), resource: "DefaultResource" }]), inherent: &["name"] });
}
