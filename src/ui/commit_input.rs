use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InspectorElementId, Rgba,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, Size, Style, TextRun, UTF16Selection, Window, WrappedLine, actions,
    div, fill, point, prelude::*, px, relative, rgba, size,
};

/// Mínimo de líneas visibles cuando el campo está vacío o con poco texto.
const MIN_VISIBLE_LINES: f32 = 2.0;
/// Máximo de líneas visibles antes de activar scroll interno.
const MAX_VISIBLE_LINES: f32 = 6.0;
/// Padding vertical total del contenedor (`p_2` arriba + `p_2` abajo).
const CONTAINER_VERTICAL_PADDING: f32 = 16.0;

use super::theme::{BORDER_COLOR, INPUT_BACKGROUND_COLOR, MUTED_TEXT_COLOR, PRIMARY_TEXT_COLOR};

actions!(
    commit_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        DocumentHome,
        DocumentEnd,
        SelectHome,
        SelectEnd,
        SelectDocumentHome,
        SelectDocumentEnd,
        DeleteWordBackward,
        DeleteWordForward,
        SelectAll,
        Copy,
        Cut,
        Paste
    ]
);

/// Evento usado para que la ventana actualice contador y disponibilidad.
pub struct CommitMessageChanged;

/// Editor de commit pequeño, independiente del editor GPL de Zed.
pub struct CommitInput {
    focus_handle: FocusHandle,
    content: String,
    content_version: u64,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    is_generating: bool,
    is_selecting: bool,
    last_layout: Option<Vec<WrappedLine>>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl CommitInput {
    /// Crea un editor vacío y registra sus atajos locales.
    #[must_use]
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            content_version: 0,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            is_generating: false,
            is_selecting: false,
            last_layout: None,
            last_bounds: None,
        }
    }

    /// Instala keybindings comunes del campo de texto.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("CommitInput")),
            KeyBinding::new("delete", Delete, Some("CommitInput")),
            KeyBinding::new("left", Left, Some("CommitInput")),
            KeyBinding::new("right", Right, Some("CommitInput")),
            KeyBinding::new("shift-left", SelectLeft, Some("CommitInput")),
            KeyBinding::new("shift-right", SelectRight, Some("CommitInput")),
            KeyBinding::new("ctrl-left", WordLeft, Some("CommitInput")),
            KeyBinding::new("ctrl-right", WordRight, Some("CommitInput")),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, Some("CommitInput")),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, Some("CommitInput")),
            KeyBinding::new("home", Home, Some("CommitInput")),
            KeyBinding::new("end", End, Some("CommitInput")),
            KeyBinding::new("ctrl-home", DocumentHome, Some("CommitInput")),
            KeyBinding::new("ctrl-end", DocumentEnd, Some("CommitInput")),
            KeyBinding::new("shift-home", SelectHome, Some("CommitInput")),
            KeyBinding::new("shift-end", SelectEnd, Some("CommitInput")),
            KeyBinding::new("ctrl-shift-home", SelectDocumentHome, Some("CommitInput")),
            KeyBinding::new("ctrl-shift-end", SelectDocumentEnd, Some("CommitInput")),
            KeyBinding::new("ctrl-backspace", DeleteWordBackward, Some("CommitInput")),
            KeyBinding::new("ctrl-delete", DeleteWordForward, Some("CommitInput")),
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

    /// Versión monotónica del texto, incluida la edición manual.
    #[must_use]
    pub const fn content_version(&self) -> u64 {
        self.content_version
    }

    /// Sustituye la propuesta sin ejecutar ninguna acción adicional.
    pub fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        let end = content.len();
        self.content = content;
        self.content_version = self.content_version.saturating_add(1);
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        Self::changed(cx);
    }

    /// Muestra u oculta el estado de generación sin modificar el texto escrito.
    pub fn set_generating(&mut self, is_generating: bool, cx: &mut Context<Self>) {
        self.is_generating = is_generating;
        cx.notify();
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

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            previous_char_boundary(&self.content, self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            next_char_boundary(&self.content, self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            previous_word_boundary(&self.content, self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            next_word_boundary(&self.content, self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let offset = previous_char_boundary(&self.content, self.cursor_offset());
        self.select_to(offset, cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let offset = next_char_boundary(&self.content, self.cursor_offset());
        self.select_to(offset, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let offset = previous_word_boundary(&self.content, self.cursor_offset());
        self.select_to(offset, cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let offset = next_word_boundary(&self.content, self.cursor_offset());
        self.select_to(offset, cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(self.cursor_offset()), cx);
    }

    fn document_home(&mut self, _: &DocumentHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_start(self.cursor_offset()), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(self.cursor_offset()), cx);
    }

    fn select_document_home(
        &mut self,
        _: &SelectDocumentHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, cx);
    }

    fn select_document_end(
        &mut self,
        _: &SelectDocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.content.len(), cx);
    }

    fn delete_word_backward(
        &mut self,
        _: &DeleteWordBackward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = previous_word_boundary(&self.content, cursor)..cursor;
        }
        if !self.selected_range.is_empty() {
            self.replace_selection("");
            Self::changed(cx);
        }
    }

    fn delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = cursor..next_word_boundary(&self.content, cursor);
        }
        if !self.selected_range.is_empty() {
            self.replace_selection("");
            Self::changed(cx);
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
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
        self.content_version = self.content_version.saturating_add(1);
        let end = self.selected_range.start + replacement.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let anchor = if self.selected_range.is_empty() {
            self.cursor_offset()
        } else if self.selection_reversed {
            self.selected_range.end
        } else {
            self.selected_range.start
        };
        self.selected_range = anchor.min(offset)..anchor.max(offset);
        self.selection_reversed = offset < anchor;
        cx.notify();
    }

    fn line_start(&self, offset: usize) -> usize {
        self.content[..offset]
            .rfind('\n')
            .map_or(0, |newline| newline + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.content[offset..]
            .find('\n')
            .map_or(self.content.len(), |newline| offset + newline)
    }

    fn mouse_offset(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        self.character_index_for_point(position, window, cx)
            .map_or(self.content.len(), |offset| self.offset_from_utf16(offset))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let offset = self.mouse_offset(event.position, window, cx);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
        self.is_selecting = true;
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            let offset = self.mouse_offset(event.position, window, cx);
            self.select_to(offset, cx);
        }
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
            reversed: self.selection_reversed,
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
        position_point: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        if self.content.is_empty() {
            return Some(0);
        }
        let bounds = self.last_bounds.as_ref()?;
        let lines = self.last_layout.as_ref()?;
        let position = bounds.localize(&position_point)?;
        let line_height = window.line_height();
        let mut line_start = 0;
        let mut line_top = Pixels::ZERO;
        for line in lines {
            let line_height_span = line.size(line_height).height;
            if position.y <= line_top + line_height_span {
                let line_position = point(position.x, position.y - line_top);
                let utf8_index = line
                    .closest_index_for_position(line_position, line_height)
                    .unwrap_or_else(|index| index);
                return Some(self.offset_to_utf16(line_start + utf8_index));
            }
            line_top += line_height_span;
            line_start += line.len() + 1;
        }
        Some(self.offset_to_utf16(self.content.len()))
    }
}

impl Render for CommitInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle.clone();
        let is_focused = focus_handle.is_focused(window);
        let line_height = window.line_height();
        let min_height = line_height * MIN_VISIBLE_LINES + px(CONTAINER_VERTICAL_PADDING);
        let max_height = line_height * MAX_VISIBLE_LINES + px(CONTAINER_VERTICAL_PADDING);
        div()
            .id("commit-input")
            .key_context("CommitInput")
            .track_focus(&focus_handle)
            .relative()
            .w_full()
            .min_h(min_height)
            .max_h(max_height)
            .overflow_hidden()
            .p_2()
            .rounded_sm()
            .border_1()
            .border_color(if is_focused {
                super::theme::ACCENT_COLOR
            } else if self.is_generating {
                super::theme::WARNING_COLOR
            } else {
                BORDER_COLOR
            })
            .bg(INPUT_BACKGROUND_COLOR)
            .text_sm()
            .whitespace_normal()
            .cursor(gpui::CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::document_home))
            .on_action(cx.listener(Self::document_end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_document_home))
            .on_action(cx.listener(Self::select_document_end))
            .on_action(cx.listener(Self::delete_word_backward))
            .on_action(cx.listener(Self::delete_word_forward))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(
                div()
                    .id("commit-input-scroll")
                    .w_full()
                    .overflow_y_scroll()
                    .child(TextElement { input: cx.entity() }),
            )
            .when(self.is_generating, |element| {
                element.child(
                    div()
                        .absolute()
                        .top_2()
                        .right_2()
                        .rounded_sm()
                        .bg(super::theme::ELEVATED_BACKGROUND_COLOR)
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(super::theme::WARNING_COLOR)
                        .child("⏳ Generando…"),
                )
            })
    }
}

