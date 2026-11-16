//! Snipt: a tiny Win+V-style launcher for your own snippets.
//! Pure Win32 + GDI, no runtime, no web view.

#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]

mod anim;
mod draw;
mod hotkey;
mod paste;
mod snippets;
mod usage;

use draw::{EDIT_H, EX, E_BODY, E_TITLE, Fonts, MAX_ROWS, MIN_ROWS, SHORTCUT_H, WIDTH};
use hotkey::Hotkey;
use snippets::{Kind, Snippet};
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId, Sleep};
use windows_sys::Win32::UI::Controls::{
    EM_GETLINECOUNT, EM_REPLACESEL, EM_SETLIMITTEXT, EM_SETSEL, SetWindowTheme, ShowScrollBar, WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

// ---------------------------------------------------------------- constants

const WM_TRAY: u32 = WM_APP + 1;
const WM_SHOW: u32 = WM_APP + 2;
/// From the keyboard hook of the shortcut screen: wParam = key, lParam = modifiers.
const WM_SHORTCUT_KEY: u32 = WM_APP + 3;
const WM_SHORTCUT_KEYUP: u32 = WM_APP + 4;
const HOTKEY_ID: i32 = 1;
const TIMER_RESTORE: usize = 1;
const TIMER_ANIM: usize = 2;
const TIMER_BLINK: usize = 3;

const ID_OPEN: usize = 100;
const ID_MANAGE: usize = 101;
const ID_FILE: usize = 102;
const ID_AUTOSTART: usize = 103;
const ID_EXIT: usize = 104;
const ID_SHORTCUT: usize = 105;
const ID_TITLE: usize = 200;
const ID_BODY: usize = 201;

pub const PLACEHOLDERS: [&str; 5] = ["{cursor}", "{date}", "{time}", "{datetime}", "{clipboard}"];

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ---------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Search and paste.
    Browse,
    /// Same list, but Enter/click edits and rows can be deleted.
    Manage,
    /// Create or edit one snippet.
    Edit,
    /// "Change shortcut": waits for the new key combination.
    Shortcut,
}

impl Mode {
    /// Browse or Manage: the search field and the list are on screen.
    pub fn is_list(self) -> bool {
        matches!(self, Mode::Browse | Mode::Manage)
    }
}

/// Something clickable, recorded while painting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    Row(usize),
    Gear,
    New,
    EditRow(usize),
    DeleteRow(usize),
    PinRow(usize),
    ConfirmDelete,
    CancelDelete,
    SetKind(Kind),
    Insert(usize),
    Save,
    Cancel,
    EditorDelete,
    TogglePin,
    ResetShortcut,
    Done,
}

pub struct Editor {
    /// Index into `App::all`, or None for a new snippet.
    pub index: Option<usize>,
    pub kind: Kind,
    pub error: String,
    pub confirm_delete: bool,
    pub pinned: bool,
    back: Mode,
}

/// State of the "Change shortcut" screen.
#[derive(Default)]
pub struct ShortcutUi {
    /// On screen and the old shortcut is temporarily released.
    active: bool,
    /// Keys held right now (or the last combination tried), as keycaps.
    pub preview: Vec<String>,
    pub message: String,
    pub error: bool,
    /// A new shortcut was registered and saved.
    pub saved: bool,
    /// Modifiers held, as seen by the keyboard hook.
    held: HOT_KEY_MODIFIERS,
}

pub struct App {
    pub hwnd: HWND,
    /// Window that had focus before Snipt opened; paste goes there.
    prev: HWND,
    pub all: Vec<Snippet>,
    pub hits: Vec<usize>,
    pub query: String,
    pub sel: usize,
    pub top: usize,
    /// Visible list rows, fitted to the number of snippets.
    pub rows: usize,
    pub mode: Mode,
    pub editor: Editor,
    /// Snippet (index into `all`) waiting for delete confirmation in Manage.
    pub confirm: Option<usize>,
    pub hover: Option<Action>,
    pub regions: Vec<(RECT, Action)>,
    pub title_edit: HWND,
    pub body_edit: HWND,
    last_focus: HWND,
    field_brush: HBRUSH,
    dpi: u32,
    pub fonts: Fonts,
    pub text_face: &'static str,
    pub icon_face: &'static str,
    restore: Option<String>,
    /// The shortcut in use (or chosen), and its label for menus and the UI.
    hotkey_now: Option<Hotkey>,
    pub hotkey: String,
    pub shortcut: ShortcutUi,
    usage: usage::Usage,
    /// Keyboard hook, only while the shortcut screen is open.
    key_hook: HHOOK,
    taskbar_created: u32,
    icon: HICON,
    pub anim: anim::Anim,
    /// Search caret: shown or hidden (blink phase), and where it was last drawn.
    pub caret_on: bool,
    pub caret_rect: RECT,
}

impl App {
    pub fn s(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    pub fn r(&self, l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: self.s(l), top: self.s(t), right: self.s(r), bottom: self.s(b) }
    }
    /// The current shortcut as keycap labels, e.g. ["Ctrl", "Shift", "Space"].
    pub fn hotkey_parts(&self) -> Option<Vec<String>> {
        self.hotkey_now.map(|h| h.parts())
    }
}

// Single UI thread; the window procedure is the only user of this.
static mut APP: *mut App = null_mut();
fn app() -> &'static mut App {
    unsafe { &mut *APP }
}

// ---------------------------------------------------------------- entry

