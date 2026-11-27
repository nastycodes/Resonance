use std::{
    mem::size_of,
    ptr::{null, null_mut},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc,
    },
    thread,
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, POINT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::{
            NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
            DispatchMessageW, GetCursorPos, GetMessageW, HWND_MESSAGE, IMAGE_ICON, LR_DEFAULTSIZE,
            LR_SHARED, LoadImageW, MF_STRING, MSG, PostMessageW, RegisterClassW, SW_RESTORE,
            SetForegroundWindow, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
            TranslateMessage, WM_CLOSE, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_USER, WNDCLASSW,
        },
    },
};

const TRAY_CALLBACK: u32 = WM_USER + 1;
const EXIT_MENU_ID: usize = 1;

static APP_WINDOW: AtomicIsize = AtomicIsize::new(0);
static EXIT_REQUESTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

pub fn exit_requested() -> Arc<AtomicBool> {
    EXIT_REQUESTED
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

pub fn start(app_window: isize) {
    APP_WINDOW.store(app_window, Ordering::SeqCst);
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);

    thread::Builder::new()
        .name("resonance-tray".into())
        .spawn(move || {
            if let Err(error) = run(&ready_sender) {
                let _ = ready_sender.send(Err(error));
            }
        })
        .expect("Failed to start the Resonance tray thread");

    match ready_receiver.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("Failed to create the Resonance tray icon: {error}"),
        Err(error) => panic!("Resonance tray thread stopped during startup: {error}"),
    }
}

fn run(ready_sender: &mpsc::SyncSender<Result<(), String>>) -> Result<(), String> {
    unsafe {
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }

        let class_name: Vec<u16> = "ResonanceTrayWindow\0".encode_utf16().collect();
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(tray_window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&window_class) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }

        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            null(),
        );
        if window.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }

        let icon = LoadImageW(
            instance,
            1usize as *const u16,
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        );
        if icon.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }

        let mut icon_data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: TRAY_CALLBACK,
            hIcon: icon,
            ..Default::default()
        };
        let tooltip: Vec<u16> = "Resonance\0".encode_utf16().collect();
        icon_data.szTip[..tooltip.len()].copy_from_slice(&tooltip);

        if Shell_NotifyIconW(NIM_ADD, &icon_data) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }

        ready_sender
            .send(Ok(()))
            .map_err(|error| error.to_string())?;
        let mut message: MSG = std::mem::zeroed();
        loop {
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result <= 0 {
                if result == -1 {
                    eprintln!(
                        "Resonance tray message loop failed: {}",
                        std::io::Error::last_os_error()
                    );
                }
                break;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data);
        Ok(())
    }
}

unsafe extern "system" fn tray_window_proc(
    window: HWND,
    message: u32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        TRAY_CALLBACK => match lparam as u32 {
            WM_LBUTTONUP => {
                let app_window = APP_WINDOW.load(Ordering::SeqCst) as HWND;
                unsafe {
                    ShowWindow(app_window, SW_RESTORE);
                    SetForegroundWindow(app_window);
                }
                0
            }
            WM_RBUTTONUP => {
                show_exit_menu(window);
                0
            }
            _ => 0,
        },
        _ => unsafe { DefWindowProcW(window, message, _wparam, lparam) },
    }
}

fn show_exit_menu(window: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            eprintln!("Failed to create the Resonance tray menu");
            return;
        }

        let exit_label: Vec<u16> = "Beenden\0".encode_utf16().collect();
        if AppendMenuW(menu, MF_STRING, EXIT_MENU_ID, exit_label.as_ptr()) == 0 {
            eprintln!("Failed to add the exit command to the Resonance tray menu");
            DestroyMenu(menu);
            return;
        }

        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor) != 0 {
            SetForegroundWindow(window);
            let command = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD,
                cursor.x,
                cursor.y,
                0,
                window,
                null(),
            );
            PostMessageW(window, WM_NULL, 0, 0);
            if command == EXIT_MENU_ID as i32 {
                if let Some(exit_requested) = EXIT_REQUESTED.get() {
                    exit_requested.store(true, Ordering::SeqCst);
                }
                let app_window = APP_WINDOW.load(Ordering::SeqCst) as HWND;
                if PostMessageW(app_window, WM_CLOSE, 0, 0) == 0 {
                    if let Some(exit_requested) = EXIT_REQUESTED.get() {
                        exit_requested.store(false, Ordering::SeqCst);
                    }
                    eprintln!("Failed to send the exit command to the Resonance window");
                }
            }
        } else {
            eprintln!("Failed to locate the cursor for the Resonance tray menu");
        }
        DestroyMenu(menu);
    }
}
