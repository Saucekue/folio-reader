#![windows_subsystem = "windows"]

mod book;
mod library;
mod window;

use std::{
    cell::{Cell, RefCell}, env, ffi::{OsString, c_void},
    os::windows::ffi::OsStringExt, path::PathBuf, rc::Rc,
};

use book::Result;
use library::{Reader, Request};
use serde::Deserialize;
use serde_json::{Value, json};
use windows_sys::Win32::{Foundation::E_NOINTERFACE, UI::{
    Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, OPENFILENAMEW, OFN_ALLOWMULTISELECT, OFN_EXPLORER,
        OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
    },
    WindowsAndMessaging::{MB_ICONERROR, MessageBoxW, WM_ERASEBKGND, WM_SETFOCUS},
}};
use windows_webview::{
    Controller, ControllerOptions, Environment, EnvironmentOptions, MoveFocusReason,
    PreferredColorScheme, WebView,
};
use windows_window::Window;
use window::Fullscreen;

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn pick_files(owner: *mut c_void, title: &str, filter: &str, multiple: bool) -> Result<Vec<PathBuf>> {
    let mut paths = vec![0u16; 65536];
    let filter = wide(filter);
    let title = wide(title);
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.nFilterIndex = 1;
    dialog.lpstrFile = paths.as_mut_ptr();
    dialog.nMaxFile = paths.len() as u32;
    dialog.lpstrTitle = title.as_ptr();
    dialog.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    if multiple { dialog.Flags |= OFN_ALLOWMULTISELECT; }
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        let code = unsafe { CommDlgExtendedError() };
        if code == 0 { return Ok(Vec::new()); }
        return Err(format!("The file picker returned Windows error {code:#x}.").into());
    }
    let mut paths = paths.split(|unit| *unit == 0).take_while(|part| !part.is_empty())
        .map(|part| PathBuf::from(OsString::from_wide(part)));
    let Some(first) = paths.next() else { return Ok(Vec::new()); };
    let rest: Vec<_> = paths.map(|path| first.join(path)).collect();
    Ok(if rest.is_empty() { vec![first] } else { rest })
}

