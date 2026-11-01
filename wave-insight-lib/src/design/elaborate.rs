//! Elaboration: from a top module name to an instance tree, a flat signal
//! table and driver/load relations.
//!
//! The elaborator is intentionally conservative.  Missing modules, unknown
//! ports and recursion become diagnostics; a partially resolved design is
//! still returned so the waveform viewer can work with what is there.
//!
//! Generate blocks and arrayed instances are expanded here into ordinary
//! instance/signal paths (`top.g[2].u`, `top.u[3]`).  Driver/load resolution
//! is scoped: a name written inside a generate block resolves inside the
//! enclosing blocks first, then in the instance, then absolutely, then in the
//! parent scope.  Unresolved names, unresolved `.*`/implicit connections and
//! port width mismatches become [`DiagnosticKind::UnresolvedName`] /
//! [`DiagnosticKind::WidthMismatch`] diagnostics instead of being dropped
//! silently.

use std::collections::HashMap;

use super::const_eval::{dim_width, eval_const, ParamEnv};
use super::types::{
    Connection, Diagnostic, DiagnosticKind, Direction, ElaboratedDesign, FlatSignal, InstanceDecl,
    InstanceNode, ModuleDef, NameRef, ParamAssignment, ParamValue, PortBinding, PortConnectionDecl,
    PortDef, RangeDim, SignalDef, SignalKind, SignalReference, Span, UsageKind, UsageRole,
};

