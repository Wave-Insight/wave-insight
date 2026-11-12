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
    /// Ctrl+Enter in the editor: add the signal under the cursor to the wave
    /// view. A plain click only selects a signal now, so this is the explicit
    /// keyboard way to add it.
    #[prop_or_default]
    pub onaddsignal: Callback<SourceClick>,
}

pub enum Msg {
    SetActive(usize),
    Close(usize),
    /// A page-wide key: Ctrl+W / Ctrl+F4 closes the active tab.
    KeyDown(web_sys::KeyboardEvent),
    /// The same key in the capture phase, i.e. before the editor's own
    /// handler sees it. Ctrl+Enter is bound by monaco itself ("insert line
    /// below"), so only a capture listener can claim it for the wave add
    /// without the editor also inserting a line.
    KeyDownCapture(web_sys::KeyboardEvent),
}

pub struct CodeReader {
    /// The window keydown listener (created once, dropped in `destroy`).
    keydown: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    /// The capture-phase keydown listener (Ctrl+Enter before monaco).
    keydown_capture: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
}

impl Component for CodeReader {
    type Message = Msg;
    type Properties = CodeReaderProps;

    fn create(ctx: &Context<Self>) -> Self {
        let mut this = Self {
            keydown: None,
            keydown_capture: None,
        };
        this.attach_key_listener(ctx);
        this
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        self.detach_key_listeners();
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::SetActive(idx) => {
                // the store owns the active tab; the subscription re-renders us
                ctx.props().ontab.emit(idx);
                false
            }
            Msg::KeyDown(event) => {
                self.handle_key(ctx, event, false);
                false
            }
            Msg::KeyDownCapture(event) => {
                self.handle_key(ctx, event, true);
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
    /// Install the page-wide key listeners that own Ctrl+W / Ctrl+F4.
    ///
    /// Two listeners on purpose: Ctrl+W is a browser/editor key, but **monaco
    /// binds Ctrl+Enter to "insert line below"** and handles it on its own DOM
    /// node before a bubble-phase window listener runs. The capture-phase
    /// listener sees Ctrl+Enter first and stops it there, so the editor does
    /// not also insert a line; the bubble-phase one keeps the Ctrl+W handling
    /// where it always was.
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
        let link = ctx.link().clone();
        let capture = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                link.send_message(Msg::KeyDownCapture(event));
            },
        );
        if window
            .add_event_listener_with_callback_and_bool(
                "keydown",
                capture.as_ref().unchecked_ref(),
                true,
            )
            .is_ok()
        {
            self.keydown_capture = Some(capture);
        }
    }

    /// Remove both window listeners (the component goes away).
    fn detach_key_listeners(&mut self) {
        let Some(window) = web_sys::window() else {
            return;
        };
        if let Some(listener) = self.keydown.take() {
            let _ = window
                .remove_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref());
        }
        if let Some(listener) = self.keydown_capture.take() {
            let _ = window.remove_event_listener_with_callback_and_bool(
                "keydown",
                listener.as_ref().unchecked_ref(),
                true,
            );
        }
    }

    /// The page-wide shortcut table, shared by the two listeners.
    ///
    /// `capture` says which listener delivered the key: the add-signal gesture
    /// is handled only in the capture phase (it has to beat monaco's own
    /// Ctrl+Enter binding), the Ctrl+W gestures only in the bubble phase, so
    /// nothing ever runs twice.
    fn handle_key(&self, ctx: &Context<Self>, event: web_sys::KeyboardEvent, capture: bool) {
        let action = key_action(
            &event.key(),
            // Cmd+Enter is Ctrl+Enter on a mac, like the page-wide bulk-add
            // shortcut in the shell
            event.ctrl_key() || event.meta_key(),
            event.shift_key(),
            key_target(&event),
        );
        match action {
            KeyAction::AddSignal if capture => {
                // monaco binds Ctrl+Enter to "insert line below": claim the key
                // in the capture phase, before the editor's own handler runs,
                // so the shortcut never edits the source. No identifier under
                // the cursor (monaco still loading, or between tokens) just
                // does nothing.
                event.prevent_default();
                event.stop_propagation();
                let Some(word) = monaco_glue::active_word() else {
                    return;
                };
                ctx.props().onaddsignal.emit(SourceClick {
                    file: word.file,
                    line: word.line,
                    col: word.col,
                    word: word.word,
                });
            }
            KeyAction::AddSelection if !capture => {
                // No selection (a bare cursor, or monaco still loading):
                // nothing to add. The editor keeps owning its keys, so Ctrl+W
                // never closes the tab the user is editing.
                let Some(selection) = monaco_glue::active_selection() else {
                    return;
                };
                event.prevent_default();
                ctx.props().onaddselection.emit(selection);
            }
            // nothing to close, nothing to do: the key keeps its old meaning
            KeyAction::CloseTab if !capture && !ctx.props().sources.is_empty() => {
                event.prevent_default();
                ctx.props().onclose.emit(ctx.props().sources.active_index());
            }
            // the other listener's half of the table (or not ours at all)
            _ => {}
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
    /// Add the signal under the cursor (Ctrl+Enter in the editor).
    ///
    /// A click in the source only selects a signal now; this is the explicit
    /// add gesture, so a click never grows the wave list by accident.
    AddSignal,
    /// Not ours: the editor, a text field or the dump strip owns the key.
    Ignore,
}

/// Decode a key press for the page-wide source-pane shortcuts.
///
/// The rules:
///
/// * only Ctrl/Cmd combinations are ours (`ctrl` is true for Cmd on a mac, so
///   Cmd+W/Cmd+F4 keep their meanings there); Ctrl+F4 keeps its close meaning;
/// * a plain text field and the dump tab strip own their keys;
/// * in the **editor** Ctrl+W means "add the selected block", Ctrl+Shift+W is
///   the browser-safe alias of the same add, and Ctrl+Enter means "add the
///   signal under the cursor"; with no selection / no identifier nothing
///   happens (the editor owns its keys, so the tab the user is editing is never
///   closed from under them);
/// * anywhere else Ctrl+W still closes the active tab and Ctrl+Enter belongs
///   to the shell (it bulk-adds the module-tree selection), exactly as before.
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
            } else if key == "Enter" {
                KeyAction::AddSignal
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
    fn ctrl_enter_in_the_editor_adds_the_signal_under_the_cursor() {
        assert_eq!(
            key_action("Enter", true, false, KeyTarget::Editor),
            KeyAction::AddSignal
        );
        // the shell keeps Ctrl+Enter outside the editor (bulk add / picker):
        // the source pane must not steal it
        assert_eq!(
            key_action("Enter", true, false, KeyTarget::Other),
            KeyAction::Ignore
        );
        assert_eq!(
            key_action("Enter", true, false, KeyTarget::TextField),
            KeyAction::Ignore
        );
        // plain Enter is a newline, never an add
        assert_eq!(
            key_action("Enter", false, false, KeyTarget::Editor),
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
