//! Adapter from the new model to the old [`Module`] / [`ModuleValue`] model.
//!
//! The front end still consumes these two types, so they have to keep working.
//! They are a *bridge*, not the target design:
//!
//! * the tree keeps its shape (one [`Module`] per scope, signals keyed by their
//!   local name),
//! * `Signal::value_key` becomes the full hierarchical path instead of the VCD
//!   identifier code, because wellen does not preserve the identifier,
//! * `Signal::size` is the declared bit width,
//! * `Module::end_clock`/`ModuleValue` time stamps are `i32`, so times above
//!   `i32::MAX` are clamped,
//! * real, string and event variables are skipped, the old model cannot show
//!   them,
//! * everything is expanded into `(u8, u8)` chunks - that is why big dumps used
//!   to explode and why new code should use the streaming API instead.

use crate::data_struct::{CodeLocation, Module, ModuleValue, Signal};

use super::{time_to_i32, HierarchyView, Result, ScopeNode, SignalKind, Waveform};

/// Convert the hierarchy into the nested old [`Module`] tree.
///
/// `end_clock` is only set on the root module, exactly like the old
/// `vcd_parser` did (it assigned `end_clock` once, on the top level `Module`);
/// sub modules keep the `0` that [`Module::new`] gives them. The front end reads
/// the root's value, so this keeps the adapter a drop-in replacement.
pub(crate) fn build_module(hierarchy: &HierarchyView, end_clock: i32) -> Module {
    let mut root = build_scope_node(hierarchy.root(), hierarchy);
    root.end_clock = end_clock;
    root
}

fn build_scope_node(node: &ScopeNode, hierarchy: &HierarchyView) -> Module {
    let mut module = Module::new();

    for &index in node.signals() {
        let Some(info) = hierarchy.signal(index) else {
            continue;
        };
        let mut signal = Signal::new();
        signal.size = info.width() as usize;
        signal.value_key = info.path().to_string();
        signal.location_define = CodeLocation::new();
        module.signal.insert(info.name().to_string(), signal);
    }

    for child in node.children() {
        module
            .sub_module
            .insert(child.name().to_string(), build_scope_node(child, hierarchy));
    }

    module
}

/// Decode every change of every bit vector signal into the old layout.
///
/// Keys of the resulting map are [`crate::waveform::SignalInfo::path`]s.
pub(crate) fn build_module_value(waveform: &Waveform) -> Result<ModuleValue> {
    let mut values = ModuleValue::new();

    for info in waveform.signals() {
        //the old model only knows two/four state bit vectors
        if info.kind() != SignalKind::BitVector {
            continue;
        }
        for (time, value) in waveform.all_changes(info.index())? {
            let Some(chunk) = value.to_legacy_chunk() else {
                continue;
            };
            values.new_clk(time_to_i32(time));
            values.insert(info.path().to_string(), chunk);
        }
    }

    Ok(values)
}
