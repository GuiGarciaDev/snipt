//! Clipboard access, template expansion and the simulated Ctrl+V.

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::Sleep;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_LEFT, VK_V,
};

use crate::wide;

/// Text ready to paste plus how many characters the caret should move back
/// afterwards (for `{cursor}`).
pub struct Expanded {
    pub text: String,
    pub caret_back: usize,
}

pub fn expand(body: &str, clipboard: Option<&str>) -> Expanded {
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    let date = format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay);
    let time = format!("{:02}:{:02}", t.wHour, t.wMinute);
    let mut text = body
        .replace("{datetime}", &format!("{date} {time}"))
        .replace("{date}", &date)
        .replace("{time}", &time)
        .replace("{clipboard}", clipboard.unwrap_or(""));

    let mut caret_back = 0;
    if let Some(pos) = text.find("{cursor}") {
        let after = &text[pos + "{cursor}".len()..];
        // Editors move over "\r\n" with a single Left press, so count chars of
        // the '\n' form. Later {cursor} markers are just removed.
        caret_back = after.replace("{cursor}", "").chars().count();
        text = text.replace("{cursor}", "");
    }
    Expanded { text, caret_back }
}

fn open_clipboard(owner: HWND) -> bool {
    // Another app may hold the clipboard for a moment; retry briefly.
    for _ in 0..10 {
        if unsafe { OpenClipboard(owner) } != 0 {
            return true;
        }
        unsafe { Sleep(15) };
    }
    false
}

pub fn get_text(owner: HWND) -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT as u32) == 0 || !open_clipboard(owner) {
            return None;
        }
        let mut result = None;
        let h = GetClipboardData(CF_UNICODETEXT as u32);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                result = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        result
    }
}

/// Puts text on the clipboard, flagged so Windows clipboard history (Win+V)
/// and clipboard monitors skip it. Returns false if the clipboard was busy.
pub fn set_text(owner: HWND, text: &str) -> bool {
    let data = wide(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
    unsafe {
        if !open_clipboard(owner) {
            return false;
        }
        EmptyClipboard();
        let bytes = data.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        let mut ok = false;
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                GlobalUnlock(h);
                ok = !SetClipboardData(CF_UNICODETEXT as u32, h as HANDLE).is_null();
            }
            if !ok {
                GlobalFree(h);
            }
        }
        if ok {
            let fmt = RegisterClipboardFormatW(wide("ExcludeClipboardContentFromMonitorProcessing").as_ptr());
            let flag = GlobalAlloc(GMEM_MOVEABLE, 4);
            if fmt != 0 && !flag.is_null() && SetClipboardData(fmt, flag as HANDLE).is_null() {
                GlobalFree(flag);
            }
        }
        CloseClipboard();
        ok
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
    if vk == VK_LEFT {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn send(inputs: &[INPUT]) {
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
}

/// Sends Ctrl+V to whatever window has focus, then moves the caret back.
pub fn send_paste(caret_back: usize) {
    send(&[key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)]);
    if caret_back > 0 {
        // Give the target app time to process the paste first.
        unsafe { Sleep(60) };
        let moves: Vec<INPUT> = (0..caret_back.min(5000))
            .flat_map(|_| [key(VK_LEFT, false), key(VK_LEFT, true)])
            .collect();
        send(&moves);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_cursor_and_clipboard() {
        let e = expand("Hi {cursor}!\nBye {clipboard}", Some("X"));
        assert_eq!(e.text, "Hi !\nBye X");
        assert_eq!(e.caret_back, "!\nBye X".chars().count());
        let d = expand("{date}", None).text;
        assert_eq!(d.len(), 10);
    }
}
