//! The global shortcut: key names, parsing, and the settings file.
//!
//! The chosen shortcut is stored in `%APPDATA%\Snipt\settings.txt` as
//! `hotkey = Ctrl+Alt+S`. Without one, Snipt uses Ctrl+Shift+Space, or the
//! first free fallback if another app already has it.

use std::path::PathBuf;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    pub mods: HOT_KEY_MODIFIERS,
    pub vk: u32,
}

/// Tried in order when no shortcut was chosen (or the chosen one is taken).
/// Ctrl+Shift+Space is the default; the others are fallbacks.
pub const DEFAULTS: [Hotkey; 3] = [
    Hotkey { mods: MOD_CONTROL | MOD_SHIFT, vk: VK_SPACE as u32 },
    Hotkey { mods: MOD_CONTROL | MOD_ALT, vk: 'V' as u32 },
    Hotkey { mods: MOD_WIN | MOD_SHIFT, vk: 'V' as u32 },
];

const MODS: [(HOT_KEY_MODIFIERS, &str); 4] =
    [(MOD_WIN, "Win"), (MOD_CONTROL, "Ctrl"), (MOD_ALT, "Alt"), (MOD_SHIFT, "Shift")];

const NAMED: [(u16, &str); 17] = [
    (VK_SPACE, "Space"),
    (VK_RETURN, "Enter"),
    (VK_TAB, "Tab"),
    (VK_BACK, "Backspace"),
    (VK_ESCAPE, "Esc"),
    (VK_INSERT, "Insert"),
    (VK_DELETE, "Delete"),
    (VK_HOME, "Home"),
    (VK_END, "End"),
    (VK_PRIOR, "PageUp"),
    (VK_NEXT, "PageDown"),
    (VK_UP, "Up"),
    (VK_DOWN, "Down"),
    (VK_LEFT, "Left"),
    (VK_RIGHT, "Right"),
    (VK_PAUSE, "Pause"),
    (VK_SNAPSHOT, "PrintScreen"),
];

impl Hotkey {
    /// Modifier names followed by the key, e.g. ["Ctrl", "Shift", "Space"].
    /// The key is shown as it is printed on *your* keyboard layout.
    pub fn parts(self) -> Vec<String> {
        let mut parts = mod_names(self.mods);
        parts.push(display_key(self.vk));
        parts
    }

    /// Text for menus and messages, e.g. "Ctrl+Shift+Space".
    pub fn label(self) -> String {
        self.parts().join("+")
    }

    /// Layout-independent text for the settings file.
    pub fn spec(self) -> String {
        let mut parts = mod_names(self.mods);
        parts.push(key_spec(self.vk));
        parts.join("+")
    }

    pub fn parse(s: &str) -> Option<Hotkey> {
        let mut mods = 0;
        let mut vk = None;
        for token in s.split('+').map(str::trim) {
            if let Some(&(m, _)) = MODS.iter().find(|(_, n)| n.eq_ignore_ascii_case(token)) {
                mods |= m;
            } else if token.eq_ignore_ascii_case("control") {
                mods |= MOD_CONTROL;
            } else if vk.is_none() {
                vk = Some(parse_key(token)?);
            } else {
                return None;
            }
        }
        let h = Hotkey { mods, vk: vk? };
        h.allowed().then_some(h)
    }

    /// Needs Ctrl, Alt or Win (Shift alone would block typing capitals),
    /// except for F-keys, which may be used alone.
    pub fn allowed(self) -> bool {
        is_fkey(self.vk) || self.mods & (MOD_CONTROL | MOD_ALT | MOD_WIN) != 0
    }
}

pub fn mod_names(mods: HOT_KEY_MODIFIERS) -> Vec<String> {
    MODS.iter().filter(|(m, _)| mods & m != 0).map(|(_, n)| n.to_string()).collect()
}

pub fn is_modifier(vk: u16) -> bool {
    matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN | VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU)
}