struct TextElement {
    input: Entity<CommitInput>,
}

struct TextPrepaintState {
    lines: Vec<WrappedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[allow(
    clippy::ignored_unit_patterns,
    reason = "Las firmas vienen impuestas por el trait Element de GPUI"
)]
impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = TextPrepaintState;

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
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.clone();
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        let layout_id = window.request_measured_layout(style, move |known_dimensions, available_space, window, cx| {
            let input_state = input.read(cx);
            let (display_text, text_color, _content_is_empty) =
                display_text_for_input(input_state);
            let wrap_width = known_dimensions.width.or(match available_space.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            }).unwrap_or(px(200.0));
            let lines = shape_display_lines(
                &display_text,
                text_color,
                wrap_width,
                window,
                cx,
            );
            let line_height = window.line_height();
            let content_height = content_height_for_lines(&lines, line_height);
            Size {
                width: known_dimensions.width.unwrap_or(wrap_width),
                height: content_height,
            }
        });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let selected_range = input.selected_range.clone();
        let cursor_offset = input.cursor_offset();
        let (display_text, text_color, content_is_empty) = display_text_for_input(input);
        let lines = shape_display_lines(
            &display_text,
            text_color,
            bounds.size.width,
            window,
            cx,
        );
        let line_height = window.line_height();
        let cursor_position = position_for_index(&lines, cursor_offset, line_height);
        let cursor = cursor_position.map(|position| {
            fill(
                Bounds::new(
                    point(bounds.left() + position.x, bounds.top() + position.y),
                    size(px(2.0), line_height),
                ),
                gpui::blue(),
            )
        });
        let selections = selection_quads(
            &lines,
            &selected_range,
            line_height,
            bounds,
            content_is_empty,
        );
        TextPrepaintState {
            lines,
            cursor,
            selections,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        let mut line_top = px(0.0);
        for line in &prepaint.lines {
            let line_height = window.line_height();
            let mut origin = bounds.origin;
            origin.y += line_top;
            line.paint(
                origin,
                line_height,
                gpui::TextAlign::Left,
                Some(bounds),
                window,
                cx,
            )
            .ok();
            line_top += line.size(line_height).height;
        }
        let lines = std::mem::take(&mut prepaint.lines);
        self.input.update(cx, |input, _| {
            input.last_layout = Some(lines);
            input.last_bounds = Some(bounds);
        });
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
    }
}

