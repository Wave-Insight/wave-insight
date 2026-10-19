//! Elaboration: from a top module name to an instance tree, a flat signal
//! table and driver/load relations.
//!
//! The elaborator is intentionally conservative.  Missing modules, unknown
//! ports and recursion become diagnostics; a partially resolved design is
//! still returned so the waveform viewer can work with what is there.

use std::collections::HashMap;

use super::const_eval::{dim_width, eval_const, ParamEnv};
use super::types::{
    Connection, Diagnostic, DiagnosticKind, Direction, ElaboratedDesign, FlatSignal, InstanceDecl,
    InstanceNode, ModuleDef, NameRef, ParamAssignment, ParamValue, PortBinding, PortConnectionDecl,
    PortDef, RangeDim, SignalDef, SignalKind, SignalReference, SignalUsage, Span, UsageKind,
    UsageRole,
};

const MAX_INSTANCE_DEPTH: usize = 64;

/// Elaborate `top` from the parsed modules.
pub(crate) fn elaborate(modules: &[ModuleDef], top: &str) -> ElaboratedDesign {
    let module_map: HashMap<&str, &ModuleDef> = modules
        .iter()
        .map(|module| (module.name.as_str(), module))
        .collect();

    let mut design = ElaboratedDesign {
        top: top.to_string(),
        root: 0,
        instances: Vec::new(),
        signals: Vec::new(),
        diagnostics: Vec::new(),
        signal_index: HashMap::new(),
    };

    let Some(top_module) = module_map.get(top).copied() else {
        design.diagnostics.push(Diagnostic::error(
            DiagnosticKind::UnresolvedModule,
            format!("top module `{}` was not found", top),
            None,
        ));
        return design;
    };

    build_instance(
        &mut design,
        &module_map,
        top_module,
        top,
        top,
        None,
        None,
        None,
        &ParamEnv::new(),
        0,
    );

    link_references(&mut design, &module_map);
    design
}

#[allow(clippy::too_many_arguments)]
fn build_instance(
    design: &mut ElaboratedDesign,
    modules: &HashMap<&str, &ModuleDef>,
    module: &ModuleDef,
    path: &str,
    name: &str,
    parent: Option<usize>,
    decl_index: Option<usize>,
    declaration: Option<&InstanceDecl>,
    parent_env: &ParamEnv,
    depth: usize,
) -> Option<usize> {
    if depth > MAX_INSTANCE_DEPTH {
        design.diagnostics.push(Diagnostic::error(
            DiagnosticKind::RecursionLimit,
            format!(
                "instance hierarchy under `{}` is deeper than {} levels; it was truncated",
                path, MAX_INSTANCE_DEPTH
            ),
            None,
        ));
        return None;
    }

    let overrides: &[ParamAssignment] = declaration
        .map(|declaration| declaration.parameters.as_slice())
        .unwrap_or(&[]);
    let parameters = resolve_parameters(module, overrides, parent_env, &mut design.diagnostics);
    let env: ParamEnv = parameters
        .iter()
        .filter_map(|parameter| Some((parameter.name.clone(), parameter.value?)))
        .collect();

    let ports = build_port_bindings(module, declaration, &env, &mut design.diagnostics);

    let instance = InstanceNode {
        path: path.to_string(),
        name: name.to_string(),
        module: module.name.clone(),
        parent,
        children: Vec::new(),
        parameters,
        ports,
        span: declaration
            .map(|declaration| declaration.span.clone())
            .unwrap_or_else(|| module.span.clone()),
        module_span: module.span.clone(),
        decl_index,
    };

    let index = design.instances.len();
    design.instances.push(instance);

    for port in &module.ports {
        let signal_path = format!("{}.{}", path, port.name);
        register_signal(
            design,
            &signal_path,
            &port.name,
            path,
            &module.name,
            Some(port.direction),
            port.kind,
            width_for(port, &env),
            &port.span,
        );
    }
    for signal in &module.signals {
        let signal_path = format!("{}.{}", path, signal.name);
        register_signal(
            design,
            &signal_path,
            &signal.name,
            path,
            &module.name,
            None,
            signal.kind,
            width_for_signal(signal, &env),
            &signal.span,
        );
    }

    let mut children = Vec::new();
    for (child_decl_index, child_declaration) in module.instances.iter().enumerate() {
        let Some(child_module) = modules
            .get(child_declaration.module.as_str())
            .copied()
        else {
            design.diagnostics.push(Diagnostic::error(
                DiagnosticKind::UnresolvedModule,
                format!(
                    "module `{}` instantiated as `{}` is not defined",
                    child_declaration.module, child_declaration.name
                ),
                Some(child_declaration.module_span.clone()),
            ));
            continue;
        };
        let child_path = format!("{}.{}", path, child_declaration.name);
        if let Some(child) = build_instance(
            design,
            modules,
            child_module,
            &child_path,
            &child_declaration.name,
            Some(index),
            Some(child_decl_index),
            Some(child_declaration),
            &env,
            depth + 1,
        ) {
            children.push(child);
        }
    }
    design.instances[index].children = children;
    Some(index)
}