fn run() -> Result<()> {
    let state_file = env::current_exe()?.with_file_name("reading-state.json");
    let reader = RefCell::new(Reader::new(state_file));
    let initial_theme = reader.borrow().theme();
    let theme = Rc::new(Cell::new(initial_theme));
    let browser_data = env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(env::temp_dir)
        .join("FolioReader").join("WebView");
    let holder: Rc<RefCell<Option<Controller>>> = Rc::new(RefCell::new(None));
    let resize = holder.clone();
    let focus = holder.clone();
    let paint_theme = theme.clone();
    let window = Window::new("Folio Reader").size(1190, 820).on_message(move |hwnd, message, wparam, _| {
        if message == WM_ERASEBKGND {
            return window::paint_background(hwnd, wparam as _, window::background_color(paint_theme.get()));
        }
        if message == WM_SETFOCUS && let Some(controller) = focus.borrow().as_ref() {
            let _ = controller.move_focus(MoveFocusReason::Programmatic);
        }
        None
    }).on_resize(move |width, height| {
        if let Some(controller) = resize.borrow().as_ref() {
            let _ = controller.set_bounds(0, 0, width, height);
        }
    }).create()?;
    window::set_icon(window.hwnd());
    let env = Environment::with_options(&EnvironmentOptions::new().user_data_folder(browser_data.to_string_lossy()))
        .map_err(|error| format!("Folio needs the Windows WebView2 Runtime. {error}"))?;
    let color = window::background_color(initial_theme);
    let options = ControllerOptions::new().default_background_color(color);
    let controller = match env.create_controller_with_options(&window, &options) {
        Err(error) if error.code().0 == E_NOINTERFACE => {
            let controller = env.create_controller(&window)?;
            let _ = controller.set_default_background_color(color);
            controller
        }
        result => result?,
    };
    let (width, height) = window.client_size();
    controller.set_bounds(0, 0, width, height)?;
    controller.set_visible(true)?;
    controller.set_allow_external_drop(true)?;
    let webview = Rc::new(controller.webview()?);
    set_browser_theme(&webview, initial_theme);
    controller.move_focus(MoveFocusReason::Programmatic)?;
    *holder.borrow_mut() = Some(controller);
    let settings = webview.settings()?;
    settings.set_dev_tools_enabled(false)?;
    settings.set_browser_accelerator_keys_enabled(false)?;

    let fullscreen = RefCell::new(Fullscreen::default());
    let owner = window.hwnd();
    let send = Rc::downgrade(&webview);
    let _messages = webview.on_web_message_received(move |args| {
        if args.source() != "about:blank" { return; }
        if let Some(webview) = send.upgrade() {
            respond(&webview, &reader, &fullscreen, owner, &args.web_message_as_json());
            let saved_theme = reader.borrow().theme();
            if theme.replace(saved_theme) != saved_theme {
                if let Some(controller) = holder.borrow().as_ref() {
                    let _ = controller.set_default_background_color(window::background_color(saved_theme));
                }
                set_browser_theme(&webview, saved_theme);
            }
        }
    })?;
    let first_navigation = Cell::new(true);
    let _navigation = webview.on_navigation_starting(move |args| {
        let uri = args.uri();
        let initial_shell = first_navigation.replace(false)
            && (uri == "about:blank" || uri.starts_with("data:text/html"));
        if !initial_shell { let _ = args.set_cancel(true); }
    })?;
    let _popups = webview.on_new_window_requested(|args| { let _ = args.set_handled(true); })?;
    let html = include_str!("../ui/index.html")
        .replacen("data-theme=\"dark\"", &format!("data-theme=\"{initial_theme}\""), 1)
        .replace("/* FOLIO_CSS */", include_str!("../ui/style.css"))
        .replace("/* FOLIO_JS */", include_str!("../ui/reader.js"));
    webview.navigate_to_string(&html)?;
    windows_window::run();
    Ok(())
}

fn set_browser_theme(webview: &WebView, theme: &str) {
    if let Ok(profile) = webview.profile() {
        let scheme = if theme == "light" { PreferredColorScheme::Light } else { PreferredColorScheme::Dark };
        let _ = profile.set_preferred_color_scheme(scheme);
    }
}

fn respond(
    webview: &WebView, reader: &RefCell<Reader>, fullscreen: &RefCell<Fullscreen>,
    owner: *mut c_void, message: &str,
) {
    if message.len() > 280 * 1024 { return; }
    #[derive(Deserialize)]
    struct Envelope { id: u32, request: Request }
    let Ok(Envelope { id, request }) = serde_json::from_str(message) else { return };
    let result = match request {
        Request::Fullscreen { enabled } => fullscreen.borrow_mut().set(owner, enabled).map(|()| Value::Null),
        Request::Import { .. } => pick_files(owner, "Add EPUBs to the library", "EPUB books (*.epub)\0*.epub\0", true)
            .and_then(|paths| reader.borrow_mut().handle(request, paths)),
        Request::PickCover { .. } | Request::PickFolderImage { .. } => pick_files(owner, "Choose an image",
            "Images (*.png;*.jpg;*.jpeg;*.webp;*.gif)\0*.png;*.jpg;*.jpeg;*.webp;*.gif\0", false)
            .and_then(|paths| reader.borrow_mut().handle(request, paths)),
        request => reader.borrow_mut().handle(request, Vec::new()),
    };
    let response = match result {
        Ok(value) => json!({"id": id, "value": value}),
        Err(error) => json!({"id": id, "error": error.to_string()}),
    };
    let _ = webview.post_web_message_as_json(&response.to_string());
}

fn main() {
    if let Err(error) = run() {
        let text = wide(&error.to_string());
        let title = wide("Folio Reader");
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_ICONERROR); }
    }
}
