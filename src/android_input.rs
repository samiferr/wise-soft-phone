use android_activity::{
    AndroidApp,
    input::{ImeOptions, InputType, TextInputAction, TextInputState, TextSpan},
};
use eframe::egui::{
    self, Response,
    text::{CCursor, CCursorRange},
};
use std::{cell::RefCell, time::Duration};

// winit 0.30 delivers hardware keys but does not forward GameActivity TextEvent edits.
thread_local! {
    static EDITOR: RefCell<Option<(AndroidApp, Option<egui::Id>)>> = const { RefCell::new(None) };
}

pub fn init(app: AndroidApp) {
    EDITOR.with_borrow_mut(|editor| *editor = Some((app, None)));
}

/// Space covered by system bars, display cutouts and the keyboard, as
/// (left, top, right, bottom) in physical pixels.
pub fn insets() -> [f32; 4] {
    EDITOR.with_borrow(|editor| {
        let Some((app, _)) = editor else {
            return [0.; 4];
        };
        let (Some(window), rect) = (app.native_window(), app.content_rect()) else {
            return [0.; 4];
        };
        // The content rectangle is empty until Android reports it.
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return [0.; 4];
        }
        [
            rect.left.max(0) as f32,
            rect.top.max(0) as f32,
            (window.width() - rect.right).max(0) as f32,
            (window.height() - rect.bottom).max(0) as f32,
        ]
    })
}

pub fn sync(response: &Response, value: &mut String, password: bool) {
    EDITOR.with_borrow_mut(|editor| {
        let Some((app, active)) = editor else {
            return;
        };
        if !response.has_focus() {
            if *active == Some(response.id) {
                *active = None;
                app.set_text_input_state(TextInputState::default());
            }
            return;
        }
        response
            .ctx
            .request_repaint_after(Duration::from_millis(50));
        let new_field = *active != Some(response.id);
        if new_field {
            let variation = if password {
                InputType::TYPE_TEXT_VARIATION_PASSWORD
            } else {
                InputType::TYPE_TEXT_FLAG_NO_SUGGESTIONS
            };
            app.set_ime_editor_info(
                InputType::TYPE_CLASS_TEXT | variation,
                TextInputAction::Done,
                ImeOptions::IME_FLAG_NO_PERSONALIZED_LEARNING | ImeOptions::IMG_FLAG_NO_EXTRACT_UI,
            );
            *active = Some(response.id);
            response.scroll_to_me(Some(egui::Align::Center));
        }
        let mut edit = egui::TextEdit::load_state(&response.ctx, response.id).unwrap_or_default();
        if new_field || response.changed() || response.clicked() {
            // Android EditorInfo uses UTF-16 offsets. egui uses Unicode scalar indices.
            let offset = |cursor: CCursor| {
                value
                    .chars()
                    .take(cursor.index.0)
                    .map(char::len_utf16)
                    .sum()
            };
            let selection = edit.cursor.char_range().map_or(
                TextSpan {
                    start: value.encode_utf16().count(),
                    end: value.encode_utf16().count(),
                },
                |range| TextSpan {
                    start: offset(range.secondary),
                    end: offset(range.primary),
                },
            );
            app.set_text_input_state(TextInputState {
                text: value.clone(),
                selection,
                compose_region: None,
            });
        } else {
            let state = app.text_input_state();
            if *value != state.text {
                *value = state.text;
                let cursor = |units: usize| {
                    let mut used = 0;
                    CCursor::new(
                        value
                            .chars()
                            .take_while(|ch| {
                                used += ch.len_utf16();
                                used <= units
                            })
                            .count(),
                    )
                };
                edit.cursor.set_char_range(Some(CCursorRange::two(
                    cursor(state.selection.start),
                    cursor(state.selection.end),
                )));
                egui::TextEdit::store_state(&response.ctx, response.id, edit);
                response.ctx.request_repaint();
            }
        }
    });
}