fn main() {
    // Debug builds have no console; keep panics readable.
    #[cfg(debug_assertions)]
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write(std::env::temp_dir().join("snipt-panic.txt"), info.to_string());
    }));
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let class = wide("SniptWindow");

        // One instance only; a second launch just opens the first one.
        CreateMutexW(null(), 0, wide("Local\\Snipt.SingleInstance").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let other = FindWindowW(class.as_ptr(), null());
            if !other.is_null() {
                PostMessageW(other, WM_SHOW, 0, 0);
            }
            return;
        }

        let first_run = !snippets::file_path().exists();
        let inst = GetModuleHandleW(null());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: inst,
            hIcon: app_icon(GetSystemMetrics(SM_CXICON)),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: app_icon(GetSystemMetrics(SM_CXSMICON)),
        };
        RegisterClassExW(&wc);

        let text_face = draw::pick_font(&["Segoe UI Variable Text", "Segoe UI"]);
        let icon_face = draw::pick_font(&["Segoe Fluent Icons", "Segoe MDL2 Assets"]);
        APP = Box::into_raw(Box::new(App {
            hwnd: null_mut(),
            prev: null_mut(),
            all: snippets::load(),
            hits: Vec::new(),
            query: String::new(),
            sel: 0,
            top: 0,
            rows: MIN_ROWS,
            mode: Mode::Browse,
            editor: Editor { index: None, kind: Kind::Template, error: String::new(), confirm_delete: false, pinned: false, back: Mode::Browse },
            confirm: None,
            hover: None,
            regions: Vec::new(),
            title_edit: null_mut(),
            body_edit: null_mut(),
            last_focus: null_mut(),
            field_brush: CreateSolidBrush(draw::SURFACE),
            dpi: 0,
            fonts: Fonts::empty(),
            text_face,
            icon_face,
            restore: None,
            hotkey_now: None,
            hotkey: "no hotkey (use the tray icon)".to_string(),
            shortcut: ShortcutUi::default(),
            usage: usage::Usage::load(),
            key_hook: null_mut(),
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            icon: app_icon(GetSystemMetrics(SM_CXSMICON)),
            anim: anim::Anim::new(),
            caret_on: true,
            caret_rect: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        }));

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            wide("Snipt").as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            WIDTH,
            draw::list_height(MIN_ROWS),
            null_mut(),
            null_mut(),
            inst,
            null(),
        );
        let a = app();
        a.hwnd = hwnd;

        // Native text boxes for the editor (selection, undo, IME, Ctrl+V for free).
        let edit = wide("EDIT");
        a.title_edit = CreateWindowExW(0, edit.as_ptr(), null(), WS_CHILD | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            0, 0, 0, 0, hwnd, ID_TITLE as _, inst, null());
        a.body_edit = CreateWindowExW(0, edit.as_ptr(), null(),
            WS_CHILD | WS_TABSTOP | WS_VSCROLL | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32,
            0, 0, 0, 0, hwnd, ID_BODY as _, inst, null());
        SendMessageW(a.title_edit, EM_SETLIMITTEXT, 120, 0);
        SetWindowTheme(a.body_edit, wide("DarkMode_Explorer").as_ptr(), null()); // dark scrollbar

        // Windows 11 look: dark frame, small rounded corners, subtle border.
        let on: i32 = 1;
        let corners: i32 = 3; // DWMWCP_ROUNDSMALL
        let border: u32 = draw::BORDER;
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &on as *const _ as _, 4);
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &corners as *const _ as _, 4);
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, &border as *const _ as _, 4);

        // The shortcut you chose, else the first free default.
        let saved = hotkey::load_saved();
        let saved_ok = saved.is_some_and(|h| register(h));
        if !saved_ok {
            register_default();
        }

        // If "Start with Windows" is on, point it at this copy of the exe, so
        // moving snipt.exe to another folder doesn't silently break autostart.
        if autostart_enabled() {
            set_autostart(true);
        }

        tray(NIM_ADD, None);
        if let (Some(h), false) = (saved, saved_ok) {
            let msg = format!("{} is in use by another app, so Snipt uses {} for now. Change it from the tray menu.", h.label(), a.hotkey);
            tray(NIM_MODIFY, Some(&msg));
        } else if first_run {
            let msg = format!("Snipt is running. Press {} to open it.", a.hotkey);
            tray(NIM_MODIFY, Some(&msg));
        }

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            // Editor shortcuts are caught before the text boxes see them.
            if msg.message == WM_KEYDOWN && a.mode == Mode::Edit && editor_key(msg.wParam as u16) {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ---------------------------------------------------------------- window procedure

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if APP.is_null() || app().hwnd.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let a = app();
    match msg {
        WM_HOTKEY if a.mode == Mode::Shortcut && shown() => {} // just picked it
        WM_HOTKEY => {
            if !shown() {
                show(Mode::Browse);
            } else if a.mode == Mode::Edit {
                force_foreground(hwnd); // never throw away an edit in progress
            } else {
                hide(true);
            }
        }
        WM_SHOW => show(Mode::Browse),
        WM_TRAY => match lp as u32 {
            WM_LBUTTONUP => show(Mode::Browse),
            WM_RBUTTONUP => tray_menu(),
            _ => {}
        },
        WM_COMMAND if lp != 0 => {
            // Notifications from the editor text boxes.
            match ((wp >> 16) & 0xffff) as u32 {
                EN_SETFOCUS => {
                    a.last_focus = lp as HWND;
                    redraw();
                }
                EN_KILLFOCUS => redraw(),
                EN_CHANGE => {
                    update_body_scrollbar();
                    if !a.editor.error.is_empty() {
                        a.editor.error.clear();
                        redraw();
                    }
                }
                _ => {}
            }
        }
        WM_COMMAND => match wp & 0xffff {
            ID_OPEN => show(Mode::Browse),
            ID_MANAGE => show(Mode::Manage),
            ID_FILE => open_file(),
            ID_AUTOSTART => set_autostart(!autostart_enabled()),
            ID_SHORTCUT => show(Mode::Shortcut),
            ID_EXIT => {
                DestroyWindow(hwnd);
            }
            _ => {}
        },
        WM_CTLCOLOREDIT => {
            let dc = wp as HDC;
            SetTextColor(dc, draw::TEXT);
            SetBkColor(dc, draw::SURFACE);
            return a.field_brush as LRESULT;
        }
        WM_ACTIVATE => {
            if (wp & 0xffff) as u32 == WA_INACTIVE {
                // Clicking elsewhere closes the popup, like Win+V. The editor
                // stays open so you can copy text from other apps into it.
                if a.mode != Mode::Edit && !a.anim.closing {
                    hide(false); // focus already went where you clicked
                }
            } else if a.mode == Mode::Edit {
                SetFocus(if a.last_focus.is_null() { a.title_edit } else { a.last_focus });
            }
        }
        WM_KEYDOWN => {
            restart_blink();
            on_key(wp as u16)
        }
        // Alt combinations arrive as "system" keys.
        WM_SHORTCUT_KEY => shortcut_key(wp as u16, lp as HOT_KEY_MODIFIERS),
        WM_SHORTCUT_KEYUP => shortcut_keyup(lp as HOT_KEY_MODIFIERS),
        WM_SYSKEYDOWN if a.mode == Mode::Shortcut => shortcut_key(wp as u16, held_mods()),
        WM_SYSCHAR if a.mode == Mode::Shortcut => {}
        WM_KEYUP | WM_SYSKEYUP if a.mode == Mode::Shortcut => shortcut_keyup(held_mods()),
        WM_CHAR if a.mode.is_list() => {
            let c = wp as u32;
            if c >= 0x20 && c != 0x7f && GetKeyState(VK_CONTROL as i32) >= 0 {
                if let Some(ch) = char::from_u32(c) {
                    a.query.push(ch);
                    a.confirm = None;
                    refilter();
                }
            }
        }
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut tme);
            let act = hit(lp);
            let mut dirty = act != a.hover;
            if dirty && !matches!(act, None | Some(Action::Row(_)) | Some(Action::None)) {
                a.anim.hover = anim::Tween::from_to(0.0, 1.0, a.anim.ms(anim::HOVER_MS));
                animate();
            }
            a.hover = act;
            if let Some(Action::Row(i)) = act {
                if i != a.sel && a.confirm.is_none() {
                    a.sel = i;
                    dirty = true;
                }
            }
            if dirty {
                redraw();
            }
        }
        WM_MOUSELEAVE => {
            if a.hover.take().is_some() {
                redraw();
            }
        }
        WM_LBUTTONDOWN => {
            if let Some(act) = hit(lp) {
                click(act);
            }
        }
        WM_MOUSEWHEEL if a.mode.is_list() => {
            let delta = ((wp >> 16) & 0xffff) as i16;
            let max_top = a.hits.len().saturating_sub(a.rows);
            a.top = if delta > 0 { a.top.saturating_sub(1) } else { (a.top + 1).min(max_top) };
            a.sel = a.sel.clamp(a.top, (a.top + a.rows - 1).min(a.hits.len().saturating_sub(1)));
            redraw();
        }
        WM_TIMER if wp == TIMER_ANIM => tick(),
        WM_TIMER if wp == TIMER_BLINK => {
            if !shown() || !a.mode.is_list() {
                stop_blink();
            } else {
                a.caret_on = !a.caret_on;
                InvalidateRect(hwnd, &a.caret_rect, 0); // just the caret's pixels
            }
        }
        WM_TIMER if wp == TIMER_RESTORE => {
            KillTimer(hwnd, TIMER_RESTORE);
            if let Some(text) = a.restore.take() {
                paste::set_text(hwnd, &text);
            }
        }
        WM_DPICHANGED => {
            set_dpi((wp & 0xffff) as u32);
            let r = &*(lp as *const RECT);
            a.anim.height = anim::Tween::at(height() as f32);
            a.anim.applied_h = a.s(height());
            SetWindowPos(hwnd, null_mut(), r.left, r.top, a.s(WIDTH), a.anim.applied_h, SWP_NOZORDER | SWP_NOACTIVATE);
            layout_editor();
        }
        WM_ERASEBKGND => return 1,
        WM_PAINT => draw::paint(a),
        WM_DESTROY => {
            tray(NIM_DELETE, None);
            PostQuitMessage(0);
        }
        m if m == a.taskbar_created && m != 0 => tray(NIM_ADD, None), // Explorer restarted
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

