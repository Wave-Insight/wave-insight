//! Sorted span index for the "what is at this source position" queries.
//!
//! DesignDb::item_at and DesignDb::signals_at used to walk every module, signal
//! and driver/load reference on each request, and the LSP asks those questions
//! on every keystroke.  This module builds, once per elaborated design, a
//! per-file sorted structure of the same spans the scanner considered: spans
//! are mapped to a numeric key (line * width + column), entries are sorted by
//! interval start and stored in an augmented segment tree of the maximum
//! interval end, so a query only visits the entries that can contain the point.
//! The old consideration order is kept as a tie breaker (size, order), which
//! keeps every answer identical to the brute-force scan.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::{ElaboratedDesign, ModuleDef, SourceItem, Span};

/// The index for one elaborated design, split by source file.
#[derive(Clone, Debug, Default)]
pub(crate) struct SpanIndex {
    files: HashMap<PathBuf, FileIndex>,
}

#[derive(Clone, Debug)]
struct IndexedItem {
    start: u64,
    end: u64,
    size: u64,
    order: u32,
    item: SourceItem,
}

#[derive(Clone, Debug)]
struct IndexedSignal {
    start: u64,
    end: u64,
    order: u32,
    path: String,
}

/// Per-file index: intervals sorted by start plus a segment tree of their
/// maximum end, and the same structure for the signal-path query.
#[derive(Clone, Debug, Default)]
struct FileIndex {
    width: u64,
    items: Vec<IndexedItem>,
    item_max_end: Vec<u64>,
    signals: Vec<IndexedSignal>,
    signal_max_end: Vec<u64>,
}

struct PendingItem {
    span: Span,
    size: u64,
    order: u32,
    item: SourceItem,
}

struct PendingSignal {
    span: Span,
    order: u32,
    path: String,
}

impl SpanIndex {
    /// Build the index from an elaborated design and the module database.
    /// Entry order is exactly the order the old scanner considered items.
    pub(crate) fn build(design: &ElaboratedDesign, modules: &[ModuleDef]) -> Self {
        let mut pending_items: HashMap<PathBuf, Vec<PendingItem>> = HashMap::new();
        let mut pending_signals: HashMap<PathBuf, Vec<PendingSignal>> = HashMap::new();
        let mut order: u32 = 0;

        let mut push_item = |span: &Span, item: SourceItem, order: &mut u32| {
            pending_items
                .entry(span.file.clone())
                .or_default()
                .push(PendingItem {
                    span: span.clone(),
                    size: span.size_key(),
                    order: *order,
                    item,
                });
            *order = order.saturating_add(1);
        };

        // 1. module level declarations, same loop order as the scanner.
        for module in modules {
            push_item(
                &module.span,
                SourceItem::Module {
                    name: module.name.clone(),
                    span: module.span.clone(),
                },
                &mut order,
            );
            for port in &module.ports {
                push_item(
                    &port.span,
                    SourceItem::Port {
                        module: module.name.clone(),
                        name: port.name.clone(),
                        span: port.span.clone(),
                    },
                    &mut order,
                );
                if let Some(header_span) = &port.header_span {
                    push_item(
                        header_span,
                        SourceItem::Port {
                            module: module.name.clone(),
                            name: port.name.clone(),
                            span: header_span.clone(),
                        },
                        &mut order,
                    );
                }
            }
            for signal in &module.signals {
                push_item(
                    &signal.span,
                    SourceItem::Signal {
                        module: module.name.clone(),
                        name: signal.name.clone(),
                        span: signal.span.clone(),
                    },
                    &mut order,
                );
            }
            for declaration in &module.instances {
                push_item(
                    &declaration.module_span,
                    SourceItem::Module {
                        name: declaration.module.clone(),
                        span: declaration.module_span.clone(),
                    },
                    &mut order,
                );
                push_item(
                    &declaration.span,
                    SourceItem::Instance {
                        path: format!("{}.{}", module.name, declaration.name),
                        module: declaration.module.clone(),
                        span: declaration.span.clone(),
                    },
                    &mut order,
                );
            }
        }

        // 2. elaborated instances.
        for instance in design.instances() {
            push_item(
                &instance.module_span,
                SourceItem::Module {
                    name: instance.module.clone(),
                    span: instance.module_span.clone(),
                },
                &mut order,
            );
            push_item(
                &instance.span,
                SourceItem::Instance {
                    path: instance.path.clone(),
                    module: instance.module.clone(),
                    span: instance.span.clone(),
                },
                &mut order,
            );
        }

        // 3. flat signal declarations and driver/load references.
        for (signal_index, signal) in design.signals().iter().enumerate() {
            let signal_order = signal_index as u32;
            let mut push_signal = |span: &Span| {
                pending_signals
                    .entry(span.file.clone())
                    .or_default()
                    .push(PendingSignal {
                        span: span.clone(),
                        order: signal_order,
                        path: signal.path.clone(),
                    });
            };
            push_signal(&signal.declaration);
            for driver in &signal.drivers {
                push_signal(&driver.span);
            }
            for load in &signal.loads {
                push_signal(&load.span);
            }

            push_item(
                &signal.declaration,
                SourceItem::Signal {
                    module: signal.module.clone(),
                    name: signal.local_name.clone(),
                    span: signal.declaration.clone(),
                },
                &mut order,
            );
            for driver in &signal.drivers {
                push_item(
                    &driver.span,
                    SourceItem::Driver {
                        signal: signal.path.clone(),
                        span: driver.span.clone(),
                        kind: driver.kind,
                    },
                    &mut order,
                );
            }
            for load in &signal.loads {
                push_item(
                    &load.span,
                    SourceItem::Load {
                        signal: signal.path.clone(),
                        span: load.span.clone(),
                        kind: load.kind,
                    },
                    &mut order,
                );
            }
        }

        let mut files: HashMap<PathBuf, FileIndex> = HashMap::new();
        for (path, items) in pending_items {
            let file = files.entry(path).or_default();
            file.width = KEY_WIDTH;
            file.items = items
                .into_iter()
                .map(|pending| IndexedItem {
                    start: key(&pending.span, KEY_WIDTH, true),
                    end: key(&pending.span, KEY_WIDTH, false),
                    size: pending.size,
                    order: pending.order,
                    item: pending.item,
                })
                .collect();
            file.items.sort_by_key(|entry| entry.start);
            file.item_max_end =
                build_max_end(file.items.iter().map(|entry| entry.end), file.items.len());
        }
        for (path, signals) in pending_signals {
            let file = files.entry(path).or_default();
            file.width = KEY_WIDTH;
            let width = file.width;
            file.signals = signals
                .into_iter()
                .map(|pending| IndexedSignal {
                    start: key(&pending.span, width, true),
                    end: key(&pending.span, width, false),
                    order: pending.order,
                    path: pending.path,
                })
                .collect();
            file.signals.sort_by_key(|entry| entry.start);
            file.signal_max_end = build_max_end(
                file.signals.iter().map(|entry| entry.end),
                file.signals.len(),
            );
        }
        SpanIndex { files }
    }

