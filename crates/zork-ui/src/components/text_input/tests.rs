mod keyboard {
    use super::super::*;
    use gpui::{
        px, size, AppContext, EntityInputHandler, HeadlessAppContext, Keystroke, WindowHandle,
    };
    use std::{cell::Cell, ops::Range, rc::Rc, time::Instant};

    static PLATFORM: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn serial_platform() -> std::sync::MutexGuard<'static, ()> {
        PLATFORM.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct EditorHarness {
        app: HeadlessAppContext,
        window: WindowHandle<ComposerInput>,
    }
    impl EditorHarness {
        fn new(width: f32, value: &str, single: bool, mac: bool) -> Self {
            Self::configured(width, value, single, mac, |input| input)
        }
        fn configured(
            width: f32,
            value: &str,
            single: bool,
            mac: bool,
            configure: impl FnOnce(ComposerInput) -> ComposerInput,
        ) -> Self {
            let mut app =
                HeadlessAppContext::new(gpui_platform::current_platform(true).text_system());
            app.update(|cx| {
                gpui_base::init(cx);
                shortcuts::bind_keys(cx, mac);
            });
            let window = app
                .open_window(size(px(width), px(100.)), |w, cx| {
                    let input = cx.new(|cx| {
                        let v = ComposerInput::new("", cx);
                        configure(if single { v.single_line() } else { v })
                    });
                    input.update(cx, |v, cx| v.set_value(value, cx));
                    let focus = input.read(cx).focus_handle();
                    w.focus(&focus, cx);
                    input
                })
                .unwrap();
            let mut view = Self { app, window };
            view.draw();
            view.app.run_until_parked();
            view
        }
        fn update<R>(
            &mut self,
            f: impl FnOnce(&mut ComposerInput, &mut Window, &mut Context<ComposerInput>) -> R,
        ) -> R {
            self.window.update(&mut self.app, f).unwrap()
        }
        fn draw(&mut self) {
            self.app
                .update_window(self.window.into(), |_, w, cx| {
                    w.refresh();
                    w.draw(cx).clear(cx);
                })
                .unwrap();
        }
        fn key(&mut self, key: &str) {
            self.app
                .update_window(self.window.into(), |_, w, cx| {
                    w.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
                })
                .unwrap();
            self.app.run_until_parked();
            self.draw();
        }
        fn select(&mut self, range: Range<usize>) {
            self.update(|v, _, cx| edit!(v, cx, |state, scx| state.set_selected_range(range, scx)));
            self.draw();
        }
        fn selection(&mut self) -> Range<usize> {
            self.update(|v, _, cx| edit!(v, cx, |state, _scx| state.selected_range()))
        }
        fn value(&mut self) -> String {
            self.update(|v, _, _| v.value().to_owned())
        }
        fn paste(&mut self, value: &str) {
            self.update(|v, w, cx| {
                v.paste_item(ClipboardItem::new_string(value.into()), false, w, cx)
            });
            self.app.run_until_parked();
            self.draw();
        }
        fn type_text(&mut self, value: &str) {
            for ch in value.chars() {
                self.update(|v, w, cx| v.replace_text_in_range(None, &ch.to_string(), w, cx));
                self.app.run_until_parked();
            }
            self.draw();
        }
    }

    #[test]
    fn numeric_slots_normalize_paste_replace_selection_and_keep_undo() {
        let _guard = serial_platform();
        let mut e = EditorHarness::configured(300., "", true, true, |input| input.numeric_code(6));
        e.paste("１２a３４５６７");
        assert_eq!(e.value(), "123456");
        e.select(2..3);
        e.type_text("9");
        assert_eq!(e.value(), "129456");
        e.key("cmd-z");
        assert_eq!(e.value(), "123456");
        e.select(0..6);
        e.paste("8a9");
        assert_eq!(e.value(), "89");
        e.type_text("x");
        assert_eq!(e.value(), "89");
        e.key("backspace");
        assert_eq!(e.value(), "8");
    }

    #[test]
    fn standalone_multiline_inserts_newline_and_readonly_blocks_edits() {
        let _guard = serial_platform();
        let mut e =
            EditorHarness::configured(300., "第一行", false, true, ComposerInput::multiline);
        e.key("cmd-right");
        e.key("enter");
        e.type_text("第二行");
        assert_eq!(e.value(), "第一行\n第二行");
        e.update(|v, _, cx| v.set_editable(false, true, cx));
        e.draw();
        e.key("cmd-a");
        assert_eq!(e.selection(), 0..e.value().len());
        e.type_text("不可写入");
        e.paste("不能替换");
        assert_eq!(e.value(), "第一行\n第二行");
        e.update(|v, _, cx| v.set_editable(false, false, cx));
        e.draw();
        e.type_text("恢复编辑");
        assert_eq!(e.value(), "恢复编辑");
    }

    #[test]
    fn command_shift_arrows_respect_line_and_document_boundaries() {
        let _platform = serial_platform();
        let value = "first\nsecond line\nlast";
        for (key, expected) in [
            ("cmd-shift-left", 6..9),
            ("cmd-shift-right", 9..17),
            ("cmd-shift-up", 0..9),
            ("cmd-shift-down", 9..value.len()),
        ] {
            let mut e = EditorHarness::new(600., value, false, true);
            e.select(9..9);
            e.key(key);
            assert_eq!(e.selection(), expected, "{key}");
        }
    }

    #[test]
    fn pc_bindings_select_words_and_document_and_support_undo() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(600., "one word\nlast", false, false);
        e.key("ctrl-home");
        assert_eq!(e.selection(), 0..0);
        e.key("ctrl-shift-right");
        assert_eq!(e.selection(), 0..4);
        e.paste("new ");
        assert_eq!(e.value(), "new word\nlast");
        e.key("ctrl-z");
        assert_eq!(e.value(), "one word\nlast");
        e.key("ctrl-shift-z");
        assert_eq!(e.value(), "new word\nlast");
        e.key("ctrl-end");
        e.key("ctrl-shift-home");
        assert_eq!(e.selection(), 0..13);
    }

    #[test]
    fn wrapped_line_navigation_keeps_visual_row_affinity() {
        let _platform = serial_platform();
        for width in [130., 280.] {
            let mut e = EditorHarness::new(
                width,
                &"alpha bravo charlie delta echo ".repeat(5),
                false,
                true,
            );
            e.select(2..2);
            e.key("cmd-right");
            let end = e.selection().end;
            assert!(end > 2 && end < e.value().len());
            e.key("cmd-right");
            assert_eq!(e.selection(), end..end);
            e.key("cmd-shift-left");
            assert_eq!(e.selection(), 0..end);
        }
    }

    #[test]
    fn clicking_a_wrapped_row_end_keeps_selection_on_that_row() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(130., &"alpha bravo charlie delta ".repeat(4), false, true);
        e.select(0..0);
        e.key("cmd-right");
        let expected_end = e.selection().end;
        let position = e.update(|v, _, cx| {
            edit!(v, cx, |state, _scx| {
                let (bounds, height) = state.cursor_layout().unwrap();
                gpui::point(bounds.left() - px(0.5), bounds.top() + height / 2.)
            })
        });
        e.select(0..0);
        e.app
            .update_window(e.window.into(), |_, w, cx| {
                w.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
                w.dispatch_event(
                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        position,
                        button: gpui::MouseButton::Left,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                w.dispatch_event(
                    gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                        position,
                        button: gpui::MouseButton::Left,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            })
            .unwrap();
        e.draw();
        let end = e.selection().end;
        assert_eq!(end, expected_end, "mouse {position:?}");
        e.key("cmd-shift-left");
        assert_eq!(e.selection(), 0..end);
    }

    #[test]
    fn vertical_navigation_preserves_column_and_selection_anchor() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(600., "abcdef\nx\nabcdef", false, true);
        e.select(4..4);
        for (key, expected) in [
            ("down", 8..8),
            ("down", 13..13),
            ("up", 8..8),
            ("up", 4..4),
            ("shift-down", 4..8),
            ("shift-down", 4..13),
            ("shift-up", 4..8),
            ("shift-up", 4..4),
        ] {
            e.key(key);
            assert_eq!(e.selection(), expected, "{key}");
        }
    }

    #[test]
    fn page_navigation_to_unshaped_unicode_lines_keeps_valid_grapheme_offsets() {
        use unicode_segmentation::UnicodeSegmentation;
        let _platform = serial_platform();
        let text = format!("abcdef\n{}", "界a👩‍💻é\n".repeat(30));
        let mut e = EditorHarness::new(600., &text, false, false);
        e.select(2..2);
        for key in [
            "pagedown",
            "pagedown",
            "shift-pagedown",
            "shift-pageup",
            "pageup",
        ] {
            e.key(key);
            let selection = e.selection();
            for offset in [selection.start, selection.end] {
                assert!(
                    offset == text.len() || text.grapheme_indices(true).any(|(i, _)| i == offset),
                    "{key}: {offset}"
                );
            }
        }
        e.type_text("X");
        e.key("ctrl-z");
        assert_eq!(e.value(), text);
    }

    #[test]
    fn unicode_deletion_word_commands_and_clipboard_use_the_library_buffer() {
        let _platform = serial_platform();
        let value = "A👨‍👩‍👧‍👦é中";
        let mut e = EditorHarness::new(600., value, false, true);
        e.key("backspace");
        assert_eq!(e.value(), "A👨‍👩‍👧‍👦é");
        e.key("backspace");
        assert_eq!(e.value(), "A👨‍👩‍👧‍👦");
        e.select(1..1);
        e.key("delete");
        assert_eq!(e.value(), "A");
        e.paste(" one word");
        e.key("alt-backspace");
        assert_eq!(e.value(), "A one ");
        e.key("cmd-z");
        assert_eq!(e.value(), "A one word");
        e.key("cmd-a");
        e.key("cmd-c");
        e.key("cmd-x");
        assert_eq!(e.value(), "");
        e.key("cmd-v");
        assert_eq!(e.value(), "A one word");
        e.key("cmd-z");
        assert_eq!(e.value(), "");
    }

    #[test]
    fn ime_preedit_is_local_and_commit_is_one_edit_without_early_submit() {
        let _platform = serial_platform();
        for unmark in [false, true] {
            let mut e = EditorHarness::new(600., "AB", false, true);
            let edits = Rc::new(Cell::new(0));
            let submits = Rc::new(Cell::new(0));
            e.update(|v, w, cx| {
                let edits = edits.clone();
                let submits = submits.clone();
                cx.subscribe(&cx.entity(), move |_, _, _: &ComposerEdited, _| {
                    edits.set(edits.get() + 1)
                })
                .detach();
                cx.subscribe(&cx.entity(), move |_, _, _: &ComposerSubmit, _| {
                    submits.set(submits.get() + 1)
                })
                .detach();
                v.set_selected_text_range(1..1, w, cx);
                v.replace_and_mark_text_in_range(None, "n", Some(1..1), w, cx);
                v.replace_and_mark_text_in_range(None, "ni", Some(2..2), w, cx);
                v.replace_and_mark_text_in_range(None, "你", Some(1..1), w, cx);
            });
            e.draw();
            for key in [
                "enter",
                "shift-enter",
                "cmd-enter",
                "cmd-shift-left",
                "up",
                "backspace",
                "delete",
                "cmd-z",
            ] {
                e.key(key);
            }
            assert_eq!(e.value(), "A你B");
            assert_eq!((edits.get(), submits.get()), (0, 0));
            e.update(|v, w, cx| {
                if unmark {
                    v.unmark_text(w, cx);
                } else {
                    v.replace_text_in_range(None, "你", w, cx);
                }
            });
            e.app.run_until_parked();
            e.draw();
            assert_eq!(edits.get(), 1);
            e.key("cmd-z");
            assert_eq!(e.value(), "AB");
            assert_eq!(e.selection(), 1..1);
            e.key("cmd-shift-z");
            assert_eq!(e.value(), "A你B");
            e.key("enter");
            assert_eq!(submits.get(), 1);
            e.key("cmd-enter");
            assert_eq!(submits.get(), 2);
        }
    }

    #[test]
    fn utf16_ranges_and_selection_direction_survive_replacement_undo() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(600., "A👩‍💻中", false, true);
        e.update(|v, w, cx| {
            assert_eq!(v.text_length_utf16(w, cx), Some(7));
            v.set_selected_text_range(6..1, w, cx);
            let selected = v.selected_text_range(false, w, cx).unwrap();
            assert_eq!(selected.range, 1..6);
            assert!(selected.reversed);
        });
        e.paste("X");
        assert_eq!(e.value(), "AX中");
        e.key("cmd-z");
        assert_eq!(e.value(), "A👩‍💻中");
        e.update(|v, w, cx| assert!(v.selected_text_range(false, w, cx).unwrap().reversed));
    }

    #[test]
    fn committed_echo_preserves_history_and_restore_clears_it() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(600., "one", false, true);
        e.paste(" two");
        e.update(|v, _, cx| v.set_value("one two", cx));
        e.key("cmd-z");
        assert_eq!(e.value(), "one");
        e.key("cmd-shift-z");
        e.update(|v, _, cx| v.reset_value("one two", cx));
        e.key("cmd-z");
        assert_eq!(e.value(), "one two");
        e.key("cmd-a");
        e.key("backspace");
        e.update(|v, _, cx| v.clear(cx));
        e.key("cmd-z");
        assert_eq!(e.value(), "");
    }

    #[test]
    fn replacement_typing_is_one_undo_and_restores_directed_selection() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(600., "first\nsecond line", false, true);
        e.select(10..10);
        e.key("cmd-shift-left");
        e.type_text("替换");
        assert_eq!(e.value(), "first\n替换nd line");
        e.key("left");
        e.key("cmd-z");
        assert_eq!(e.value(), "first\nsecond line");
        assert_eq!(e.selection(), 6..10);
        e.update(|v, w, cx| assert!(v.selected_text_range(false, w, cx).unwrap().reversed));
        e.key("cmd-shift-z");
        assert_eq!(e.value(), "first\n替换nd line");
    }

    #[test]
    fn platform_commands_preserve_paragraphs_kill_yank_transpose_and_open_line() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(600., "first\nsecond\nlast", false, true);
        e.select(9..9);
        for (key, selection) in [
            ("alt-up", 6..6),
            ("alt-up", 0..0),
            ("alt-down", 5..5),
            ("alt-shift-down", 5..12),
            ("alt-shift-up", 5..6),
        ] {
            e.key(key);
            assert_eq!(e.selection(), selection, "{key}");
        }
        e.select(8..8);
        e.key("ctrl-k");
        assert_eq!(e.value(), "first\nse\nlast");
        e.key("ctrl-y");
        assert_eq!(e.value(), "first\nsecond\nlast");
        e.key("cmd-z");
        e.key("cmd-z");
        assert_eq!(e.value(), "first\nsecond\nlast");
        assert_eq!(e.selection(), 8..8);
        e.key("ctrl-t");
        assert_eq!(e.value(), "first\nsceond\nlast");
        e.key("cmd-z");
        e.key("ctrl-o");
        assert_eq!(e.value(), "first\nse\ncond\nlast");
        assert_eq!(e.selection(), 8..8);
        e.key("cmd-z");
        e.key("cmd-shift-z");
        assert_eq!(e.selection(), 8..8);
        let value = e.value();
        e.key("tab");
        assert_eq!(e.value(), value);
    }

    #[test]
    fn word_selection_can_reverse_and_graphemes_can_span_rope_chunks() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(600., "one two three", false, true);
        e.select(4..4);
        e.key("alt-shift-right");
        assert_eq!(e.selection(), 4..7);
        e.key("alt-shift-left");
        assert_eq!(e.selection(), 4..4);
        let text = format!("Ae{}Z", "\u{301}".repeat(6000));
        e.update(|v, _, cx| v.reset_value(text.clone(), cx));
        e.draw();
        e.key("backspace");
        e.key("backspace");
        assert_eq!(e.value(), "A");
        e.key("cmd-z");
        assert_eq!(e.value(), text);
    }

    #[test]
    fn content_height_tracks_wrapping_without_idle_layout_events_and_scroll_keeps_caret() {
        let _platform = serial_platform();
        let mut e =
            EditorHarness::new(130., &"alpha bravo charlie delta\n".repeat(20), false, true);
        let height = e.update(|v, _, _| v.content_height().unwrap());
        assert!(height > 100.);
        e.select(3..3);
        e.key("end");
        assert_eq!(e.selection(), 3..3);
        assert!(e.update(|v, _, cx| edit!(v, cx, |state, _scx| state.scroll_offset().y)) < px(0.));
        e.key("home");
        assert_eq!(e.selection(), 3..3);
        assert_eq!(
            e.update(|v, _, cx| edit!(v, cx, |state, _scx| state.scroll_offset().y)),
            px(0.)
        );
        let layouts = Rc::new(Cell::new(0));
        let notifications = Rc::new(Cell::new(0));
        e.update(|v, _, cx| {
            edit!(v, cx, |_state, scx| {
                let notifications = notifications.clone();
                scx.observe(&scx.entity(), move |_, _, _| {
                    notifications.set(notifications.get() + 1)
                })
                .detach();
            })
        });
        e.update(|_, _, cx| {
            let layouts = layouts.clone();
            cx.subscribe(&cx.entity(), move |_, _, _: &ComposerLayoutChanged, _| {
                layouts.set(layouts.get() + 1)
            })
            .detach();
        });
        for _ in 0..5 {
            e.draw();
            e.app.run_until_parked();
        }
        assert_eq!(layouts.get(), 0);
        assert_eq!(
            notifications.get(),
            0,
            "painting an unchanged editor must settle"
        );
        e.update(|v, _, cx| {
            v.reset_value("short", cx);
            assert_eq!(v.content_height().unwrap(), height);
        });
        e.draw();
        assert!(e.update(|v, _, _| v.content_height().unwrap()) < height);
    }

    #[test]
    fn unfocused_projection_and_blur_stop_cursor_notifications() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(240., "draft", false, true);
        // Headless windows start inactive; cursor focus notifications require
        // the same active-window state as an editor receiving real input.
        e.update(|_, window, _| window.activate_window());
        e.app.run_until_parked();
        e.update(|v, window, cx| {
            window.blur();
            v.reset_value("draft restored from core", cx);
        });
        e.draw();
        e.app.run_until_parked();
        let notifications = Rc::new(Cell::new(0));
        e.update(|v, _, cx| {
            edit!(v, cx, |_state, scx| {
                let notifications = notifications.clone();
                scx.observe(&scx.entity(), move |_, _, _| {
                    notifications.set(notifications.get() + 1);
                }).detach();
            });
        });
        for _ in 0..4 {
            e.app.advance_clock(std::time::Duration::from_millis(600));
            e.app.run_until_parked();
        }
        assert_eq!(notifications.get(), 0, "an unfocused projection started a blink loop");
        e.update(|v, window, cx| window.focus(&v.focus_handle(), cx));
        e.app.run_until_parked();
        notifications.set(0);
        for _ in 0..2 {
            e.app.advance_clock(std::time::Duration::from_millis(600));
            e.app.run_until_parked();
        }
        assert!(notifications.get() > 0, "a focused editor must retain its blinking cursor");
        e.update(|_, window, _| window.blur());
        e.app.run_until_parked();
        notifications.set(0);
        for _ in 0..4 {
            e.app.advance_clock(std::time::Duration::from_millis(600));
            e.app.run_until_parked();
        }
        assert_eq!(notifications.get(), 0, "blur did not cancel the cursor timer");
    }

    #[test]
    fn password_single_line_and_non_text_paste_keep_platform_contracts() {
        let _platform = serial_platform();
        let mut e = EditorHarness::new(130., "a密👩‍💻é", true, true);
        e.update(|v, _, cx| v.set_secret(true, cx));
        e.draw();
        e.key("cmd-a");
        e.key("cmd-shift-right");
        e.key("cmd-a");
        e.paste("one\r\ntwo\rthree\nfour");
        assert_eq!(e.value(), "one two three four");
        e.key("shift-enter");
        assert_eq!(e.value(), "one two three four");
        e.key("cmd-z");
        assert_eq!(e.value(), "a密👩‍💻é");
        let files = Rc::new(Cell::new(0));
        e.update(|v, w, cx| {
            let files = files.clone();
            cx.subscribe(&cx.entity(), move |_, _, _: &ComposerFilesPasted, _| {
                files.set(files.get() + 1)
            })
            .detach();
            EntityInputHandler::paste(
                v,
                ClipboardItem {
                    entries: vec![gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                        vec!["/fixture.txt".into()].into(),
                    ))],
                },
                w,
                cx,
            );
        });
        e.app.run_until_parked();
        assert_eq!(files.get(), 1);
        assert_eq!(e.value(), "a密👩‍💻é");
    }

    #[test]
    fn editor_tab_honors_scope_ime_and_scope_removal() {
        let _platform = serial_platform();
        struct ScopeView {
            first: Entity<ComposerInput>,
            last: Entity<ComposerInput>,
            scope: FocusHandle,
            outside: FocusHandle,
            trapped: bool,
        }
        impl Render for ScopeView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let group = div()
                    .id("editor-scope")
                    .flex()
                    .flex_col()
                    .child(div().h(px(32.)).child(self.first.clone()))
                    .child(div().h(px(32.)).child(self.last.clone()));
                let group = if self.trapped {
                    crate::modal::trap_focus(group, &self.scope)
                } else {
                    group
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("outside-scope")
                            .h(px(32.))
                            .track_focus(&self.outside)
                            .child("Outside"),
                    )
                    .child(group)
            }
        }
        let mut app = HeadlessAppContext::new(gpui_platform::current_platform(true).text_system());
        app.update(|cx| {
            gpui_base::init(cx);
            shortcuts::bind_keys(cx, true);
        });
        let window = app
            .open_window(size(px(300.), px(200.)), |_, cx| {
                let first = cx.new(|cx| ComposerInput::new("First", cx).single_line());
                let last = cx.new(|cx| ComposerInput::new("Last", cx).single_line());
                cx.new(|cx| ScopeView {
                    first,
                    last,
                    scope: cx.focus_handle(),
                    outside: cx.focus_handle().tab_stop(true),
                    trapped: true,
                })
            })
            .unwrap();
        let pump = |app: &mut HeadlessAppContext| {
            app.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))
                .unwrap();
            app.run_until_parked();
        };
        pump(&mut app);
        pump(&mut app);
        window
            .update(&mut app, |v, w, cx| {
                w.focus(&v.last.read(cx).focus_handle(), cx)
            })
            .unwrap();
        for (key, expect_first) in [("tab", true), ("shift-tab", false)] {
            app.update_window(window.into(), |_, w, cx| {
                w.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
            })
            .unwrap();
            pump(&mut app);
            let inside = window
                .update(&mut app, |v, w, cx| {
                    let input = if expect_first { &v.first } else { &v.last };
                    input.read(cx).focus_handle().is_focused(w)
                })
                .unwrap();
            assert!(inside, "{key} escaped the focus scope");
        }
        window
            .update(&mut app, |v, w, cx| {
                w.focus(&v.first.read(cx).focus_handle(), cx);
                v.first.update(cx, |input, cx| {
                    input.replace_and_mark_text_in_range(None, "ni", Some(2..2), w, cx)
                });
            })
            .unwrap();
        pump(&mut app);
        for key in ["tab", "shift-tab"] {
            app.update_window(window.into(), |_, w, cx| {
                w.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
            })
            .unwrap();
            pump(&mut app);
            assert!(
                window
                    .update(&mut app, |v, w, cx| v
                        .first
                        .read(cx)
                        .focus_handle()
                        .is_focused(w))
                    .unwrap(),
                "IME lost focus on {key}"
            );
        }
        window
            .update(&mut app, |v, w, cx| {
                v.first.update(cx, |input, cx| input.unmark_text(w, cx))
            })
            .unwrap();
        pump(&mut app);
        window
            .update(&mut app, |v, w, cx| {
                v.trapped = false;
                w.focus(&v.last.read(cx).focus_handle(), cx);
                cx.notify();
            })
            .unwrap();
        pump(&mut app);
        app.update_window(window.into(), |_, w, cx| {
            w.dispatch_keystroke(Keystroke::parse("tab").unwrap(), cx);
        })
        .unwrap();
        pump(&mut app);
        assert!(
            window
                .update(&mut app, |v, w, _| v.outside.is_focused(w))
                .unwrap(),
            "Removed scope retained a traversal handler"
        );
    }

    #[test]
    #[ignore = "run separately from builds to compare native text-layout CPU costs"]
    fn composer_draw_cpu_budget() {
        let _platform = serial_platform();
        for width in [280., 680.] {
            let mut e = EditorHarness::new(
                width,
                &"中英文 composer 👩‍💻 é words\n".repeat(30),
                false,
                true,
            );
            e.select(12..160);
            let mut samples = Vec::new();
            for frame in 0..220 {
                let start = Instant::now();
                e.draw();
                if frame >= 20 {
                    samples.push(start.elapsed().as_secs_f64() * 1000.);
                }
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "composer width={width} frames=200 p95_ms={:.3} p99_ms={:.3}",
                samples[189], samples[197]
            );
            assert!(samples[189] < 8.33);
        }
    }
}