#[allow(clippy::too_many_arguments)]
fn register_signal(
    design: &mut ElaboratedDesign,
    path: &str,
    local_name: &str,
    instance: &str,
    module: &str,
    direction: Option<Direction>,
    kind: SignalKind,
    width: Option<u32>,
    declaration: &Span,
) {
    let index = design.signals.len();
    design.signals.push(FlatSignal {
        path: path.to_string(),
        local_name: local_name.to_string(),
        instance: instance.to_string(),
        module: module.to_string(),
        direction,
        kind,
        width,
        declaration: declaration.clone(),
        drivers: Vec::new(),
        loads: Vec::new(),
    });
    design.signal_index.insert(path.to_string(), index);
}

fn resolve_parameters(
    module: &ModuleDef,
    overrides: &[ParamAssignment],
    parent_env: &ParamEnv,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ParamValue> {
    let mut named: HashMap<&str, &ParamAssignment> = HashMap::new();
    let mut ordered: Vec<&ParamAssignment> = Vec::new();
    for assignment in overrides {
        match assignment.name.as_deref() {
            Some(name) => {
                named.insert(name, assignment);
            }
            None => ordered.push(assignment),
        }
    }

    let mut values = Vec::new();
    let mut env = ParamEnv::new();
    let mut ordered_index = 0usize;
    for parameter in &module.parameters {
        let assignment = if parameter.is_local {
            if named.contains_key(parameter.name.as_str()) {
                diagnostics.push(Diagnostic::warning(
                    DiagnosticKind::UnknownPort,
                    format!(
                        "localparam `{}` cannot be overridden; the override was ignored",
                        parameter.name
                    ),
                    Some(parameter.span.clone()),
                ));
            }
            None
        } else {
            let named_override = named.get(parameter.name.as_str()).copied();
            let ordered_override = ordered.get(ordered_index).copied();
            ordered_index += 1;
            named_override.or(ordered_override)
        };

        let (text, value) = if let Some(assignment) = assignment {
            match &assignment.value {
                Some(text) if !text.is_empty() => {
                    (Some(text.clone()), eval_const(text, parent_env))
                }
                _ => (
                    parameter.default.clone(),
                    parameter
                        .default
                        .as_deref()
                        .and_then(|text| eval_const(text, &env)),
                ),
            }
        } else {
            (
                parameter.default.clone(),
                parameter
                    .default
                    .as_deref()
                    .and_then(|text| eval_const(text, &env)),
            )
        };

        if let Some(value) = value {
            env.insert(parameter.name.clone(), value);
        }
        values.push(ParamValue {
            name: parameter.name.clone(),
            text,
            value,
            is_local: parameter.is_local,
        });
    }

    for (name, assignment) in &named {
        if !module.parameters.iter().any(|p| p.name == *name) {
            diagnostics.push(Diagnostic::error(
                DiagnosticKind::UnknownPort,
                format!("module `{}` has no parameter named `{}`", module.name, name),
                Some(assignment.span.clone()),
            ));
        }
    }
    let non_local = module.parameters.iter().filter(|p| !p.is_local).count();
    if ordered.len() > non_local {
        diagnostics.push(Diagnostic::error(
            DiagnosticKind::UnknownPort,
            format!(
                "module `{}` has {} overridable parameters but {} positional overrides were given",
                module.name,
                non_local,
                ordered.len()
            ),
            ordered
                .get(non_local)
                .map(|assignment| assignment.span.clone()),
        ));
    }

    values
}

fn build_port_bindings(
    module: &ModuleDef,
    declaration: Option<&InstanceDecl>,
    env: &ParamEnv,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<PortBinding> {
    let empty: Vec<PortConnectionDecl> = Vec::new();
    let connections: &[PortConnectionDecl] = declaration
        .map(|declaration| declaration.connections.as_slice())
        .unwrap_or(empty.as_slice());

    let named: HashMap<&str, &PortConnectionDecl> = connections
        .iter()
        .filter_map(|connection| {
            connection
                .port
                .as_deref()
                .map(|port| (port, connection))
        })
        .filter(|(port, _)| *port != "*")
        .collect();
    let wildcard = connections
        .iter()
        .find(|connection| connection.port.as_deref() == Some("*"));
    let ordered: Vec<&PortConnectionDecl> = connections
        .iter()
        .filter(|connection| connection.port.is_none())
        .collect();

    if declaration.is_some() {
        for connection in connections {
            if let Some(port) = connection.port.as_deref() {
                if port != "*" && !module.ports.iter().any(|p| p.name == port) {
                    diagnostics.push(Diagnostic::error(
                        DiagnosticKind::UnknownPort,
                        format!("module `{}` has no port named `{}`", module.name, port),
                        Some(connection.span.clone()),
                    ));
                }
            }
        }
        if ordered.len() > module.ports.len() {
            diagnostics.push(Diagnostic::error(
                DiagnosticKind::UnknownPort,
                format!(
                    "module `{}` has {} ports but {} positional connections were given",
                    module.name,
                    module.ports.len(),
                    ordered.len()
                ),
                ordered
                    .get(module.ports.len())
                    .map(|connection| connection.span.clone()),
            ));
        }
    }

    module
        .ports
        .iter()
        .enumerate()
        .map(|(port_index, port)| {
            let chosen = named
                .get(port.name.as_str())
                .copied()
                .or_else(|| ordered.get(port_index).copied())
                .or(wildcard);
            let connection = chosen.and_then(|connection| {
                if connection.port.as_deref() == Some("*") {
                    // `.*` connects a parent signal with the same name; the
                    // name is resolved during linking and simply stays
                    // unresolved when the parent has no such signal.
                    return Some(Connection {
                        text: port.name.clone(),
                        span: connection.span.clone(),
                        names: vec![NameRef {
                            name: port.name.clone(),
                            span: connection.span.clone(),
                        }],
                        signals: Vec::new(),
                    });
                }
                let text = connection
                    .expression
                    .clone()
                    .or_else(|| connection.names.first().map(|name| name.name.clone()))?;
                Some(Connection {
                    text,
                    span: connection.span.clone(),
                    names: connection.names.clone(),
                    signals: Vec::new(),
                })
            });
            PortBinding {
                port: port.name.clone(),
                direction: port.direction,
                width: width_for(port, env),
                connection,
            }
        })
        .collect()
}

fn width_for(port: &PortDef, env: &ParamEnv) -> Option<u32> {
    width_of(&port.packed, port.width, env)
}

fn width_for_signal(signal: &SignalDef, env: &ParamEnv) -> Option<u32> {
    width_of(&signal.packed, signal.width, env)
}

fn width_of(dimensions: &[RangeDim], fallback: Option<u32>, env: &ParamEnv) -> Option<u32> {
    if dimensions.is_empty() {
        return fallback;
    }
    let mut width: u32 = 1;
    for dimension in dimensions {
        width = width.checked_mul(dim_width(&dimension.msb, &dimension.lsb, env)?)?;
    }
    Some(width)
}

// ---------------------------------------------------------------------------
// Reference linking
// ---------------------------------------------------------------------------

fn link_references(design: &mut ElaboratedDesign, modules: &HashMap<&str, &ModuleDef>) {
    link_usages(design, modules);
    link_connections(design, modules);
}

fn link_usages(design: &mut ElaboratedDesign, modules: &HashMap<&str, &ModuleDef>) {
    for instance_index in 0..design.instances.len() {
        let Some(module) = modules
            .get(design.instances[instance_index].module.as_str())
            .copied()
        else {
            continue;
        };
        let instance_path = design.instances[instance_index].path.clone();
        for usage in &module.usages {
            collect_usage(design, &instance_path, usage);
        }
    }
}

fn collect_usage(design: &mut ElaboratedDesign, instance_path: &str, usage: &SignalUsage) {
    let Some(signal_path) = resolve_name(instance_path, &usage.name, &design.signal_index) else {
        return;
    };
    let Some(signal_index) = design.signal_index.get(&signal_path).copied() else {
        return;
    };
    let reference = SignalReference {
        span: usage.span.clone(),
        kind: usage.kind,
    };
    match usage.role {
        UsageRole::Driver => design.signals[signal_index].drivers.push(reference),
        UsageRole::Load => design.signals[signal_index].loads.push(reference),
    }
}

fn link_connections(design: &mut ElaboratedDesign, modules: &HashMap<&str, &ModuleDef>) {
    for child_index in 0..design.instances.len() {
        let (parent_index, decl_index, child_path) = {
            let child = &design.instances[child_index];
            let Some(parent) = child.parent else {
                continue;
            };
            let Some(decl_index) = child.decl_index else {
                continue;
            };
            (parent, decl_index, child.path.clone())
        };
        let parent_path = design.instances[parent_index].path.clone();
        let parent_module_name = design.instances[parent_index].module.clone();
        let Some(parent_module) = modules.get(parent_module_name.as_str()).copied() else {
            continue;
        };
        let Some(_declaration) = parent_module.instances.get(decl_index) else {
            continue;
        };

        let port_count = design.instances[child_index].ports.len();
        for port_index in 0..port_count {
            let Some(mut connection) = design.instances[child_index].ports[port_index]
                .connection
                .clone()
            else {
                continue;
            };
            let port_name = design.instances[child_index].ports[port_index].port.clone();
            let direction = design.instances[child_index].ports[port_index].direction;

            let mut resolved = Vec::new();
            for name in &connection.names {
                if let Some(path) = resolve_name(&parent_path, &name.name, &design.signal_index) {
                    if !resolved.contains(&path) {
                        resolved.push(path);
                    }
                }
            }
            connection.signals = resolved.clone();
            design.instances[child_index].ports[port_index].connection = Some(connection.clone());

            let child_port_path = format!("{}.{}", child_path, port_name);
            let (parent_roles, child_roles) = roles_for(direction);
            for parent_signal in &resolved {
                add_reference(
                    &mut design.signals,
                    &design.signal_index,
                    parent_signal,
                    &connection.span,
                    parent_roles,
                );
            }
            add_reference(
                &mut design.signals,
                &design.signal_index,
                &child_port_path,
                &connection.span,
                child_roles,
            );
        }
    }
}

fn roles_for(direction: Direction) -> (&'static [UsageRole], &'static [UsageRole]) {
    const DRIVER: &[UsageRole] = &[UsageRole::Driver];
    const LOAD: &[UsageRole] = &[UsageRole::Load];
    const BOTH: &[UsageRole] = &[UsageRole::Driver, UsageRole::Load];
    match direction {
        Direction::Input => (LOAD, DRIVER),
        Direction::Output => (DRIVER, LOAD),
        Direction::Inout | Direction::Ref | Direction::Unknown => (BOTH, BOTH),
    }
}

fn add_reference(
    signals: &mut [FlatSignal],
    index: &HashMap<String, usize>,
    signal_path: &str,
    span: &Span,
    roles: &[UsageRole],
) {
    let Some(signal_index) = index.get(signal_path).copied() else {
        return;
    };
    let reference = SignalReference {
        span: span.clone(),
        kind: UsageKind::PortConnection,
    };
    for role in roles {
        match role {
            UsageRole::Driver => signals[signal_index].drivers.push(reference.clone()),
            UsageRole::Load => signals[signal_index].loads.push(reference.clone()),
        }
    }
}

/// Resolve a source name inside an instance scope to a flat signal path.
fn resolve_name(
    instance_path: &str,
    name: &str,
    index: &HashMap<String, usize>,
) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let local = format!("{}.{}", instance_path, name);
    if index.contains_key(&local) {
        return Some(local);
    }
    if index.contains_key(name) {
        return Some(name.to_string());
    }
    if let Some((parent, _)) = instance_path.rsplit_once('.') {
        let candidate = format!("{}.{}", parent, name);
        if index.contains_key(&candidate) {
            return Some(candidate);
        }
    }
    None
}