    /// What is at file:line:col?  None when no indexed span contains it.
    pub(crate) fn item_at(&self, file: &Path, line: u32, col: u32) -> Option<SourceItem> {
        let file_index = self.files.get(file)?;
        let query = point_key(line, col, file_index.width);
        let mut best: Option<&IndexedItem> = None;
        file_index.collect_items(1, 0, file_index.tree_len(), query, &mut best);
        best.map(|entry| entry.item.clone())
    }

    /// Every signal path whose declaration or driver/load reference covers
    /// file:line:col, in flat-table order.
    pub(crate) fn signals_at(&self, file: &Path, line: u32, col: u32) -> Vec<String> {
        let Some(file_index) = self.files.get(file) else {
            return Vec::new();
        };
        let query = point_key(line, col, file_index.width);
        let mut found: Vec<(u32, &str)> = Vec::new();
        file_index.collect_signals(1, 0, file_index.signal_tree_len(), query, &mut found);
        found.sort_unstable_by_key(|(order, _)| *order);
        let mut paths: Vec<String> = Vec::with_capacity(found.len());
        for (_, path) in found {
            if paths.last().map(|last| last.as_str()) != Some(path) {
                paths.push(path.to_owned());
            }
        }
        paths
    }
}

/// Interval key: both ends use the column, so a point span contains its own
/// position under the usual start <= point <= end test.
fn key(span: &Span, width: u64, start: bool) -> u64 {
    if start {
        u64::from(span.start_line) * width + u64::from(span.start_col)
    } else {
        u64::from(span.end_line) * width + u64::from(span.end_col)
    }
}

fn point_key(line: u32, column: u32, width: u64) -> u64 {
    u64::from(line) * width + u64::from(column)
}

/// Numeric key multiplier: one past the largest possible column, so
/// `line * KEY_WIDTH + column` never wraps into the next line for any `u32`
/// column.  This keeps the interval test exact even for positions far to the
/// right of every span (comments, long lines).
const KEY_WIDTH: u64 = 1 << 32;

/// Segment tree of the maximum interval end (padded to a power of two).
fn build_max_end(ends: impl Iterator<Item = u64>, len: usize) -> Vec<u64> {
    if len == 0 {
        return Vec::new();
    }
    let tree_len = len.next_power_of_two();
    let mut tree = vec![0u64; tree_len * 2];
    for (index, end) in ends.enumerate() {
        tree[tree_len + index] = end;
    }
    for node in (1..tree_len).rev() {
        tree[node] = tree[node * 2].max(tree[node * 2 + 1]);
    }
    tree
}

impl FileIndex {
    fn tree_len(&self) -> usize {
        self.items.len().next_power_of_two().max(1)
    }

    fn signal_tree_len(&self) -> usize {
        self.signals.len().next_power_of_two().max(1)
    }

    fn collect_items<'a>(
        &'a self,
        node: usize,
        lo: usize,
        hi: usize,
        query: u64,
        best: &mut Option<&'a IndexedItem>,
    ) {
        if lo >= self.items.len() || self.items[lo].start > query {
            return;
        }
        if self.item_max_end.get(node).copied().unwrap_or(0) < query {
            return;
        }
        if hi - lo == 1 {
            let entry = &self.items[lo];
            if entry.end >= query {
                let is_better = match best {
                    Some(current) => (entry.size, entry.order) < (current.size, current.order),
                    None => true,
                };
                if is_better {
                    *best = Some(entry);
                }
            }
            return;
        }
        let mid = lo + (hi - lo) / 2;
        self.collect_items(node * 2, lo, mid, query, best);
        self.collect_items(node * 2 + 1, mid, hi, query, best);
    }

    fn collect_signals<'a>(
        &'a self,
        node: usize,
        lo: usize,
        hi: usize,
        query: u64,
        found: &mut Vec<(u32, &'a str)>,
    ) {
        if lo >= self.signals.len() || self.signals[lo].start > query {
            return;
        }
        if self.signal_max_end.get(node).copied().unwrap_or(0) < query {
            return;
        }
        if hi - lo == 1 {
            let entry = &self.signals[lo];
            if entry.end >= query {
                found.push((entry.order, entry.path.as_str()));
            }
            return;
        }
        let mid = lo + (hi - lo) / 2;
        self.collect_signals(node * 2, lo, mid, query, found);
        self.collect_signals(node * 2 + 1, mid, hi, query, found);
    }
}