fn hit(lp: LPARAM) -> Option<Action> {
    let x = (lp & 0xffff) as i16 as i32;
    let y = ((lp >> 16) & 0xffff) as i16 as i32;
    // Last drawn wins, so buttons on top of a row take precedence.
    app().regions.iter().rev().find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|&(_, a)| a)
}

unsafe fn click(act: Action) {
    let a = app();
    match act {
        Action::None => {}
        Action::Row(i) => {
            a.sel = i;
            if a.mode == Mode::Manage {
                open_editor(a.hits.get(i).copied());
            } else {
                choose(false);
            }
        }
        Action::Gear => set_mode(if a.mode == Mode::Manage { Mode::Browse } else { Mode::Manage }),
        Action::New => open_editor(None),
        Action::EditRow(si) => open_editor(Some(si)),
        Action::DeleteRow(si) => {
            a.confirm = Some(si);
            redraw();
        }
        Action::PinRow(si) => toggle_pin(si),
        Action::TogglePin => {
            a.editor.pinned = !a.editor.pinned;
            redraw();
        }
        Action::ResetShortcut => reset_shortcut(),
        Action::Done => hide(true),
        Action::ConfirmDelete => {
            if let Some(si) = a.confirm.take() {
                delete(si);
            }
        }
        Action::CancelDelete => {
            a.confirm = None;
            redraw();
        }
        Action::SetKind(k) => {
            a.editor.kind = k;
            a.editor.error.clear();
            redraw();
        }
        Action::Insert(i) => {
            SendMessageW(a.body_edit, EM_REPLACESEL, 1, wide(PLACEHOLDERS[i]).as_ptr() as LPARAM);
            SetFocus(a.body_edit);
        }
        Action::Save => save_editor(),
        Action::Cancel => close_editor(),
        Action::EditorDelete => {
            if a.editor.confirm_delete {
                if let Some(i) = a.editor.index {
                    close_editor();
                    delete(i);
                }
            } else {
                a.editor.confirm_delete = true;
                redraw();
            }
        }
    }
}

// ---------------------------------------------------------------- show / hide / size