fn is_fkey(vk: u32) -> bool {
    (VK_F1 as u32..=VK_F24 as u32).contains(&vk)
}

/// Stable name: letters, digits, F-keys and named keys; `VK_xx` for the rest.
fn key_spec(vk: u32) -> String {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => char::from_u32(vk).unwrap_or('?').to_string(),
        _ if is_fkey(vk) => format!("F{}", vk - VK_F1 as u32 + 1),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        _ => NAMED
            .iter()
            .find(|(k, _)| *k as u32 == vk)
            .map_or_else(|| format!("VK_{vk:02X}"), |(_, n)| n.to_string()),
    }
}

/// Like `key_spec`, but punctuation keys show their character on the
/// current keyboard layout (e.g. "Ç" on a Brazilian keyboard).
fn display_key(vk: u32) -> String {
    let spec = key_spec(vk);
    if !spec.starts_with("VK_") {
        return spec;
    }
    let ch = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) } & 0x7FFF; // high bit: dead key
    match char::from_u32(ch) {
        Some(c) if ch > 0x20 => c.to_uppercase().collect(),
        _ => spec,
    }
}

fn parse_key(token: &str) -> Option<u32> {
    let upper = token.to_ascii_uppercase();
    let b = upper.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_uppercase() || b[0].is_ascii_digit()) {
        return Some(b[0] as u32);
    }
    if let Some(n) = upper.strip_prefix("VK_") {
        return u32::from_str_radix(n, 16).ok().filter(|&v| v > 0 && v < 0xFF);
    }
    if let Some(n) = upper.strip_prefix("NUM").and_then(|n| n.parse::<u32>().ok()) {
        return (n <= 9).then_some(0x60 + n);
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then_some(VK_F1 as u32 + n - 1);
    }
    NAMED.iter().find(|(_, n)| n.eq_ignore_ascii_case(token)).map(|&(k, _)| k as u32)
}

// ---------------------------------------------------------------- settings file

fn settings_path() -> PathBuf {
    crate::snippets::file_path().with_file_name("settings.txt")
}

/// The shortcut chosen in "Change shortcut…", if any.
pub fn load_saved() -> Option<Hotkey> {
    let text = std::fs::read_to_string(settings_path()).ok()?;
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("hotkey"))
        .and_then(|(_, v)| Hotkey::parse(v))
}

/// Saves (or with `None`, forgets) the chosen shortcut, keeping other lines.
pub fn save(h: Option<Hotkey>) -> std::io::Result<()> {
    let path = settings_path();
    let old = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        "# Snipt settings. Change the shortcut from the tray menu: Change shortcut...\n".to_string()
    });
    let mut out: Vec<String> = old
        .lines()
        .filter(|l| !l.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("hotkey")))
        .map(str::to_string)
        .collect();
    if let Some(h) = h {
        out.push(format!("hotkey = {}", h.spec()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, out.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_names_shortcuts() {
        for s in ["Ctrl+Shift+Space", "Win+Shift+V", "Ctrl+Alt+V", "Alt+F2", "F9", "Ctrl+Num5", "Ctrl+Alt+VK_BA", "Win+PageDown"] {
            let h = Hotkey::parse(s).unwrap_or_else(|| panic!("{s}"));
            assert_eq!(h.spec(), s);
        }
        assert_eq!(Hotkey::parse("shift + ctrl + space"), Some(DEFAULTS[0]));
        assert_eq!(Hotkey::parse("control+alt+v"), Some(DEFAULTS[1]));
        for bad in ["", "Ctrl", "Shift+A", "A", "Ctrl+A+B", "Ctrl+Nope", "Ctrl+F25"] {
            assert_eq!(Hotkey::parse(bad), None, "{bad}");
        }
        assert_eq!(DEFAULTS[0].label(), "Ctrl+Shift+Space"); // the default
    }
}