fn display_text_for_input(input: &CommitInput) -> (String, Rgba, bool) {
    let content_is_empty = input.content.is_empty();
    let display_text = if content_is_empty {
        if input.is_generating {
            "Generando mensaje con Cursor…".to_owned()
        } else {
            "Escribe el mensaje de commit…".to_owned()
        }
    } else {
        input.content.clone()
    };
    let text_color = if content_is_empty {
        MUTED_TEXT_COLOR
    } else {
        PRIMARY_TEXT_COLOR
    };
    (display_text, text_color, content_is_empty)
}

fn shape_display_lines(
    display_text: &str,
    text_color: Rgba,
    wrap_width: Pixels,
    window: &mut Window,
    _cx: &mut App,
) -> Vec<WrappedLine> {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let run = TextRun {
        len: display_text.len(),
        font: style.font(),
        color: text_color.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_text(
            display_text.to_owned().into(),
            font_size,
            &[run],
            Some(wrap_width),
            None,
        )
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn content_height_for_lines(lines: &[WrappedLine], line_height: Pixels) -> Pixels {
    lines
        .iter()
        .map(|line| line.size(line_height).height)
        .fold(Pixels::ZERO, |total, height| total + height)
        .max(line_height)
}

fn position_for_index(
    lines: &[WrappedLine],
    index: usize,
    line_height: Pixels,
) -> Option<Point<Pixels>> {
    let mut line_start = 0;
    let mut line_top = px(0.0);
    for line in lines {
        let line_end = line_start + line.len();
        if index <= line_end {
            return line
                .position_for_index(index - line_start, line_height)
                .map(|position| point(position.x, position.y + line_top));
        }
        line_top += line.size(line_height).height;
        line_start = line_end + 1;
    }
    None
}

fn selection_quads(
    lines: &[WrappedLine],
    selected_range: &Range<usize>,
    line_height: Pixels,
    bounds: Bounds<Pixels>,
    content_is_empty: bool,
) -> Vec<PaintQuad> {
    if content_is_empty || selected_range.is_empty() {
        return Vec::new();
    }

    let mut selections = Vec::new();
    let mut line_start = 0;
    let mut line_top = px(0.0);
    for line in lines {
        let line_end = line_start + line.len();
        let start = selected_range.start.max(line_start);
        let end = selected_range.end.min(line_end);
        if start < end
            && let (Some(start_position), Some(end_position)) = (
                line.position_for_index(start - line_start, line_height),
                line.position_for_index(end - line_start, line_height),
            )
        {
            selections.push(fill(
                Bounds::from_corners(
                    point(
                        bounds.left() + start_position.x,
                        bounds.top() + line_top + start_position.y,
                    ),
                    point(
                        bounds.left() + end_position.x,
                        bounds.top() + line_top + end_position.y + line_height,
                    ),
                ),
                rgba(0x335F_9EF8),
            ));
        }
        line_top += line.size(line_height).height;
        line_start = line_end + 1;
    }
    selections
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

fn previous_word_boundary(value: &str, mut offset: usize) -> usize {
    while offset > 0 {
        let previous = previous_char_boundary(value, offset);
        let character = value[previous..offset].chars().next().unwrap_or_default();
        if !character.is_whitespace() {
            break;
        }
        offset = previous;
    }
    while offset > 0 {
        let previous = previous_char_boundary(value, offset);
        let character = value[previous..offset].chars().next().unwrap_or_default();
        if character.is_whitespace() {
            break;
        }
        offset = previous;
    }
    offset
}

fn next_word_boundary(value: &str, mut offset: usize) -> usize {
    while offset < value.len() {
        let next = next_char_boundary(value, offset);
        let character = value[offset..next].chars().next().unwrap_or_default();
        if !character.is_whitespace() {
            break;
        }
        offset = next;
    }
    while offset < value.len() {
        let next = next_char_boundary(value, offset);
        let character = value[offset..next].chars().next().unwrap_or_default();
        if character.is_whitespace() {
            break;
        }
        offset = next;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::{next_word_boundary, previous_word_boundary};

    #[test]
    fn word_boundaries_behave_like_standard_text_editors() {
        let value = "feat: añade historial";
        assert_eq!(previous_word_boundary(value, value.len()), 13);
        assert_eq!(previous_word_boundary(value, 12), 6);
        assert_eq!(next_word_boundary(value, 0), 5);
        assert_eq!(next_word_boundary(value, 6), 12);
    }
}