const MAX_INSTANCE_DEPTH: usize = 64;
/// Upper bound on the elements one arrayed instance may expand to, so a huge
/// declaration cannot explode the flat signal table.
const MAX_ARRAY_ELEMENTS: usize = 4096;

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
        span_index: None,
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
    // The span index is built with the design: one sort per elaboration and
    // every later position query is a binary search instead of a full scan.
    design.span_index = Some(super::span_index::SpanIndex::build(&design, modules));
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
        scope: declaration
            .map(|declaration| declaration.scope.clone())
            .unwrap_or_default(),
    };

    let index = design.instances.len();
    design.instances.push(instance);

    for port in &module.ports {
        let signal_path = join_path(path, &[], &port.name);
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
        // A declaration inside a generate block carries its block path, so the
        // flat table gets "top.g[2].x" while local_name stays "x".
        let signal_path = join_path(path, &signal.scope, &signal.name);
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
        let Some(child_module) = modules.get(child_declaration.module.as_str()).copied() else {
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
        let Some(elements) = instance_elements(child_declaration, &env, &mut design.diagnostics)
        else {
            continue;
        };
        for suffix in elements {
            let leaf_name = format!("{}{}", child_declaration.name, suffix);
            let child_path = join_path(path, &child_declaration.scope, &leaf_name);
            if let Some(child) = build_instance(
                design,
                modules,
                child_module,
                &child_path,
                &leaf_name,
                Some(index),
                Some(child_decl_index),
                Some(child_declaration),
                &env,
                depth + 1,
            ) {
                children.push(child);
            }
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

/// Join an instance path, an enclosing generate-block path and a local name
/// into a flat path, e.g. "top" + ["g[2]"] + "u" -> "top.g[2].u".
fn join_path(instance_path: &str, scope: &[String], name: &str) -> String {
    let mut path = String::with_capacity(instance_path.len() + name.len() + 8);
    path.push_str(instance_path);
    for component in scope {
        path.push('.');
        path.push_str(component);
    }
    path.push('.');
    path.push_str(name);
    path
}

/// The path suffixes one instantiation expands to: [""] for a plain instance,
/// ["[3]", "[2]", "[1]", "[0]"] for the arrayed form u[3:0] (Verdi order).
///
/// Returns None when the array bounds cannot be evaluated with this instance's
/// parameters; a diagnostic is recorded in that case and the instantiation is
/// skipped instead of guessing.
fn instance_elements(
    declaration: &InstanceDecl,
    env: &ParamEnv,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<String>> {
    if declaration.array.is_empty() {
        return Some(vec![String::new()]);
    }
    let mut elements: Vec<String> = vec![String::new()];
    for dimension in &declaration.array {
        let (Some(msb), Some(lsb)) = (
            eval_const(&dimension.msb, env),
            eval_const(&dimension.lsb, env),
        ) else {
            diagnostics.push(Diagnostic::warning(
                DiagnosticKind::UnsupportedConstruct,
                format!(
                    "array bounds of instance {} are not constant; the instance was not elaborated",
                    declaration.name
                ),
                Some(declaration.span.clone()),
            ));
            return None;
        };
        let indices: Vec<i64> = if msb >= lsb {
            (lsb..=msb).rev().collect()
        } else {
            (msb..=lsb).collect()
        };
        let mut expanded = Vec::with_capacity(elements.len() * indices.len());
        for prefix in &elements {
            for index in &indices {
                expanded.push(format!("{prefix}[{index}]"));
            }
        }
        elements = expanded;
        if elements.len() > MAX_ARRAY_ELEMENTS {
            diagnostics.push(Diagnostic::warning(
                DiagnosticKind::RecursionLimit,
                format!(
                    "arrayed instance {} expands to more than {} elements; the extra elements were not elaborated",
                    declaration.name, MAX_ARRAY_ELEMENTS
                ),
                Some(declaration.span.clone()),
            ));
            elements.truncate(MAX_ARRAY_ELEMENTS);
            break;
        }
    }
    Some(elements)
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
        .filter_map(|connection| connection.port.as_deref().map(|port| (port, connection)))
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
                            bits: None,
                        }],
                        signals: Vec::new(),
                        wildcard: true,
                        implicit: false,
                    });
                }
                let text = connection
                    .expression
                    .clone()
                    .or_else(|| connection.names.first().map(|name| name.name.clone()))?;
                // The implicit form ".a" has no expression but one reference
                // (the port name); it connects a parent signal with that name.
                let implicit = connection.expression.is_none();
                Some(Connection {
                    text,
                    span: connection.span.clone(),
                    names: connection.names.clone(),
                    signals: Vec::new(),
                    wildcard: false,
                    implicit,
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

/// Link the driver/load references collected while parsing.
///
/// A name is resolved against the writing instance, then against the enclosing
/// generate blocks (a use of "x" inside "begin : g" first tries
/// "<instance>.g[i].x"), then absolutely and finally in the parent scope.  A
/// name that resolves nowhere becomes an [`DiagnosticKind::UnresolvedName`]
/// warning instead of disappearing: this is the only place a bogus source link
/// would otherwise hide.  Names that are obviously not signals (parameters,
/// instance names, system tasks and hierarchical member references) are
/// filtered to keep real designs quiet.
fn link_usages(design: &mut ElaboratedDesign, modules: &HashMap<&str, &ModuleDef>) {
    let mut diagnostics = Vec::new();
    for instance_index in 0..design.instances.len() {
        let Some(module) = modules
            .get(design.instances[instance_index].module.as_str())
            .copied()
        else {
            continue;
        };
        let instance_path = design.instances[instance_index].path.clone();
        for usage in &module.usages {
            let Some(signal_path) = resolve_scoped(
                &instance_path,
                &usage.scope,
                &usage.name,
                &design.signal_index,
            ) else {
                if should_warn_unresolved(module, &usage.name) {
                    diagnostics.push(Diagnostic::warning(
                        DiagnosticKind::UnresolvedName,
                        format!(
                            "name `{}` does not resolve to a signal in module `{}`; it was not linked as a {}",
                            usage.name,
                            module.name,
                            match usage.role {
                                UsageRole::Driver => "driver",
                                UsageRole::Load => "load",
                            }
                        ),
                        Some(usage.span.clone()),
                    ));
                }
                continue;
            };
            let Some(signal_index) = design.signal_index.get(&signal_path).copied() else {
                continue;
            };
            let reference = SignalReference {
                span: usage.span.clone(),
                kind: usage.kind,
                bits: usage.bits.clone(),
            };
            match usage.role {
                UsageRole::Driver => design.signals[signal_index].drivers.push(reference),
                UsageRole::Load => design.signals[signal_index].loads.push(reference),
            }
        }
    }
    design.diagnostics.extend(diagnostics);
}

/// True when an unresolved name is worth a warning: parameters, local instance
/// names, system tasks and hierarchical member references (`u.WIDTH`) are not
/// signals and resolving them is out of scope.
fn should_warn_unresolved(module: &ModuleDef, name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('$')
        && !name.contains('.')
        && !module
            .parameters
            .iter()
            .any(|parameter| parameter.name == name)
        && !module.genvars.iter().any(|genvar| genvar == name)
        && !module
            .instances
            .iter()
            .any(|instance| instance.name == name)
}

/// Add the driver/load references of every port connection.
///
/// A connection adds one reference per resolved *parent* signal (the signal the
/// connection reads or drives) and one for the *child port* itself, both at the
/// connection's span.  The span covers `.port(expression)` for named
/// connections (see [`PortConnectionDecl::span`]), so definition/hover on the
/// port name or on the expression answer with the connected signal; the parent
/// signal is registered first, so on the shared span it is the answer that wins
/// over the child port.
fn link_connections(design: &mut ElaboratedDesign, modules: &HashMap<&str, &ModuleDef>) {
    let mut diagnostics = Vec::new();
    for child_index in 0..design.instances.len() {
        let (parent_index, child_path) = {
            let child = &design.instances[child_index];
            let Some(parent) = child.parent else {
                continue;
            };
            (parent, child.path.clone())
        };
        let parent_path = design.instances[parent_index].path.clone();
        let parent_module_name = design.instances[parent_index].module.clone();
        let Some(parent_module) = modules.get(parent_module_name.as_str()).copied() else {
            continue;
        };
        let child_scope = design.instances[child_index].scope.clone();
        let child_leaf = design.instances[child_index].name.clone();

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
            let port_width = design.instances[child_index].ports[port_index].width;

            // Resolved parent signals keep the select as written on the
            // connection name, so a hookup of `bus[3:0]` reports that range.
            let mut resolved: Vec<(String, Option<String>)> = Vec::new();
            let mut unresolved = Vec::new();
            for name in &connection.names {
                match resolve_scoped(&parent_path, &child_scope, &name.name, &design.signal_index) {
                    Some(path) => {
                        if !resolved.iter().any(|(known, _)| *known == path) {
                            resolved.push((path, name.bits.clone()));
                        }
                    }
                    None => unresolved.push(name.name.clone()),
                }
            }
            connection.signals = resolved.iter().map(|(path, _)| path.clone()).collect();
            design.instances[child_index].ports[port_index].connection = Some(connection.clone());

            let child_port_path = format!("{}.{}", child_path, port_name);
            let (parent_roles, child_roles) = roles_for(direction);
            for (parent_signal, bits) in &resolved {
                add_reference(
                    &mut design.signals,
                    &design.signal_index,
                    parent_signal,
                    bits.as_deref(),
                    &connection.span,
                    parent_roles,
                );
            }
            add_reference(
                &mut design.signals,
                &design.signal_index,
                &child_port_path,
                None,
                &connection.span,
                child_roles,
            );

            // Unresolved connection names, including the wildcard and implicit
            // forms, used to be dropped silently; make them visible.
            for name in &unresolved {
                if should_warn_unresolved(parent_module, name) {
                    let message = if connection.wildcard {
                        format!(
                            "wildcard connection .*: module `{}` has no signal named `{}` for port `{}` of `{}`; the port was left unconnected",
                            parent_module.name, name, port_name, child_leaf
                        )
                    } else if connection.implicit {
                        format!(
                            "implicit connection .{}: module `{}` has no signal named `{}`; the port `{}` of `{}` was left unconnected",
                            port_name, parent_module.name, name, port_name, child_leaf
                        )
                    } else {
                        format!(
                            "connection .{}({}): `{}` does not resolve to a signal in `{}`; the link was dropped",
                            port_name, name, name, parent_module.name
                        )
                    };
                    diagnostics.push(Diagnostic::warning(
                        DiagnosticKind::UnresolvedName,
                        message,
                        Some(connection.span.clone()),
                    ));
                }
            }

            // Width check for the unambiguous case: a port connected to one
            // plain signal name (concatenations and expressions are not
            // width-checked, their width is out of this elaborator's scope).
            if resolved.len() == 1
                && connection.names.len() == 1
                && connection.text.trim() == connection.names[0].name.trim()
            {
                let connected = &resolved[0].0;
                if let (Some(port_width), Some(signal_index)) =
                    (port_width, design.signal_index.get(connected).copied())
                {
                    if let Some(signal_width) = design.signals[signal_index].width {
                        if signal_width != port_width {
                            diagnostics.push(Diagnostic::warning(
                                DiagnosticKind::WidthMismatch,
                                format!(
                                    "port `{}.{}` is {} bits but the connected signal `{}` is {} bits",
                                    child_leaf, port_name, port_width, connected, signal_width
                                ),
                                Some(connection.span.clone()),
                            ));
                        }
                    }
                }
            }
        }
    }
    design.diagnostics.extend(diagnostics);
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
    bits: Option<&str>,
    span: &Span,
    roles: &[UsageRole],
) {
    let Some(signal_index) = index.get(signal_path).copied() else {
        return;
    };
    let reference = SignalReference {
        span: span.clone(),
        kind: UsageKind::PortConnection,
        bits: bits.map(str::to_owned),
    };
    for role in roles {
        match role {
            UsageRole::Driver => signals[signal_index].drivers.push(reference.clone()),
            UsageRole::Load => signals[signal_index].loads.push(reference.clone()),
        }
    }
}

/// Resolve a source name written inside an instance (and, for a generate body,
/// inside its generate-block path) to a flat signal path.
///
/// The search order is: the innermost generate block first, then each enclosing
/// generate block, then the instance body itself, then an absolute path, then
/// the parent instance.  This is what makes a use of "x" inside "begin : g"
/// find "top.g[2].x" while a hierarchical "u.pc" still finds "top.u.pc".
fn resolve_scoped(
    instance_path: &str,
    scope: &[String],
    name: &str,
    index: &HashMap<String, usize>,
) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    for depth in (0..=scope.len()).rev() {
        let candidate = if depth == 0 {
            format!("{}.{}", instance_path, name)
        } else {
            format!("{}.{}.{}", instance_path, scope[..depth].join("."), name)
        };
        if index.contains_key(&candidate) {
            return Some(candidate);
        }
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
