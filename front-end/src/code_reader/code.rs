use wasm_bindgen::JsCast;
use yew::prelude::*;

use crate::code_reader::code_block::CodeBlock;
use crate::code_reader::monaco_glue;
use crate::lsp::LspBridge;
use crate::nav::selection_add::EditorSelection;
use crate::nav::{Goto, SourceClick};
use crate::state::SourcesSlice;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CodeReaderProps {
    /// The open documents and the active tab. Both are store state now (R2
    /// step 3): the component renders the slice and dispatches the intents.
    pub sources: SourcesSlice,
    /// A request to bring one position into view; the token makes repeats work.
    #[prop_or_default]
    pub goto: Option<Goto>,
    /// A plain left click on an identifier inside the editor.
    #[prop_or_default]
    pub onwordclick: Callback<SourceClick>,
    /// The page's language server client; the editor opens its documents here.
    pub lsp: LspBridge,
    /// A tab was clicked.
    #[prop_or_default]
    pub ontab: Callback<usize>,
    /// A tab was closed.
    #[prop_or_default]
    pub onclose: Callback<usize>,
    /// The editor applied jump `token` (the store then clears the request).
    #[prop_or_default]
    pub onshown: Callback<u32>,
    /// The visible buffer changed; the store is the document of record.
    #[prop_or_default]
    pub onchange: Callback<(String, String)>,
    /// A block of code was selected and the user asked for every signal it
    /// mentions (Ctrl+W with a selection, Ctrl+Shift+W, or the editor's
    /// "Add signals in selection to the wave" context-menu entry).
    #[prop_or_default]
    pub onaddselection: Callback<EditorSelection>,
}

pub enum Msg {
    SetActive(usize),
    Close(usize),
    /// A page-wide key: Ctrl+W / Ctrl+F4 closes the active tab.
    KeyDown(web_sys::KeyboardEvent),
}

pub struct CodeReader {
    /// The window keydown listener (created once, dropped in `destroy`).
    keydown: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
}

impl Component for CodeReader {
    type Message = Msg;
    type Properties = CodeReaderProps;

    fn create(ctx: &Context<Self>) -> Self {
        let mut this = Self { keydown: None };
        this.attach_key_listener(ctx);
        this
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let (Some(window), Some(listener)) = (web_sys::window(), self.keydown.take()) {
            let _ = window
                .remove_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref());
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::SetActive(idx) => {
                // the store owns the active tab; the subscription re-renders us
                ctx.props().ontab.emit(idx);
                false
            }
            Msg::KeyDown(event) => {
                // The page-wide shortcuts: Ctrl+W / Ctrl+F4 close the active
                // source tab, and Ctrl+W (or Ctrl+Shift+W) with a non-empty
                // editor selection adds the selected block's signals instead
                // (the Verdi flow). `key_action` holds the whole key table.
                let action = key_action(
                    &event.key(),
                    event.ctrl_key(),
                    event.shift_key(),
                    key_target(&event),
                );
                match action {
                    KeyAction::AddSelection => {
                        // No selection (a bare cursor, or monaco still loading):
                        // nothing to add. The editor keeps owning its keys, so
                        // Ctrl+W never closes the tab the user is editing.
                        let Some(selection) = monaco_glue::active_selection() else {
                            return false;
                        };
                        event.prevent_default();
                        ctx.props().onaddselection.emit(selection);
                    }
                    KeyAction::CloseTab => {
                        if !ctx.props().sources.is_empty() {
                            event.prevent_default();
                            ctx.props().onclose.emit(ctx.props().sources.active_index());
                        }
                    }
                    KeyAction::Ignore => {}
                }
                false
            }
            Msg::Close(idx) => {
                ctx.props().onclose.emit(idx);
                false
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let props = ctx.props();

        // the store owns the active tab; a closed tab can leave the index one
        // render behind, so clamp it
        let active = props.sources.active_index();
        let (file_name, text) = props
            .sources
            .active_source()
            .map(|source| (source.name.clone(), source.text.clone()))
            .unwrap_or_else(|| (String::new(), String::new()));

        html! {
            <>
                <nav class="tabs" role="tablist" aria-label="Open sources">
                {
                    for props.sources.items.iter().enumerate().map(|(idx,source)| {
                        let name = &source.name;
                        let class = if idx == active {"tab active"} else {"tab"};
                        html! {
                            // A div, not a button: the closer is a real button,
                            // and a button inside a button is invalid html.
                            <div class={class}
                                onclick={link.callback(move |_| Msg::SetActive(idx))}>
                                <button type="button" class="tab-name"
                                    role="tab" aria-selected={if idx == active {"true"} else {"false"}}
                                    title={name.clone()}
                                    onclick={link.callback(move |_| Msg::SetActive(idx))}>
                                    {name.clone()}
                                </button>
                                <button type="button" class="tab-close"
                                    aria-label={format!("Close {name}")}
                                    title="Close the tab"
                                    onclick={link.callback(move |event: MouseEvent| {
                                        // closing must not also activate the tab
                                        event.stop_propagation();
                                        Msg::Close(idx)
                                    })}>{"×"}</button>
                            </div>
                        }
                    })
                }
                </nav>
                <div class="code-host">
                    <CodeBlock file={file_name} text={text}
                        goto={props.goto.clone()}
                        onwordclick={props.onwordclick.clone()}
                        onshown={props.onshown.clone()}
                        onchange={props.onchange.clone()}
                        onaddselection={props.onaddselection.clone()}
                        lsp={props.lsp.clone()} />
                </div>
            </>
        }
    }
}

impl CodeReader {
    /// Install the page-wide key listener that owns Ctrl+W / Ctrl+F4.
    fn attach_key_listener(&mut self, ctx: &Context<Self>) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let link = ctx.link().clone();
        let listener = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                link.send_message(Msg::KeyDown(event));
            },
        );
        if window
            .add_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref())
            .is_ok()
        {
            self.keydown = Some(listener);
        }
    }
}