unsafe fn show(mode: Mode) {
    let a = app();
    if shown() && a.mode == Mode::Edit {
        return force_foreground(a.hwnd);
    }
    finish_close(); // reopened mid fade-out: start fresh
    if shown() && a.mode == Mode::Shortcut && mode != Mode::Shortcut {
        leave_shortcut();
    }
    let fg = GetForegroundWindow();
    a.prev = if fg == a.hwnd { null_mut() } else { fg };
    a.all = snippets::load(); // tiny file; reloading picks up outside edits
    a.query.clear();
    a.confirm = None;
    a.hover = None;
    a.mode = mode;
    refilter();
    if mode == Mode::Shortcut {
        begin_shortcut();
    }

    let anchor = anchor_point(a.prev);
    let mon = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
    let (mut dx, mut dy) = (96, 96);
    GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    set_dpi(dx);
    a.rows = a.all.len().clamp(MIN_ROWS, MAX_ROWS);

    let work = work_area(anchor);
    let (w, h, m) = (a.s(WIDTH), a.s(height()), a.s(8));
    let mut y = anchor.y + m;
    if y + h > work.bottom - m {
        y = anchor.y - h - a.s(28); // not enough room below: open above
    }
    let y = y.clamp(work.top + m, (work.bottom - h - m).max(work.top));
    let x = anchor.x.clamp(work.left + m, (work.right - w - m).max(work.left));

    a.anim.enabled = anim::system_enabled();
    a.anim.height = anim::Tween::at(height() as f32);
    a.anim.applied_h = h;
    a.anim.base_y = y;
    let mut start_y = y;
    if a.anim.enabled {
        // Fade in + short upward slide. The window is layered (translucent)
        // only for these ~110 ms; `end_open` turns it back into a normal one.
        let ex = GetWindowLongPtrW(a.hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(a.hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
        SetLayeredWindowAttributes(a.hwnd, 0, 0, LWA_ALPHA);
        a.anim.layered = true;
        a.anim.open = anim::Tween::from_to(0.0, 1.0, anim::OPEN_MS);
        start_y = y + a.s(anim::SLIDE);
    }
    SetWindowPos(a.hwnd, HWND_TOPMOST, x, start_y, w, h, SWP_SHOWWINDOW);
    force_foreground(a.hwnd);
    redraw();
    if a.anim.layered {
        animate();
    }
    restart_blink();
}

/// Shows the search caret and (re)starts its blink at the Windows blink rate,
/// so it stays solid while you type. No timer when blinking is turned off.
unsafe fn restart_blink() {
    let a = app();
    if !a.caret_on {
        a.caret_on = true;
        InvalidateRect(a.hwnd, &a.caret_rect, 0);
    }
    let ms = GetCaretBlinkTime();
    if a.mode.is_list() && ms != 0 && ms != u32::MAX && shown() {
        SetTimer(a.hwnd, TIMER_BLINK, ms, None);
    } else {
        KillTimer(a.hwnd, TIMER_BLINK);
    }
}

unsafe fn stop_blink() {
    let a = app();
    KillTimer(a.hwnd, TIMER_BLINK);
    a.caret_on = true;
}

// ---------------------------------------------------------------- animation

/// Starts the animation timer if it isn't running.
pub fn animate() {
    let a = app();
    if !a.anim.running {
        a.anim.running = true;
        unsafe { SetTimer(a.hwnd, TIMER_ANIM, 10, None) };
    }
}

/// One animation frame (~64 per second). Stops the timer when all is still.
unsafe fn tick() {
    let a = app();
    let mut active = false;

    if a.anim.closing {
        let t = a.anim.close.value();
        SetLayeredWindowAttributes(a.hwnd, 0, (t * 255.0).round() as u8, LWA_ALPHA);
        let dy = ((1.0 - t) * a.s(anim::CLOSE_SLIDE) as f32).round() as i32;
        let mut r: RECT = zeroed();
        GetWindowRect(a.hwnd, &mut r);
        SetWindowPos(a.hwnd, null_mut(), r.left, a.anim.base_y + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        if a.anim.close.done() {
            finish_close();
        } else {
            active = true;
        }
    }

    if a.anim.layered {
        let t = a.anim.open.value();
        SetLayeredWindowAttributes(a.hwnd, 0, (t * 255.0).round() as u8, LWA_ALPHA);
        let dy = ((1.0 - t) * a.s(anim::SLIDE) as f32).round() as i32;
        let mut r: RECT = zeroed();
        GetWindowRect(a.hwnd, &mut r);
        SetWindowPos(a.hwnd, null_mut(), r.left, a.anim.base_y + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        if a.anim.open.done() {
            end_open();
        } else {
            active = true;
        }
    }

    apply_height();
    active |= !a.anim.height.done();

    let painting = a.anim.painting();
    if painting || a.anim.repaint {
        redraw();
    }
    a.anim.repaint = painting; // draws the final frame on the next tick
    active |= painting;

    if !active && !a.anim.repaint {
        KillTimer(a.hwnd, TIMER_ANIM);
        a.anim.running = false;
    }
}

/// Ends the fade-in: fully opaque, final position, normal (non-layered) window.
unsafe fn end_open() {
    let a = app();
    if !a.anim.layered {
        return;
    }
    a.anim.layered = false;
    a.anim.open = anim::Tween::at(1.0);
    let mut r: RECT = zeroed();
    GetWindowRect(a.hwnd, &mut r);
    SetWindowPos(a.hwnd, null_mut(), r.left, a.anim.base_y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    SetLayeredWindowAttributes(a.hwnd, 0, 255, LWA_ALPHA);
    let ex = GetWindowLongPtrW(a.hwnd, GWL_EXSTYLE);
    SetWindowLongPtrW(a.hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
    RedrawWindow(a.hwnd, null(), null_mut(), RDW_ERASE | RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN);
}

/// Applies the animated window height (only when it changed by a pixel).
unsafe fn apply_height() {
    let a = app();
    let h = a.s(a.anim.height.value().round() as i32);
    if h != a.anim.applied_h {
        a.anim.applied_h = h;
        SetWindowPos(a.hwnd, null_mut(), 0, 0, a.s(WIDTH), h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

unsafe fn work_area(p: POINT) -> RECT {
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST), &mut mi);
    mi.rcWork
}

fn height() -> i32 {
    let a = app();
    match a.mode {
        Mode::Edit => EDIT_H,
        Mode::Shortcut => SHORTCUT_H,
        _ => draw::list_height(a.rows),
    }
}

/// Re-fits the window to the current mode, keeping it on screen.
unsafe fn resize() {
    let a = app();
    if a.mode.is_list() {
        a.rows = a.all.len().clamp(MIN_ROWS, MAX_ROWS);
    }
    let mut r: RECT = zeroed();
    GetWindowRect(a.hwnd, &mut r);
    let work = work_area(POINT { x: r.left, y: r.top });
    let h = a.s(height());
    let y = r.top.min(work.bottom - h - a.s(8)).max(work.top);
    if y != r.top {
        SetWindowPos(a.hwnd, null_mut(), r.left, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        a.anim.base_y = y;
    }
    // Ease the height instead of jumping (list <-> editor, add / delete).
    let ms = a.anim.ms(anim::HEIGHT_MS);
    a.anim.height.go(height() as f32, ms);
    apply_height();
    animate();
    redraw();
}

/// Text caret of the focused app if it exposes one, else the mouse pointer.
unsafe fn anchor_point(target: HWND) -> POINT {
    if !target.is_null() {
        let mut gti: GUITHREADINFO = zeroed();
        gti.cbSize = size_of::<GUITHREADINFO>() as u32;
        let tid = GetWindowThreadProcessId(target, null_mut());
        if GetGUIThreadInfo(tid, &mut gti) != 0 && !gti.hwndCaret.is_null() && gti.rcCaret.bottom > gti.rcCaret.top {
            let mut p = POINT { x: gti.rcCaret.left, y: gti.rcCaret.bottom };
            ClientToScreen(gti.hwndCaret, &mut p);
            return p;
        }
    }
    let mut p: POINT = zeroed();
    GetCursorPos(&mut p);
    p
}

unsafe fn force_foreground(h: HWND) {
    SetForegroundWindow(h);
    if GetForegroundWindow() != h {
        let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
        let me = GetCurrentThreadId();
        AttachThreadInput(me, fg_thread, 1);
        SetForegroundWindow(h);
        BringWindowToTop(h);
        AttachThreadInput(me, fg_thread, 0);
    }
    if app().mode != Mode::Edit {
        SetFocus(h);
    }
}

/// Visible and not on its way out.
fn shown() -> bool {
    let a = app();
    unsafe { IsWindowVisible(a.hwnd) != 0 && !a.anim.closing }
}

/// Closes the popup. Focus goes back to your app right away, so pasting is
/// never delayed; the window then fades out and drops a few pixels (~90 ms),
/// click-through, on its own.
unsafe fn hide(give_back_focus: bool) {
    let a = app();
    if !shown() {
        return;
    }
    stop_blink();
    leave_shortcut();
    if a.anim.enabled {
        // Continue from the current opacity if it was still fading in.
        let from = if a.anim.layered { a.anim.open.value() } else { 1.0 };
        a.anim.layered = false;
        a.anim.open = anim::Tween::at(1.0);
        let ex = GetWindowLongPtrW(a.hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(a.hwnd, GWL_EXSTYLE, ex | (WS_EX_LAYERED | WS_EX_TRANSPARENT) as isize);
        SetLayeredWindowAttributes(a.hwnd, 0, (from * 255.0).round() as u8, LWA_ALPHA);
        let mut r: RECT = zeroed();
        GetWindowRect(a.hwnd, &mut r);
        a.anim.base_y = r.top;
        a.anim.close = anim::Tween::from_to(from, 0.0, anim::CLOSE_MS);
        a.anim.closing = true;
        animate();
    } else {
        end_open();
        ShowWindow(a.hwnd, SW_HIDE);
    }
    if give_back_focus && !a.prev.is_null() && IsWindow(a.prev) != 0 {
        SetForegroundWindow(a.prev);
    }
}

/// Ends the fade-out: hidden, back in place, a normal window again.
unsafe fn finish_close() {
    let a = app();
    if !a.anim.closing {
        return;
    }
    a.anim.closing = false;
    ShowWindow(a.hwnd, SW_HIDE);
    let ex = GetWindowLongPtrW(a.hwnd, GWL_EXSTYLE);
    SetWindowLongPtrW(a.hwnd, GWL_EXSTYLE, ex & !((WS_EX_LAYERED | WS_EX_TRANSPARENT) as isize));
}

// ---------------------------------------------------------------- paste

/// Pastes the selected snippet into the previous window (or opens the URL).
unsafe fn choose(open_url: bool) {
    let a = app();
    let Some(&i) = a.hits.get(a.sel) else { return };
    let snip = a.all[i].clone();

    if open_url && snip.kind == Kind::Url {
        hide(false);
        let url = snip.body.trim();
        let url = if url.starts_with("www.") { format!("https://{url}") } else { url.to_string() };
        ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(&url).as_ptr(), null(), null(), SW_SHOWNORMAL);
        return count_use(&snip.name);
    }

    let old = paste::get_text(a.hwnd);
    let expanded = paste::expand(&snip.body, old.as_deref());
    hide(true);
    if a.prev.is_null() || !paste::set_text(a.hwnd, &expanded.text) {
        return; // nowhere to paste: the text simply stays on the clipboard
    }
    // Wait (briefly) until the target app is really in front again.
    for _ in 0..20 {
        if GetForegroundWindow() == a.prev {
            break;
        }
        Sleep(10);
    }
    paste::send_paste(expanded.caret_back);
    count_use(&snip.name); // after pasting, so it never delays the paste

    // Put the user's own clipboard back once the target has read ours.
    if old.is_some() {
        a.restore = old;
        SetTimer(a.hwnd, TIMER_RESTORE, 500, None);
    }
}

/// Remembers that a snippet was used, for the most-used ordering.
fn count_use(name: &str) {
    let a = app();
    a.usage.record(name, usage::now());
    let _ = a.usage.save();
}

// ---------------------------------------------------------------- list input

unsafe fn on_key(vk: u16) {
    let a = app();
    match a.mode {
        Mode::Edit => {
            editor_key(vk);
            return;
        }
        Mode::Shortcut => return shortcut_key(vk, held_mods()),
        _ => {}
    }
    let ctrl = GetKeyState(VK_CONTROL as i32) < 0;

    // Inline delete confirmation takes the keyboard until answered.
    if let Some(si) = a.confirm {
        match vk {
            VK_RETURN | VK_DELETE => {
                a.confirm = None;
                delete(si);
            }
            VK_ESCAPE => {
                a.confirm = None;
                redraw();
            }
            _ => {}
        }
        return;
    }

    let n = a.hits.len();
    let selected = a.hits.get(a.sel).copied();
    match vk {
        VK_ESCAPE => {
            if !a.query.is_empty() {
                a.query.clear();
                refilter();
            } else if a.mode == Mode::Manage {
                set_mode(Mode::Browse);
            } else {
                hide(true);
            }
            return;
        }
        VK_RETURN if a.mode == Mode::Manage => return open_editor(selected),
        VK_RETURN => return choose(GetKeyState(VK_SHIFT as i32) < 0),
        VK_DELETE if a.mode == Mode::Manage => {
            a.confirm = selected;
            return redraw();
        }
        VK_BACK => {
            if ctrl {
                a.query.clear();
            } else {
                a.query.pop();
            }
            return refilter();
        }
        VK_E if ctrl => return open_editor(selected),
        VK_N if ctrl => return open_editor(None),
        VK_P if ctrl => {
            if let Some(si) = selected {
                toggle_pin(si);
            }
            return;
        }
        _ if n == 0 => return,
        VK_DOWN | VK_TAB => a.sel = (a.sel + 1) % n,
        VK_UP => a.sel = (a.sel + n - 1) % n,
        VK_NEXT => a.sel = (a.sel + a.rows).min(n - 1),
        VK_PRIOR => a.sel = a.sel.saturating_sub(a.rows),
        _ => return,
    }
    scroll_into_view();
    redraw();
}

fn scroll_into_view() {
    let a = app();
    if a.sel < a.top {
        a.top = a.sel;
    } else if a.sel >= a.top + a.rows {
        a.top = a.sel + 1 - a.rows;
    }
}

/// Snippets matching the search, in display order (pinned, best match, most used).
fn find() -> Vec<usize> {
    let a = app();
    let now = usage::now();
    snippets::search(&a.all, &a.query, |s| a.usage.score(&s.name, now))
}

fn refilter() {
    let a = app();
    a.hits = find();
    a.sel = 0;
    a.top = 0;
    a.anim.snap_select = true; // new results: jump, don't glide
    redraw();
}

/// Pins or unpins a snippet; the selection follows it to its new place.
unsafe fn toggle_pin(si: usize) {
    let a = app();
    if si >= a.all.len() {
        return;
    }
    a.all[si].pinned = !a.all[si].pinned;
    let _ = snippets::save(&a.all);
    a.all = snippets::load(); // same order, so `si` still points at it
    a.hits = find();
    a.confirm = None;
    if let Some(pos) = a.hits.iter().position(|&i| i == si) {
        a.sel = pos;
    }
    scroll_into_view();
    redraw();
}

unsafe fn set_mode(m: Mode) {
    let a = app();
    a.mode = m;
    a.confirm = None;
    redraw();
}

// ---------------------------------------------------------------- editor

unsafe fn open_editor(index: Option<usize>) {
    let a = app();
    let back = if a.mode == Mode::Edit { a.editor.back } else { a.mode };
    let (title, body, kind, pinned) = match index.and_then(|i| a.all.get(i)) {
        Some(s) => (s.name.clone(), s.body.clone(), s.kind, s.pinned),
        // New snippet: what you searched for becomes the title.
        None => (a.query.trim().to_string(), String::new(), Kind::Template, false),
    };
    a.editor = Editor { index, kind, error: String::new(), confirm_delete: false, pinned, back };
    a.confirm = None;
    a.hover = None;
    a.mode = Mode::Edit;
    SetWindowTextW(a.title_edit, wide(&title).as_ptr());
    SetWindowTextW(a.body_edit, wide(&body.replace('\n', "\r\n")).as_ptr());
    resize();
    layout_editor();
    ShowWindow(a.title_edit, SW_SHOW);
    ShowWindow(a.body_edit, SW_SHOW);
    let first = if title.is_empty() || index.is_none() { a.title_edit } else { a.body_edit };
    SetFocus(first);
    SendMessageW(first, EM_SETSEL, usize::MAX, -1); // caret at the end
}

unsafe fn close_editor() {
    let a = app();
    ShowWindow(a.title_edit, SW_HIDE);
    ShowWindow(a.body_edit, SW_HIDE);
    a.mode = a.editor.back;
    a.last_focus = null_mut();
    a.hover = None;
    a.anim.snap_select = true;
    SetFocus(a.hwnd);
    resize();
    restart_blink();
}

/// Places the text boxes inside the fields drawn by `draw::paint_editor`.
unsafe fn layout_editor() {
    let a = app();
    for h in [a.title_edit, a.body_edit] {
        SendMessageW(h, WM_SETFONT, a.fonts.query as WPARAM, 0);
    }
    let x = a.s(EX + 10);
    let w = a.s(WIDTH - 2 * EX - 20);
    let line = a.s(20);
    let ty = a.s(E_TITLE.0) + (a.s(E_TITLE.1 - E_TITLE.0) - line) / 2;
    MoveWindow(a.title_edit, x, ty, w, line, 1);
    MoveWindow(a.body_edit, x, a.s(E_BODY.0 + 7), a.s(WIDTH - 2 * EX - 14), a.s(E_BODY.1 - E_BODY.0 - 12), 1);
    update_body_scrollbar();
}

/// Shows the content scrollbar only when the text doesn't fit.
unsafe fn update_body_scrollbar() {
    let a = app();
    let lines = SendMessageW(a.body_edit, EM_GETLINECOUNT, 0, 0) as i32;
    let mut rc: RECT = zeroed();
    GetClientRect(a.body_edit, &mut rc);
    ShowScrollBar(a.body_edit, SB_VERT, (lines * a.s(19) > rc.bottom) as _);
}

/// Keyboard shortcuts while editing. Returns true when the key was handled.
unsafe fn editor_key(vk: u16) -> bool {
    let a = app();
    let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
    let focus = GetFocus();
    match vk {
        VK_ESCAPE => {
            if a.editor.confirm_delete {
                a.editor.confirm_delete = false;
                redraw();
            } else {
                close_editor();
            }
        }
        VK_RETURN | VK_S if ctrl => save_editor(),
        VK_RETURN if focus == a.title_edit => {
            SetFocus(a.body_edit);
        }
        VK_TAB => {
            SetFocus(if focus == a.title_edit { a.body_edit } else { a.title_edit });
        }
        VK_A if ctrl => {
            SendMessageW(focus, EM_SETSEL, 0, -1);
        }
        VK_P if ctrl => {
            a.editor.pinned = !a.editor.pinned;
            redraw();
        }
        VK_T if ctrl => {
            let i = Kind::ALL.iter().position(|&k| k == a.editor.kind).unwrap_or(0);
            a.editor.kind = Kind::ALL[(i + 1) % Kind::ALL.len()];
            a.editor.error.clear();
            redraw();
        }
        _ => return false,
    }
    true
}

unsafe fn window_text(h: HWND) -> String {
    let len = GetWindowTextLengthW(h);
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe fn save_editor() {
    let a = app();
    let title = window_text(a.title_edit).trim().to_string();
    let body = window_text(a.body_edit).replace("\r\n", "\n");
    let body = if a.editor.kind == Kind::Url { body.trim().to_string() } else { body.trim_end().to_string() };

    let error = if title.is_empty() {
        Some(("Give the snippet a title.", a.title_edit))
    } else if body.trim().is_empty() {
        Some(("Content can't be empty.", a.body_edit))
    } else if a.editor.kind == Kind::Url && !snippets::valid_url(&body) {
        Some(("Enter a valid URL, e.g. https://example.com", a.body_edit))
    } else {
        None
    };
    if let Some((msg, field)) = error {
        a.editor.error = msg.to_string();
        SetFocus(field);
        return redraw();
    }

    let mut snippet = Snippet::new(&title, &body, a.editor.kind);
    snippet.pinned = a.editor.pinned;
    let index = match a.editor.index {
        Some(i) if i < a.all.len() => {
            // Renamed: its usage count moves with it.
            if a.all[i].name != title {
                a.usage.rename(&a.all[i].name, &title);
                let _ = a.usage.save();
            }
            a.all[i] = snippet;
            i
        }
        _ => {
            a.all.push(snippet);
            a.all.len() - 1
        }
    };
    if let Err(e) = snippets::save(&a.all) {
        a.editor.error = format!("Couldn't save: {e}");
        return redraw();
    }
    a.all = snippets::load();
    a.query.clear();
    refilter();
    if let Some(pos) = a.hits.iter().position(|&i| i == index) {
        a.sel = pos;
    }
    close_editor();
    scroll_into_view();
}

unsafe fn delete(si: usize) {
    let a = app();
    if si >= a.all.len() {
        return;
    }
    let keep = a.sel;
    let gone = a.all.remove(si);
    if !a.all.iter().any(|s| s.name == gone.name) {
        a.usage.remove(&gone.name);
        let _ = a.usage.save();
    }
    let _ = snippets::save(&a.all);
    a.all = snippets::load();
    a.hits = find();
    a.sel = keep.min(a.hits.len().saturating_sub(1));
    a.top = a.top.min(a.hits.len().saturating_sub(1));
    a.anim.snap_select = true;
    resize();
    scroll_into_view();
}

// ---------------------------------------------------------------- shortcut

/// Registers `h` as the global shortcut. On success it becomes the current one.
unsafe fn register(h: Hotkey) -> bool {
    let a = app();
    if RegisterHotKey(a.hwnd, HOTKEY_ID, h.mods | MOD_NOREPEAT, h.vk) == 0 {
        return false;
    }
    a.hotkey_now = Some(h);
    a.hotkey = h.label();
    true
}

/// First free default shortcut.
unsafe fn register_default() -> bool {
    hotkey::DEFAULTS.into_iter().any(|h| register(h))
}

/// Opens the screen. The current shortcut is released so it can be picked
/// again, and a keyboard hook sees keys before any app's shortcut does, so a
/// combination that's taken can be reported instead of silently vanishing
/// (and tapping Win doesn't open Start). The hook exists only on this screen.
unsafe fn begin_shortcut() {
    let a = app();
    UnregisterHotKey(a.hwnd, HOTKEY_ID);
    a.shortcut = ShortcutUi { active: true, ..Default::default() };
    if a.key_hook.is_null() {
        a.key_hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), GetModuleHandleW(null()), 0);
    }
}

unsafe extern "system" fn key_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    if code == HC_ACTION as i32 && a.shortcut.active && GetForegroundWindow() == a.hwnd {
        let vk = (*(lp as *const KBDLLHOOKSTRUCT)).vkCode as u16;
        let down = matches!(wp as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let m = match vk {
            VK_LWIN | VK_RWIN => MOD_WIN,
            VK_CONTROL | VK_LCONTROL | VK_RCONTROL => MOD_CONTROL,
            VK_MENU | VK_LMENU | VK_RMENU => MOD_ALT,
            VK_SHIFT | VK_LSHIFT | VK_RSHIFT => MOD_SHIFT,
            _ => 0,
        };
        if down {
            a.shortcut.held |= m;
        } else {
            a.shortcut.held &= !m;
        }
        let msg = if down { WM_SHORTCUT_KEY } else { WM_SHORTCUT_KEYUP };
        PostMessageW(a.hwnd, msg, vk as WPARAM, a.shortcut.held as LPARAM);
        return 1; // swallowed: no other app or shortcut sees it
    }
    CallNextHookEx(null_mut(), code, wp, lp)
}

/// Closes the screen. Without a new shortcut, the previous one comes back.
unsafe fn leave_shortcut() {
    let a = app();
    if !a.shortcut.active {
        return;
    }
    a.shortcut.active = false;
    if !a.key_hook.is_null() {
        UnhookWindowsHookEx(a.key_hook);
        a.key_hook = null_mut();
    }
    if !a.shortcut.saved {
        if let Some(h) = a.hotkey_now {
            register(h);
        }
    }
}

unsafe fn held_mods() -> HOT_KEY_MODIFIERS {
    let down = |vk: u16| GetKeyState(vk as i32) < 0;
    let mut m = 0;
    if down(VK_LWIN) || down(VK_RWIN) {
        m |= MOD_WIN;
    }
    if down(VK_CONTROL) {
        m |= MOD_CONTROL;
    }
    if down(VK_MENU) {
        m |= MOD_ALT;
    }
    if down(VK_SHIFT) {
        m |= MOD_SHIFT;
    }
    m
}

/// A key pressed on the "Change shortcut" screen.
unsafe fn shortcut_key(vk: u16, mods: HOT_KEY_MODIFIERS) {
    let a = app();
    if mods == 0 && (vk == VK_ESCAPE || vk == VK_RETURN) {
        return hide(true);
    }
    if hotkey::is_modifier(vk) {
        // Only modifiers so far: show them while waiting for the key.
        a.shortcut.preview = hotkey::mod_names(mods);
        a.shortcut.preview.push("\u{2026}".to_string());
        return redraw();
    }
    let h = Hotkey { mods, vk: vk as u32 };
    a.shortcut.preview = h.parts();
    a.shortcut.error = true;
    if !h.allowed() {
        a.shortcut.message = "Add Ctrl, Alt or Win. Only F-keys work on their own.".to_string();
        return redraw();
    }
    // Swap: release the one picked a moment ago (if any), try the new one.
    UnregisterHotKey(a.hwnd, HOTKEY_ID);
    if register(h) {
        let _ = hotkey::save(Some(h));
        a.shortcut.saved = true;
        a.shortcut.error = false;
        a.shortcut.message = format!("Saved. Press {} anywhere to open Snipt.", a.hotkey);
        tray(NIM_MODIFY, None);
    } else {
        a.shortcut.message = format!("{} is already used by another app or by Windows. Try another.", h.label());
        if a.shortcut.saved {
            if let Some(prev) = a.hotkey_now {
                register(prev);
            }
        }
    }
    redraw();
}

/// Modifier released before a key was pressed: update the preview.
unsafe fn shortcut_keyup(mods: HOT_KEY_MODIFIERS) {
    let a = app();
    if a.shortcut.preview.last().is_some_and(|p| p == "\u{2026}") {
        a.shortcut.preview = hotkey::mod_names(mods);
        if mods != 0 {
            a.shortcut.preview.push("\u{2026}".to_string());
        }
        redraw();
    }
}

/// Forgets the chosen shortcut and goes back to the first free default.
unsafe fn reset_shortcut() {
    let a = app();
    UnregisterHotKey(a.hwnd, HOTKEY_ID);
    let _ = hotkey::save(None);
    a.shortcut.saved = true;
    a.shortcut.preview.clear();
    if register_default() {
        a.shortcut.error = false;
        a.shortcut.message = format!("Back to the default: {}.", a.hotkey);
    } else {
        a.hotkey_now = None;
        a.hotkey = "no hotkey (use the tray icon)".to_string();
        a.shortcut.error = true;
        a.shortcut.message = "All default shortcuts are in use. Pick another one.".to_string();
    }
    tray(NIM_MODIFY, None);
    redraw();
}

// ---------------------------------------------------------------- dpi

fn redraw() {
    unsafe { InvalidateRect(app().hwnd, null(), 0) };
}

unsafe fn set_dpi(dpi: u32) {
    let a = app();
    let dpi = dpi.max(96);
    if dpi == a.dpi && !a.fonts.query.is_null() {
        return;
    }
    a.dpi = dpi;
    a.fonts.destroy();
    a.fonts = Fonts::create(a);
}

// ---------------------------------------------------------------- tray, file, autostart

/// The icon embedded in the exe by build.rs (resource 1), at the given size.
unsafe fn app_icon(size: i32) -> HICON {
    let icon = LoadImageW(GetModuleHandleW(null()), 1 as _, IMAGE_ICON, size, size, LR_DEFAULTCOLOR);
    if icon.is_null() { LoadIconW(null_mut(), IDI_APPLICATION) } else { icon as HICON }
}

unsafe fn tray(action: NOTIFY_ICON_MESSAGE, balloon: Option<&str>) {
    let a = app();
    let mut nid: NOTIFYICONDATAW = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = a.hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = a.icon;
    copy_wide(&mut nid.szTip, &format!("Snipt ({})", a.hotkey));
    if let Some(msg) = balloon {
        nid.uFlags |= NIF_INFO;
        copy_wide(&mut nid.szInfo, msg);
        copy_wide(&mut nid.szInfoTitle, "Snipt");
        nid.dwInfoFlags = NIIF_INFO;
    }
    Shell_NotifyIconW(action, &nid);
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let max = dst.len() - 1; // keep the terminating zero
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(max)) {
        *d = c;
    }
}

unsafe fn tray_menu() {
    let a = app();
    let m = CreatePopupMenu();
    AppendMenuW(m, MF_STRING, ID_OPEN, wide(&format!("Open Snipt\t{}", a.hotkey)).as_ptr());
    AppendMenuW(m, MF_STRING, ID_MANAGE, wide("Manage snippets").as_ptr());
    AppendMenuW(m, MF_STRING, ID_FILE, wide("Open snippets file").as_ptr());
    AppendMenuW(m, MF_STRING, ID_SHORTCUT, wide("Change shortcut\u{2026}").as_ptr());
    let check = if autostart_enabled() { MF_CHECKED } else { 0 };
    AppendMenuW(m, MF_STRING | check, ID_AUTOSTART, wide("Start with Windows").as_ptr());
    AppendMenuW(m, MF_SEPARATOR, 0, null());
    AppendMenuW(m, MF_STRING, ID_EXIT, wide("Exit").as_ptr());
    let mut pt: POINT = zeroed();
    GetCursorPos(&mut pt);
    SetForegroundWindow(a.hwnd); // required so the menu closes on outside click
    TrackPopupMenu(m, TPM_RIGHTBUTTON, pt.x, pt.y, 0, a.hwnd, null());
    PostMessageW(a.hwnd, WM_NULL, 0, 0);
    DestroyMenu(m);
}

unsafe fn open_file() {
    let _ = snippets::load(); // creates the file on first use
    let path = snippets::file_path().to_string_lossy().into_owned();
    ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(&path).as_ptr(), null(), null(), SW_SHOWNORMAL);
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

unsafe fn autostart_enabled() -> bool {
    let mut size = 0u32;
    RegGetValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide("Snipt").as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), &mut size) == 0
}

unsafe fn set_autostart(enable: bool) {
    let key = wide(RUN_KEY);
    let name = wide("Snipt");
    if enable {
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(null_mut(), buf.as_mut_ptr(), buf.len() as u32) as usize;
        let value = wide(&format!("\"{}\"", String::from_utf16_lossy(&buf[..len])));
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, value.as_ptr() as _, (value.len() * 2) as u32);
    } else {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
    }
}
