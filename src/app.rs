use std::borrow::Cow;

use gpui::prelude::*;
use gpui::{KeyBinding, WindowBounds, WindowOptions, size};
use sqlly_textbox::{Commit, InsertNewline, install_text_box_keybindings};

use crate::app_state::AppState;

pub fn run() {
    #[cfg(target_os = "windows")]
    let Some(_instance_lock) = crate::single_instance::acquire()
        .expect("Failed to check whether Resonance is already running")
    else {
        return;
    };

    gpui_platform::application()
        .with_assets(crate::components::icon::IconAssets)
        .run(|cx| {
            install_text_box_keybindings(cx);
            cx.bind_keys([
                KeyBinding::new("enter", Commit, Some("TextBox")),
                KeyBinding::new("shift-enter", InsertNewline, Some("TextBox")),
            ]);
            cx.set_keyring_label("Resonance");
            cx.text_system()
                .add_fonts(vec![
                    Cow::Borrowed(include_bytes!(
                        "../assets/fonts/BricolageGrotesque-Variable.ttf"
                    )),
                    Cow::Borrowed(include_bytes!(
                        "../assets/fonts/InstrumentSans-Variable.ttf"
                    )),
                ])
                .expect("Failed to load the bundled Resonance fonts");
            let window_bounds = cx.primary_display().map(|display| {
                let display_size = display.bounds().size;
                WindowBounds::centered(
                    size(display_size.width * 0.8, display_size.height * 0.8),
                    cx,
                )
            });
            let _ = cx.open_window(
                WindowOptions {
                    window_bounds,
                    ..WindowOptions::default()
                },
                |window, cx| {
                    window.set_window_title("Resonance");
                    #[cfg(target_os = "windows")]
                    {
                        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

                        let raw_window_handle = HasWindowHandle::window_handle(window)
                            .expect("Failed to get the Resonance window handle")
                            .as_raw();
                        let hwnd = match raw_window_handle {
                            RawWindowHandle::Win32(handle) => handle.hwnd.get() as isize,
                            _ => panic!("Resonance requires a Win32 window handle on Windows"),
                        };
                        let exit_requested = crate::tray::exit_requested();
                        window.on_window_should_close(cx, move |_, _| {
                            if exit_requested.swap(false, std::sync::atomic::Ordering::SeqCst) {
                                true
                            } else {
                                unsafe {
                                    windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                                        hwnd as windows_sys::Win32::Foundation::HWND,
                                        windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
                                    );
                                }
                                false
                            }
                        });
                        crate::tray::start(hwnd);
                    }
                    let app = cx.new(AppState::new);
                    let focus = app.read(cx).input_focus.clone();
                    window.focus(&focus, cx);
                    app
                },
            );
        });
}
