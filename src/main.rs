//! Snipt: a tiny Win+V-style launcher for your own snippets.
//! Pure Win32 + GDI, no runtime, no web view.

#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]

mod draw;
mod paste;
mod snippets;

use draw::{EDIT_H, EX, E_BODY, E_TITLE, Fonts, MAX_ROWS, MIN_ROWS, WIDTH};
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
const HOTKEY_ID: i32 = 1;
const TIMER_RESTORE: usize = 1;

const ID_OPEN: usize = 100;
const ID_MANAGE: usize = 101;
const ID_FILE: usize = 102;
const ID_AUTOSTART: usize = 103;
const ID_EXIT: usize = 104;
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
    ConfirmDelete,
    CancelDelete,
    SetKind(Kind),
    Insert(usize),
    Save,
    Cancel,
    EditorDelete,
}

pub struct Editor {
    /// Index into `App::all`, or None for a new snippet.
    pub index: Option<usize>,
    pub kind: Kind,
    pub error: String,
    pub confirm_delete: bool,
    back: Mode,
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
    hotkey: &'static str,
    taskbar_created: u32,
    icon: HICON,
}

impl App {
    pub fn s(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    pub fn r(&self, l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: self.s(l), top: self.s(t), right: self.s(r), bottom: self.s(b) }
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
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: null_mut(),
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
            editor: Editor { index: None, kind: Kind::Template, error: String::new(), confirm_delete: false, back: Mode::Browse },
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
            hotkey: "no hotkey (use the tray icon)",
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            icon: draw::make_icon(text_face),
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

        // First free shortcut wins.
        let choices: [(HOT_KEY_MODIFIERS, u32, &'static str); 3] = [
            (MOD_WIN | MOD_SHIFT, 'V' as u32, "Win+Shift+V"),
            (MOD_CONTROL | MOD_SHIFT, VK_SPACE as u32, "Ctrl+Shift+Space"),
            (MOD_CONTROL | MOD_ALT, 'V' as u32, "Ctrl+Alt+V"),
        ];
        for (mods, vk, label) in choices {
            if RegisterHotKey(hwnd, HOTKEY_ID, mods | MOD_NOREPEAT, vk) != 0 {
                a.hotkey = label;
                break;
            }
        }

        tray(NIM_ADD, None);
        if first_run {
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
        WM_HOTKEY => {
            if IsWindowVisible(hwnd) == 0 {
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
                if a.mode != Mode::Edit {
                    ShowWindow(hwnd, SW_HIDE);
                }
            } else if a.mode == Mode::Edit {
                SetFocus(if a.last_focus.is_null() { a.title_edit } else { a.last_focus });
            }
        }
        WM_KEYDOWN => on_key(wp as u16),
        WM_CHAR if a.mode != Mode::Edit => {
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
        WM_MOUSEWHEEL if a.mode != Mode::Edit => {
            let delta = ((wp >> 16) & 0xffff) as i16;
            let max_top = a.hits.len().saturating_sub(a.rows);
            a.top = if delta > 0 { a.top.saturating_sub(1) } else { (a.top + 1).min(max_top) };
            a.sel = a.sel.clamp(a.top, (a.top + a.rows - 1).min(a.hits.len().saturating_sub(1)));
            redraw();
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
            SetWindowPos(hwnd, null_mut(), r.left, r.top, a.s(WIDTH), a.s(height()), SWP_NOZORDER | SWP_NOACTIVATE);
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
    if IsWindowVisible(a.hwnd) != 0 && a.mode == Mode::Edit {
        return force_foreground(a.hwnd);
    }
    let fg = GetForegroundWindow();
    a.prev = if fg == a.hwnd { null_mut() } else { fg };
    a.all = snippets::load(); // tiny file; reloading picks up outside edits
    a.query.clear();
    a.confirm = None;
    a.hover = None;
    a.mode = mode;
    refilter();

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

    SetWindowPos(a.hwnd, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
    force_foreground(a.hwnd);
    redraw();
}

unsafe fn work_area(p: POINT) -> RECT {
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST), &mut mi);
    mi.rcWork
}

fn height() -> i32 {
    let a = app();
    if a.mode == Mode::Edit { EDIT_H } else { draw::list_height(a.rows) }
}

/// Re-fits the window to the current mode, keeping it on screen.
unsafe fn resize() {
    let a = app();
    if a.mode != Mode::Edit {
        a.rows = a.all.len().clamp(MIN_ROWS, MAX_ROWS);
    }
    let mut r: RECT = zeroed();
    GetWindowRect(a.hwnd, &mut r);
    let work = work_area(POINT { x: r.left, y: r.top });
    let h = a.s(height());
    let y = r.top.min(work.bottom - h - a.s(8)).max(work.top);
    SetWindowPos(a.hwnd, null_mut(), r.left, y, a.s(WIDTH), h, SWP_NOZORDER | SWP_NOACTIVATE);
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

unsafe fn hide(give_back_focus: bool) {
    let a = app();
    ShowWindow(a.hwnd, SW_HIDE);
    if give_back_focus && !a.prev.is_null() && IsWindow(a.prev) != 0 {
        SetForegroundWindow(a.prev);
    }
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
        return;
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

    // Put the user's own clipboard back once the target has read ours.
    if old.is_some() {
        a.restore = old;
        SetTimer(a.hwnd, TIMER_RESTORE, 500, None);
    }
}

// ---------------------------------------------------------------- list input

unsafe fn on_key(vk: u16) {
    let a = app();
    if a.mode == Mode::Edit {
        editor_key(vk);
        return;
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

fn refilter() {
    let a = app();
    a.hits = snippets::search(&a.all, &a.query);
    a.sel = 0;
    a.top = 0;
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
    let (title, body, kind) = match index.and_then(|i| a.all.get(i)) {
        Some(s) => (s.name.clone(), s.body.clone(), s.kind),
        // New snippet: what you searched for becomes the title.
        None => (a.query.trim().to_string(), String::new(), Kind::Template),
    };
    a.editor = Editor { index, kind, error: String::new(), confirm_delete: false, back };
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
    SetFocus(a.hwnd);
    resize();
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

    let snippet = Snippet::new(&title, &body, a.editor.kind);
    let index = match a.editor.index {
        Some(i) if i < a.all.len() => {
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
    a.all.remove(si);
    let _ = snippets::save(&a.all);
    a.all = snippets::load();
    a.hits = snippets::search(&a.all, &a.query);
    a.sel = keep.min(a.hits.len().saturating_sub(1));
    a.top = a.top.min(a.hits.len().saturating_sub(1));
    resize();
    scroll_into_view();
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
