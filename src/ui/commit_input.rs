use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InspectorElementId, IntoElement,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, Pixels, Style, UTF16Selection, Window,
    actions, div, prelude::*, px, relative,
};

use super::theme::{BORDER_COLOR, INPUT_BACKGROUND_COLOR, MUTED_TEXT_COLOR, PRIMARY_TEXT_COLOR};

actions!(
    commit_input,
    [Backspace, Delete, SelectAll, Copy, Cut, Paste]
);

/// Evento usado para que la ventana actualice contador y disponibilidad.
pub struct CommitMessageChanged;

/// Editor de commit pequeño, independiente del editor GPL de Zed.
pub struct CommitInput {
    focus_handle: FocusHandle,
    content: String,
    selected_range: Range<usize>,
    marked_range: Option<Range<usize>>,
}

impl CommitInput {
    /// Crea un editor vacío y registra sus atajos locales.
    #[must_use]
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            selected_range: 0..0,
            marked_range: None,
        }
    }

    /// Instala keybindings comunes del campo de texto.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("CommitInput")),
            KeyBinding::new("delete", Delete, Some("CommitInput")),
            KeyBinding::new("ctrl-a", SelectAll, Some("CommitInput")),
            KeyBinding::new("ctrl-c", Copy, Some("CommitInput")),
            KeyBinding::new("ctrl-x", Cut, Some("CommitInput")),
            KeyBinding::new("ctrl-v", Paste, Some("CommitInput")),
        ]);
    }

    /// Devuelve el texto actual sin normalizar sus saltos.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Sustituye la propuesta sin ejecutar ninguna acción adicional.
    pub fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        let end = content.len();
        self.content = content;
        self.selected_range = end..end;
        self.marked_range = None;
        Self::changed(cx);
    }

    /// Limpia el editor después de un commit confirmado.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_content(String::new(), cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() && self.selected_range.start > 0 {
            let previous = previous_char_boundary(&self.content, self.selected_range.start);
            self.selected_range = previous..self.selected_range.start;
        }
        self.replace_selection("");
        Self::changed(cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() && self.selected_range.end < self.content.len() {
            let next = next_char_boundary(&self.content, self.selected_range.end);
            self.selected_range = self.selected_range.end..next;
        }
        self.replace_selection("");
        Self::changed(cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        self.replace_selection("");
        Self::changed(cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text);
            Self::changed(cx);
        }
    }

    fn replace_selection(&mut self, replacement: &str) {
        self.content
            .replace_range(self.selected_range.clone(), replacement);
        let end = self.selected_range.start + replacement.len();
        self.selected_range = end..end;
        self.marked_range = None;
    }

    fn changed(cx: &mut Context<Self>) {
        cx.emit(CommitMessageChanged);
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .scan(0, |utf16_count, (utf8_index, character)| {
                let current = *utf16_count;
                *utf16_count += character.len_utf16();
                Some((current, utf8_index))
            })
            .find_map(|(utf16_count, utf8_index)| (utf16_count >= offset).then_some(utf8_index))
            .unwrap_or(self.content.len())
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset].encode_utf16().count()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }
}

impl gpui::EventEmitter<CommitMessageChanged> for CommitInput {}

impl Focusable for CommitInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for CommitInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.replace_selection(new_text);
        Self::changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let replacement_start = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone())
            .start;
        self.selected_range = replacement_start..replacement_start;
        self.replace_selection(new_text);
        if !new_text.is_empty() {
            self.marked_range = Some(replacement_start..replacement_start + new_text.len());
        }
        if let Some(selected) = new_selected_range_utf16 {
            let selected = self.range_from_utf16(&selected);
            self.selected_range =
                replacement_start + selected.start..replacement_start + selected.end;
        }
        Self::changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.content.len()))
    }
}

impl Render for CommitInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle.clone();
        let is_focused = focus_handle.is_focused(window);
        let displayed_text = if self.content.is_empty() {
            "Mensaje de commit".to_owned()
        } else {
            self.content.clone()
        };
        let text_color = if self.content.is_empty() {
            MUTED_TEXT_COLOR
        } else {
            PRIMARY_TEXT_COLOR
        };

        div()
            .id("commit-input")
            .key_context("CommitInput")
            .track_focus(&focus_handle)
            .relative()
            .h(px(92.0))
            .w_full()
            .p_2()
            .rounded_sm()
            .border_1()
            .border_color(if is_focused {
                super::theme::ACCENT_COLOR
            } else {
                BORDER_COLOR
            })
            .bg(INPUT_BACKGROUND_COLOR)
            .text_sm()
            .text_color(text_color)
            .whitespace_normal()
            .cursor(gpui::CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, cx| {
                window.focus(&focus_handle, cx);
            })
            .child(displayed_text)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(InputCapture { input: cx.entity() }),
            )
    }
}

struct InputCapture {
    input: Entity<CommitInput>,
}

impl IntoElement for InputCapture {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[allow(
    clippy::ignored_unit_patterns,
    reason = "Las firmas vienen impuestas por el trait Element de GPUI"
)]
impl Element for InputCapture {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
    }
}

fn previous_char_boundary(value: &str, offset: usize) -> usize {
    value[..offset]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_char_boundary(value: &str, offset: usize) -> usize {
    value[offset..]
        .char_indices()
        .nth(1)
        .map_or(value.len(), |(index, _)| offset + index)
}