/// Where a key press started, as far as the page-wide shortcuts care.
///
/// The DOM ancestor chain is collapsed into this before the key table looks at
/// it, so the table itself is pure (and host tested).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyTarget {
    /// The dump tab strip owns Ctrl+W there (it closes the active dump).
    DumpTabs,
    /// A plain text field (`<input>`, `<textarea>`, contenteditable, ...).
    TextField,
    /// The monaco source editor (its hidden textarea included).
    Editor,
    /// Anywhere else in the page.
    Other,
}

/// What one key press means for the source pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Close the active source tab (the pre-Wave-19 Ctrl+W / Ctrl+F4).
    CloseTab,
    /// Add the signals of the editor's selected block (Ctrl+W with a selection,
    /// or the Ctrl+Shift+W alias).
    AddSelection,
    /// Not ours: the editor, a text field or the dump strip owns the key.
    Ignore,
}

/// Decode a key press for the page-wide source-pane shortcuts.
///
/// The rules:
///
/// * only Ctrl combinations are ours (Ctrl+F4 keeps its old close meaning);
/// * a plain text field and the dump tab strip own their keys;
/// * in the **editor** Ctrl+W means "add the selected block", and Ctrl+Shift+W
///   is the browser-safe alias of the same add; with no selection nothing
///   happens (the editor owns its keys, so the tab the user is editing is never
///   closed from under them);
/// * anywhere else Ctrl+W still closes the active tab, exactly as before.
///
/// Pure, so the whole table is host tested.
pub fn key_action(key: &str, ctrl: bool, shift: bool, target: KeyTarget) -> KeyAction {
    if !ctrl {
        return KeyAction::Ignore;
    }
    let is_w = key.eq_ignore_ascii_case("w");
    match target {
        KeyTarget::DumpTabs | KeyTarget::TextField => KeyAction::Ignore,
        KeyTarget::Editor => {
            if is_w {
                KeyAction::AddSelection
            } else {
                KeyAction::Ignore
            }
        }
        KeyTarget::Other => {
            if key == "F4" || (is_w && !shift) {
                KeyAction::CloseTab
            } else {
                KeyAction::Ignore
            }
        }
    }
}

/// Where a keyboard event started: the DOM target collapsed to a [`KeyTarget`].
///
/// The ancestry is walked once; the first strip/editor/text-field signal wins,
/// with the dump strip checked first (it has a Ctrl+W handler of its own).
fn key_target(event: &web_sys::KeyboardEvent) -> KeyTarget {
    let Some(target) = event.target() else {
        return KeyTarget::Other;
    };
    let Ok(element) = target.dyn_into::<web_sys::Element>() else {
        return KeyTarget::Other;
    };
    let mut editor = false;
    let mut field = false;
    let mut current = Some(element);
    while let Some(element) = current {
        if element
            .get_attribute("class")
            .map(|class| class.contains("dump-tabs"))
            .unwrap_or(false)
        {
            return KeyTarget::DumpTabs;
        }
        if element
            .get_attribute("class")
            .map(|class| class.contains("monaco-editor"))
            .unwrap_or(false)
        {
            editor = true;
        }
        let tag = element.tag_name().to_ascii_lowercase();
        if tag == "input"
            || tag == "textarea"
            || tag == "select"
            || element.has_attribute("contenteditable")
        {
            field = true;
        }
        current = element.parent_element();
    }
    if editor {
        KeyTarget::Editor
    } else if field {
        KeyTarget::TextField
    } else {
        KeyTarget::Other
    }
}

#[cfg(test)]
mod tests {
    use super::{key_action, KeyAction, KeyTarget};

    #[test]
    fn ctrl_w_closes_the_tab_outside_the_editor() {
        assert_eq!(
            key_action("w", true, false, KeyTarget::Other),
            KeyAction::CloseTab
        );
        // the browser reports the capital letter when Shift is held
        assert_eq!(
            key_action("W", true, false, KeyTarget::Other),
            KeyAction::CloseTab
        );
    }

    #[test]
    fn ctrl_w_in_the_editor_is_the_block_add() {
        assert_eq!(
            key_action("w", true, false, KeyTarget::Editor),
            KeyAction::AddSelection
        );
        // ... and the editor/text field/strip keeps owning its keys otherwise
        assert_eq!(
            key_action("w", true, false, KeyTarget::TextField),
            KeyAction::Ignore
        );
        assert_eq!(
            key_action("w", true, false, KeyTarget::DumpTabs),
            KeyAction::Ignore
        );
    }

    #[test]
    fn ctrl_shift_w_is_the_add_alias_and_never_closes() {
        assert_eq!(
            key_action("W", true, true, KeyTarget::Editor),
            KeyAction::AddSelection
        );
        assert_eq!(
            key_action("W", true, true, KeyTarget::Other),
            KeyAction::Ignore
        );
        assert_eq!(
            key_action("W", true, true, KeyTarget::TextField),
            KeyAction::Ignore
        );
    }

    #[test]
    fn ctrl_f4_keeps_its_close_meaning_and_other_keys_are_ignored() {
        assert_eq!(
            key_action("F4", true, false, KeyTarget::Other),
            KeyAction::CloseTab
        );
        assert_eq!(
            key_action("F4", true, false, KeyTarget::Editor),
            KeyAction::Ignore
        );
        assert_eq!(
            key_action("z", true, false, KeyTarget::Other),
            KeyAction::Ignore
        );
        assert_eq!(
            key_action("w", false, false, KeyTarget::Other),
            KeyAction::Ignore
        );
    }
}
