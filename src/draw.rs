//! Theme, layout and GDI drawing for the three screens: Browse, Manage, Edit.
//!
//! Everything is laid out in 96-DPI units and scaled with `App::r` / `App::s`.
//! While painting, every clickable area is recorded in `App::regions`, so hit
//! testing always matches exactly what is on screen.

use crate::anim::{CONFIRM_MS, SELECT_MS, Tween};
use crate::snippets::{Kind, Snippet};
use crate::{Action, App, Mode, PLACEHOLDERS};
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{COLORREF, LPARAM, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

// ---------------------------------------------------------------- theme

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
pub const BG: COLORREF = rgb(16, 16, 18); // near-black window
pub const SURFACE: COLORREF = rgb(27, 27, 31); // search field, inputs, key caps
pub const HOVER: COLORREF = rgb(36, 36, 41);
pub const BORDER: COLORREF = rgb(42, 42, 48);
pub const SELECTED: COLORREF = rgb(31, 26, 44); // purple-tinted dark
pub const ACCENT: COLORREF = rgb(139, 92, 246);
pub const ACCENT_HOVER: COLORREF = rgb(154, 114, 250);
pub const ACCENT_TEXT: COLORREF = rgb(184, 164, 252);
pub const TEXT: COLORREF = rgb(232, 232, 237);
pub const MUTED: COLORREF = rgb(145, 145, 156);
pub const FAINT: COLORREF = rgb(98, 98, 108);
pub const DANGER: COLORREF = rgb(240, 100, 110);
pub const DANGER_BG: COLORREF = rgb(44, 21, 25);

/// Blends `a` over `b`; `t` = 0 gives `b`, 255 gives `a`.
pub fn mix(a: COLORREF, b: COLORREF, t: u32) -> COLORREF {
    let t = t.min(255);
    let ch = |c: COLORREF, s: u32| (c >> s) & 0xff;
    let m = |s| (ch(a, s) * t + ch(b, s) * (255 - t)) / 255;
    m(0) | m(8) << 8 | m(16) << 16
}

/// Hover colour for `act`, fading from `base` to `to` as the hover animates in.
fn hv(a: &App, act: Action, base: COLORREF, to: COLORREF) -> COLORREF {
    if a.hover == Some(act) {
        mix(to, base, (a.anim.hover.value() * 255.0).round() as u32)
    } else {
        base
    }
}

// Segoe Fluent Icons / Segoe MDL2 Assets code points (built into Windows).
const IC_SEARCH: u32 = 0xE721;
const IC_SETTINGS: u32 = 0xE713;
const IC_LINK: u32 = 0xE71B;
const IC_DOC: u32 = 0xE8A5;
const IC_TEXT: u32 = 0xE8C1;
const IC_PASTE: u32 = 0xE77F;
const IC_CALENDAR: u32 = 0xE787;
const IC_EDIT: u32 = 0xE70F;
const IC_DELETE: u32 = 0xE74D;
const IC_ADD: u32 = 0xE710;

// ---------------------------------------------------------------- layout (96-DPI units)

pub const WIDTH: i32 = 440;
pub const PAD: i32 = 6;
pub const SEARCH_H: i32 = 34;
pub const GAP: i32 = 4;
pub const ROW_H: i32 = 42;
pub const MAX_ROWS: usize = 7;
pub const MIN_ROWS: usize = 2;
pub const FOOTER_H: i32 = 30;
pub const LIST_TOP: i32 = PAD + SEARCH_H + GAP;

pub fn list_height(rows: usize) -> i32 {
    LIST_TOP + rows as i32 * ROW_H + GAP + FOOTER_H
}

// Editor: x margin and vertical bands (top, bottom).
pub const EX: i32 = 12;
const E_HEAD: (i32, i32) = (10, 32);
const E_TITLE_L: i32 = 40;
pub const E_TITLE: (i32, i32) = (57, 87);
const E_TYPE_L: i32 = 97;
const E_TYPE: (i32, i32) = (114, 142);
const E_BODY_L: i32 = 152;
pub const E_BODY: (i32, i32) = (169, 297);
const E_HINT: (i32, i32) = (301, 321);
const E_FOOT: i32 = 329;
pub const EDIT_H: i32 = 373;

// ---------------------------------------------------------------- fonts

pub struct Fonts {
    pub query: HFONT,
    pub name: HFONT,
    pub small: HFONT,
    pub tiny: HFONT,
    pub badge: HFONT,
    pub icon: HFONT,
    pub icon_small: HFONT,
}

impl Fonts {
    pub const fn empty() -> Fonts {
        Fonts {
            query: null_mut(),
            name: null_mut(),
            small: null_mut(),
            tiny: null_mut(),
            badge: null_mut(),
            icon: null_mut(),
            icon_small: null_mut(),
        }
    }

    fn all(&self) -> [HFONT; 7] {
        [self.query, self.name, self.small, self.tiny, self.badge, self.icon, self.icon_small]
    }

    pub unsafe fn create(a: &App) -> Fonts {
        let (t, i) = (a.text_face, a.icon_face);
        Fonts {
            query: font(a.s(14), 400, t),
            name: font(a.s(13), 600, t),
            small: font(a.s(12), 400, t),
            tiny: font(a.s(11), 400, t),
            badge: font(a.s(10), 600, t),
            icon: font(a.s(15), 400, i),
            icon_small: font(a.s(12), 400, i),
        }
    }

    pub unsafe fn destroy(&self) {
        for f in self.all() {
            if !f.is_null() {
                DeleteObject(f as HGDIOBJ);
            }
        }
    }
}

pub unsafe fn font(px: i32, weight: i32, face: &str) -> HFONT {
    CreateFontW(
        -px, 0, 0, 0, weight, 0, 0, 0,
        DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0,
        crate::wide(face).as_ptr(),
    )
}

unsafe extern "system" fn found(_: *const LOGFONTW, _: *const TEXTMETRICW, _: u32, lp: LPARAM) -> i32 {
    *(lp as *mut bool) = true;
    0
}

/// Returns the first installed font family from `faces`.
pub unsafe fn pick_font(faces: &[&'static str]) -> &'static str {
    let dc = GetDC(null_mut());
    let mut pick = faces[faces.len() - 1];
    for &face in faces {
        let mut lf: LOGFONTW = zeroed();
        lf.lfCharSet = DEFAULT_CHARSET;
        for (d, c) in lf.lfFaceName.iter_mut().zip(face.encode_utf16().take(31)) {
            *d = c;
        }
        let mut exists = false;
        EnumFontFamiliesExW(dc, &lf, Some(found), &mut exists as *mut bool as LPARAM, 0);
        if exists {
            pick = face;
            break;
        }
    }
    ReleaseDC(null_mut(), dc);
    pick
}

// ---------------------------------------------------------------- primitives

pub unsafe fn fill(dc: HDC, r: RECT, color: COLORREF) {
    let b = CreateSolidBrush(color);
    FillRect(dc, &r, b);
    DeleteObject(b as HGDIOBJ);
}

/// Rounded rectangle with a 1px outline (`stroke`) and a `fill`.
pub unsafe fn rrect(dc: HDC, r: RECT, radius: i32, fill: COLORREF, stroke: COLORREF) {
    let b = CreateSolidBrush(fill);
    let p = CreatePen(PS_SOLID, 1, stroke);
    let ob = SelectObject(dc, b as HGDIOBJ);
    let op = SelectObject(dc, p as HGDIOBJ);
    RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(b as HGDIOBJ);
    DeleteObject(p as HGDIOBJ);
}

pub unsafe fn text(dc: HDC, s: &str, mut r: RECT, f: HFONT, color: COLORREF, extra: u32) {
    if s.is_empty() {
        return; // an empty Vec's pointer is dangling; never hand it to GDI
    }
    let w: Vec<u16> = s.encode_utf16().collect();
    SelectObject(dc, f as HGDIOBJ);
    SetTextColor(dc, color);
    DrawTextW(dc, w.as_ptr(), w.len() as i32, &mut r, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS | extra);
}

pub unsafe fn text_w(dc: HDC, s: &str, f: HFONT) -> i32 {
    if s.is_empty() {
        return 0;
    }
    let w: Vec<u16> = s.encode_utf16().collect();
    SelectObject(dc, f as HGDIOBJ);
    let mut size: SIZE = zeroed();
    GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
    size.cx
}

unsafe fn glyph(dc: HDC, cp: u32, r: RECT, f: HFONT, color: COLORREF) {
    let s = char::from_u32(cp).unwrap_or(' ').to_string();
    text(dc, &s, r, f, color, DT_CENTER);
}

fn icon_for(s: &Snippet) -> u32 {
    match s.kind {
        Kind::Url => IC_LINK,
        Kind::Text => IC_TEXT,
        Kind::Template if s.body.contains("{clipboard}") => IC_PASTE,
        Kind::Template if s.body.contains("{date") || s.body.contains("{time}") => IC_CALENDAR,
        Kind::Template => IC_DOC,
    }
}

fn kind_icon(k: Kind) -> u32 {
    match k {
        Kind::Url => IC_LINK,
        Kind::Template => IC_DOC,
        Kind::Text => IC_TEXT,
    }
}

// ---------------------------------------------------------------- entry

pub unsafe fn paint(a: &mut App) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(a.hwnd, &mut ps);
    let mut rc: RECT = zeroed();
    GetClientRect(a.hwnd, &mut rc);

    // Double-buffered to avoid flicker while typing.
    let dc = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
    let old = SelectObject(dc, bmp as HGDIOBJ);
    SetBkMode(dc, TRANSPARENT as i32);
    fill(dc, rc, BG);

    a.regions.clear();
    match a.mode {
        Mode::Browse | Mode::Manage => paint_list(a, dc),
        Mode::Edit => paint_editor(a, dc),
    }

    BitBlt(hdc, 0, 0, rc.right, rc.bottom, dc, 0, 0, SRCCOPY);
    SelectObject(dc, old);
    DeleteObject(bmp as HGDIOBJ);
    DeleteDC(dc);
    EndPaint(a.hwnd, &ps);
}

// ---------------------------------------------------------------- browse / manage

unsafe fn paint_list(a: &mut App, dc: HDC) {
    let f = &a.fonts;
    let manage = a.mode == Mode::Manage;

    // Search field
    let field = a.r(PAD, PAD, WIDTH - PAD, PAD + SEARCH_H);
    rrect(dc, field, a.s(8), SURFACE, BORDER);
    glyph(dc, IC_SEARCH, a.r(PAD + 8, PAD, PAD + 30, PAD + SEARCH_H), f.icon_small, FAINT);
    let mut right = field.right - a.s(12);
    if manage {
        let w = text_w(dc, "Managing", f.tiny);
        text(dc, "Managing", RECT { left: right - w, right, ..field }, f.tiny, ACCENT_TEXT, 0);
        right -= w + a.s(10);
    }
    let tx = a.s(PAD + 34);
    let tr = RECT { left: tx, right, ..field };
    if a.query.is_empty() {
        let hint = if manage { "Search snippets to edit…" } else { "Search snippets…" };
        text(dc, hint, RECT { left: tx + a.s(5), ..tr }, f.query, FAINT, 0);
    }
    let qw = if a.query.is_empty() { 0 } else { text_w(dc, &a.query, f.query) + a.s(1) };
    text(dc, &a.query, tr, f.query, TEXT, 0);
    let cx = (tx + qw).min(right);
    let ch = a.s(16);
    let cy = field.top + (field.bottom - field.top - ch) / 2;
    a.caret_rect = RECT { left: cx, top: cy, right: cx + a.s(1).max(1) + 1, bottom: cy + ch };
    if a.caret_on {
        fill(dc, a.caret_rect, ACCENT);
    }

    // Rows
    if a.hits.is_empty() {
        let msg = match (a.all.is_empty(), manage) {
            (true, true) => "No snippets yet. Click New or press Ctrl+N.",
            (true, false) => "No snippets yet. Press Ctrl+N to add one.",
            (false, _) => "No snippets match your search.",
        };
        let r = a.r(PAD, LIST_TOP, WIDTH - PAD, LIST_TOP + a.rows as i32 * ROW_H);
        text(dc, msg, r, f.small, MUTED, DT_CENTER);
    }

    // Selection highlight, drawn once and gliding between rows.
    if a.sel < a.hits.len() && a.sel >= a.top && a.sel < a.top + a.rows {
        let target = ((a.sel - a.top) as i32 * ROW_H) as f32;
        if a.anim.snap_select {
            a.anim.select_y = Tween::at(target);
            a.anim.snap_select = false;
        } else if a.anim.select_y.target() != target {
            let ms = a.anim.ms(SELECT_MS);
            a.anim.select_y.go(target, ms);
            crate::animate();
        }
        let y = LIST_TOP + a.anim.select_y.value().round() as i32;
        rrect(dc, a.r(PAD, y, WIDTH - PAD, y + ROW_H), a.s(8), SELECTED, SELECTED);
        fill(dc, a.r(PAD, y + 11, PAD + 2, y + ROW_H - 11), ACCENT);
    }

    // Delete confirmation fades in when it appears.
    if a.confirm != a.anim.confirm_for {
        a.anim.confirm_for = a.confirm;
        if a.confirm.is_some() {
            a.anim.confirm = Tween::from_to(0.0, 1.0, a.anim.ms(CONFIRM_MS));
            crate::animate();
        }
    }

    for row in 0..a.rows {
        let idx = a.top + row;
        let Some(&si) = a.hits.get(idx) else { break };
        let y0 = LIST_TOP + row as i32 * ROW_H;
        if a.confirm == Some(si) {
            paint_confirm_row(a, dc, y0, si);
            continue;
        }
        paint_row(a, dc, y0, idx, si, manage);
    }

    // Footer
    let fy = LIST_TOP + a.rows as i32 * ROW_H + GAP;
    fill(dc, a.r(0, fy, WIDTH, fy + 1), BORDER);
    let mid = |h: i32| (fy + (FOOTER_H - h) / 2, fy + (FOOTER_H + h) / 2);

    // Gear (bottom-right) opens / closes management.
    let (gt, gb) = mid(22);
    let gear = a.r(WIDTH - PAD - 26, gt, WIDTH - PAD - 4, gb);
    if manage {
        rrect(dc, gear, a.s(6), SELECTED, SELECTED);
    } else if a.hover == Some(Action::Gear) {
        let bg = hv(a, Action::Gear, BG, HOVER);
        rrect(dc, gear, a.s(6), bg, bg);
    }
    let gc = if manage { ACCENT_TEXT } else { hv(a, Action::Gear, MUTED, TEXT) };
    glyph(dc, IC_SETTINGS, gear, a.fonts.icon_small, gc);
    a.regions.push((gear, Action::Gear));

    // Counter, divider
    let f = &a.fonts;
    let count = format!("{} / {}", a.hits.len(), a.all.len());
    let cw = text_w(dc, &count, f.tiny);
    let (ft, fb) = mid(FOOTER_H);
    let cr = RECT { left: gear.left - a.s(10) - cw, top: a.s(ft), right: gear.left - a.s(10), bottom: a.s(fb) };
    text(dc, &count, cr, f.tiny, FAINT, DT_RIGHT);
    let (dt, db) = mid(14);
    let dx = cr.left - a.s(12);
    fill(dc, RECT { left: dx, top: a.s(dt), right: dx + 1, bottom: a.s(db) }, BORDER);

    // Shortcuts, dropped from the right if they don't fit.
    let mut x = a.s(PAD + 6);
    let limit = dx - a.s(10);
    if manage {
        let label = "New";
        let w = a.s(8) + a.s(14) + a.s(4) + text_w(dc, label, f.tiny) + a.s(8);
        let (bt, bb) = mid(22);
        let r = RECT { left: x - a.s(4), top: a.s(bt), right: x - a.s(4) + w, bottom: a.s(bb) };
        rrect(dc, r, a.s(6), hv(a, Action::New, BG, HOVER), hv(a, Action::New, BORDER, HOVER));
        glyph(dc, IC_ADD, RECT { left: r.left + a.s(8), right: r.left + a.s(22), ..r }, a.fonts.icon_small, ACCENT_TEXT);
        text(dc, label, RECT { left: r.left + a.s(26), ..r }, a.fonts.tiny, ACCENT_TEXT, 0);
        a.regions.push((r, Action::New));
        x = r.right + a.s(12);
    }
    let selected_url = a.hits.get(a.sel).is_some_and(|&i| a.all[i].kind == Kind::Url);
    let items: &[(&str, &str)] = match (manage, selected_url) {
        (true, _) => &[("Enter", "Edit"), ("Del", "Delete"), ("Esc", "Done")],
        (false, true) => &[("↑↓", "Navigate"), ("Enter", "Paste"), ("Shift+Enter", "Open"), ("Ctrl+E", "Edit")],
        (false, false) => &[("↑↓", "Navigate"), ("Enter", "Paste"), ("Ctrl+E", "Edit")],
    };
    for (key, label) in items {
        match keycap(a, dc, x, limit, fy, key, label) {
            Some(nx) => x = nx,
            None => break,
        }
    }
}

/// Draws "[key] label" at x if it fits before `limit`; returns the next x.
unsafe fn keycap(a: &App, dc: HDC, x: i32, limit: i32, fy: i32, key: &str, label: &str) -> Option<i32> {
    let f = &a.fonts;
    let kw = text_w(dc, key, f.tiny) + a.s(10);
    let lw = text_w(dc, label, f.tiny);
    let end = x + kw + a.s(5) + lw;
    if end > limit {
        return None;
    }
    let top = a.s(fy + (FOOTER_H - 18) / 2);
    let cap = RECT { left: x, top, right: x + kw, bottom: top + a.s(18) };
    rrect(dc, cap, a.s(4), SURFACE, BORDER);
    text(dc, key, cap, f.tiny, TEXT, DT_CENTER);
    let lr = RECT { left: cap.right + a.s(5), top: a.s(fy), right: end + 1, bottom: a.s(fy + FOOTER_H) };
    text(dc, label, lr, f.tiny, MUTED, 0);
    Some(end + a.s(12))
}

unsafe fn paint_row(a: &mut App, dc: HDC, y0: i32, idx: usize, si: usize, manage: bool) {
    let s = &a.all[si];
    let f = &a.fonts;
    let r = a.r(PAD, y0, WIDTH - PAD, y0 + ROW_H);
    let selected = idx == a.sel; // highlight itself is drawn in paint_list
    a.regions.push((r, Action::Row(idx)));

    glyph(dc, icon_for(s), a.r(PAD + 9, y0, PAD + 31, y0 + ROW_H), f.icon, if selected { ACCENT_TEXT } else { MUTED });

    // Type badge, vertically centred on the right.
    let label = s.kind.label();
    let bw = text_w(dc, label, f.badge) + a.s(12);
    let bt = a.s(y0 + (ROW_H - 16) / 2);
    let badge = RECT { left: r.right - a.s(10) - bw, top: bt, right: r.right - a.s(10), bottom: bt + a.s(16) };
    let (bg, fg) = match s.kind {
        Kind::Url => (rgb(62, 40, 120), rgb(214, 200, 255)),
        Kind::Template => (rgb(42, 32, 70), ACCENT_TEXT),
        Kind::Text => (rgb(34, 34, 39), MUTED),
    };
    rrect(dc, badge, a.s(4), bg, bg);
    text(dc, label, badge, f.badge, fg, DT_CENTER);
    let mut text_right = badge.left - a.s(10);

    // Manage mode: edit / delete buttons on the selected row.
    if manage && selected {
        let mut x = badge.left - a.s(6);
        for (icon, act) in [(IC_DELETE, Action::DeleteRow(si)), (IC_EDIT, Action::EditRow(si))] {
            let b = RECT { left: x - a.s(24), top: a.s(y0 + 9), right: x, bottom: a.s(y0 + ROW_H - 9) };
            let danger = icon == IC_DELETE;
            if a.hover == Some(act) {
                let bg = hv(a, act, SELECTED, if danger { DANGER_BG } else { HOVER });
                rrect(dc, b, a.s(6), bg, bg);
            }
            let c = hv(a, act, MUTED, if danger { DANGER } else { TEXT });
            glyph(dc, icon, b, f.icon_small, c);
            a.regions.push((b, act));
            x = b.left - a.s(2);
        }
        text_right = x - a.s(8);
    }

    let lx = a.s(PAD + 38);
    text(dc, &s.name, RECT { left: lx, top: a.s(y0 + 5), right: text_right, bottom: a.s(y0 + 22) }, f.name, TEXT, 0);
    text(dc, &s.preview, RECT { left: lx, top: a.s(y0 + 22), right: text_right, bottom: a.s(y0 + 38) }, f.small, MUTED, 0);
}

unsafe fn paint_confirm_row(a: &mut App, dc: HDC, y0: i32, si: usize) {
    let f = &a.fonts;
    // Fades in from the selected-row colours.
    let t = (a.anim.confirm.value() * 255.0).round() as u32;
    let k = |c: COLORREF| mix(c, SELECTED, t);
    let r = a.r(PAD, y0, WIDTH - PAD, y0 + ROW_H);
    rrect(dc, r, a.s(8), k(DANGER_BG), k(rgb(74, 32, 38)));
    glyph(dc, IC_DELETE, a.r(PAD + 9, y0, PAD + 31, y0 + ROW_H), f.icon_small, k(DANGER));

    let bt = a.s(y0 + (ROW_H - 24) / 2);
    let del = RECT { left: r.right - a.s(10 + 62), top: bt, right: r.right - a.s(10), bottom: bt + a.s(24) };
    let cancel = RECT { left: del.left - a.s(6 + 62), right: del.left - a.s(6), ..del };
    let red = rgb(196, 58, 70);
    rrect(dc, del, a.s(6), k(hv(a, Action::ConfirmDelete, red, rgb(220, 70, 82))), k(red));
    text(dc, "Delete", del, f.tiny, k(rgb(255, 255, 255)), DT_CENTER);
    rrect(dc, cancel, a.s(6), k(hv(a, Action::CancelDelete, SURFACE, HOVER)), k(BORDER));
    text(dc, "Cancel", cancel, f.tiny, k(TEXT), DT_CENTER);
    a.regions.push((r, Action::None));
    a.regions.push((del, Action::ConfirmDelete));
    a.regions.push((cancel, Action::CancelDelete));

    let msg = format!("Delete \u{201C}{}\u{201D}?", a.all[si].name);
    let tr = RECT { left: a.s(PAD + 38), top: r.top, right: cancel.left - a.s(10), bottom: r.bottom };
    text(dc, &msg, tr, f.name, k(TEXT), 0);
}

// ---------------------------------------------------------------- editor

unsafe fn paint_editor(a: &mut App, dc: HDC) {
    let f = &a.fonts;
    let (x0, x1) = (EX, WIDTH - EX);
    let focus = GetFocus();

    let head = if a.editor.index.is_some() { "Edit snippet" } else { "New snippet" };
    text(dc, head, a.r(x0, E_HEAD.0, x1, E_HEAD.1), f.name, TEXT, 0);
    text(dc, "Esc to cancel", a.r(x0, E_HEAD.0, x1, E_HEAD.1), f.tiny, FAINT, DT_RIGHT);

    let label = |dc, s: &str, y: i32| text(dc, s, a.r(x0, y, x1, y + 15), a.fonts.tiny, MUTED, 0);
    label(dc, "Title", E_TITLE_L);
    label(dc, "Type", E_TYPE_L);
    label(dc, "Content", E_BODY_L);

    let stroke = |h| if focus == h { ACCENT } else { BORDER };
    rrect(dc, a.r(x0, E_TITLE.0, x1, E_TITLE.1), a.s(8), SURFACE, stroke(a.title_edit));
    rrect(dc, a.r(x0, E_BODY.0, x1, E_BODY.1), a.s(8), SURFACE, stroke(a.body_edit));

    // Type: segmented control
    let seg_w = 104;
    let group = a.r(x0, E_TYPE.0, x0 + seg_w * 3 + 4, E_TYPE.1);
    rrect(dc, group, a.s(8), SURFACE, BORDER);
    for (i, k) in Kind::ALL.into_iter().enumerate() {
        let l = x0 + 2 + seg_w * i as i32;
        let seg = a.r(l, E_TYPE.0 + 2, l + seg_w, E_TYPE.1 - 2);
        let on = a.editor.kind == k;
        let act = Action::SetKind(k);
        if on {
            rrect(dc, seg, a.s(6), SELECTED, rgb(74, 52, 140));
        } else if a.hover == Some(act) {
            let bg = hv(a, act, SURFACE, HOVER);
            rrect(dc, seg, a.s(6), bg, bg);
        }
        let c = if on { ACCENT_TEXT } else { hv(a, act, MUTED, TEXT) };
        let name = match k {
            Kind::Url => "URL",
            Kind::Template => "Template",
            Kind::Text => "Text",
        };
        let tw = a.s(14) + a.s(6) + text_w(dc, name, f.small);
        let sx = seg.left + (seg.right - seg.left - tw) / 2;
        glyph(dc, kind_icon(k), RECT { left: sx, right: sx + a.s(14), ..seg }, f.icon_small, c);
        text(dc, name, RECT { left: sx + a.s(20), right: seg.right, ..seg }, f.small, c, 0);
        a.regions.push((seg, Action::SetKind(k)));
    }

    // Hint / error line
    let hint = a.r(x0, E_HINT.0, x1, E_HINT.1);
    if !a.editor.error.is_empty() {
        text(dc, &a.editor.error, hint, f.tiny, DANGER, 0);
    } else {
        match a.editor.kind {
            Kind::Template => {
                text(dc, "Insert", hint, f.tiny, FAINT, 0);
                let mut x = hint.left + text_w(dc, "Insert", f.tiny) + a.s(8);
                for (i, ph) in PLACEHOLDERS.iter().enumerate() {
                    let w = text_w(dc, ph, f.tiny) + a.s(10);
                    let chip = RECT { left: x, top: hint.top + a.s(1), right: x + w, bottom: hint.bottom - a.s(1) };
                    rrect(dc, chip, a.s(4), hv(a, Action::Insert(i), SURFACE, HOVER), BORDER);
                    text(dc, ph, chip, f.tiny, ACCENT_TEXT, DT_CENTER);
                    a.regions.push((chip, Action::Insert(i)));
                    x = chip.right + a.s(5);
                }
            }
            Kind::Url => text(dc, "A single link, e.g. https://example.com. Shift+Enter opens it.", hint, f.tiny, FAINT, 0),
            Kind::Text => text(dc, "Pasted exactly as written. No placeholders.", hint, f.tiny, FAINT, 0),
        }
    }

    // Footer: Delete (left), Cancel + Save (right)
    fill(dc, a.r(0, E_FOOT, WIDTH, E_FOOT + 1), BORDER);
    let (bt, bb) = (E_FOOT + 9, E_FOOT + 35);
    let save = a.r(x1 - 76, bt, x1, bb);
    let cancel = a.r(x1 - 76 - 8 - 76, bt, x1 - 76 - 8, bb);
    let save_bg = hv(a, Action::Save, ACCENT, ACCENT_HOVER);
    rrect(dc, save, a.s(6), save_bg, save_bg);
    text(dc, "Save", save, f.small, rgb(255, 255, 255), DT_CENTER);
    rrect(dc, cancel, a.s(6), hv(a, Action::Cancel, SURFACE, HOVER), BORDER);
    text(dc, "Cancel", cancel, f.small, TEXT, DT_CENTER);
    a.regions.push((save, Action::Save));
    a.regions.push((cancel, Action::Cancel));

    let kx = cancel.left - a.s(12);
    let kl = "Ctrl+Enter";
    let kw = text_w(dc, kl, f.tiny) + a.s(10);

    if a.editor.index.is_some() {
        let confirm = a.editor.confirm_delete;
        let label = if confirm { "Click again to delete" } else { "Delete" };
        let w = a.s(28) + text_w(dc, label, f.small) + a.s(10);
        let del = RECT { left: a.s(x0), top: save.top, right: a.s(x0) + w, bottom: save.bottom };
        if confirm {
            rrect(dc, del, a.s(6), DANGER_BG, rgb(110, 40, 48));
        } else if a.hover == Some(Action::EditorDelete) {
            let bg = hv(a, Action::EditorDelete, BG, DANGER_BG);
            rrect(dc, del, a.s(6), bg, bg);
        }
        let c = if confirm { DANGER } else { hv(a, Action::EditorDelete, MUTED, DANGER) };
        glyph(dc, IC_DELETE, RECT { left: del.left + a.s(8), right: del.left + a.s(24), ..del }, f.icon_small, c);
        text(dc, label, RECT { left: del.left + a.s(28), ..del }, f.small, c, 0);
        a.regions.push((del, Action::EditorDelete));
        // Shortcut hint only if there is room next to Delete.
        if del.right + a.s(16) < kx - kw - a.s(40) {
            editor_keycap(a, dc, kx, kw, save, kl);
        }
    } else {
        editor_keycap(a, dc, kx, kw, save, kl);
    }
}

unsafe fn editor_keycap(a: &App, dc: HDC, right: i32, kw: i32, row: RECT, key: &str) {
    let f = &a.fonts;
    let lw = text_w(dc, "Save", f.tiny);
    let label = RECT { left: right - lw, right, ..row };
    text(dc, "Save", label, f.tiny, MUTED, 0);
    let top = row.top + (row.bottom - row.top - a.s(18)) / 2;
    let cap = RECT { left: label.left - a.s(5) - kw, top, right: label.left - a.s(5), bottom: top + a.s(18) };
    rrect(dc, cap, a.s(4), SURFACE, BORDER);
    text(dc, key, cap, f.tiny, TEXT, DT_CENTER);
}

/// Draws the tray icon at runtime (no .ico file needed): a purple tile with "S".
pub unsafe fn make_icon(face: &str) -> HICON {
    const N: i32 = 32;
    let screen = GetDC(null_mut());
    let dc = CreateCompatibleDC(screen);
    let color = CreateCompatibleBitmap(screen, N, N);
    let mask = CreateBitmap(N, N, 1, 1, null());
    let full = RECT { left: 0, top: 0, right: N, bottom: N };

    let old = SelectObject(dc, color as HGDIOBJ);
    fill(dc, full, 0);
    rrect(dc, full, 12, ACCENT, ACCENT);
    SetBkMode(dc, TRANSPARENT as i32);
    let f = font(24, 700, face);
    text(dc, "S", RECT { top: -1, ..full }, f, rgb(255, 255, 255), DT_CENTER);
    DeleteObject(f as HGDIOBJ);

    SelectObject(dc, mask as HGDIOBJ);
    fill(dc, full, rgb(255, 255, 255)); // 1 = transparent
    rrect(dc, full, 12, 0, 0); // 0 = opaque
    SelectObject(dc, old);

    let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&info);
    DeleteObject(color as HGDIOBJ);
    DeleteObject(mask as HGDIOBJ);
    DeleteDC(dc);
    ReleaseDC(null_mut(), screen);
    icon
}
