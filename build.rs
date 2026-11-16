//! Embeds `assets/snipt.ico` as the exe's icon (what Explorer, the taskbar
//! and shortcuts show; Snipt also loads it for the tray).
//!
//! No build dependencies and no Windows SDK tools: the icon is packed into a
//! compiled resource file (.res) right here, and the MSVC linker takes a .res
//! as a normal input.

use std::{env, fs, path::PathBuf};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
/// Resource id of the icon group; `main.rs` loads it with the same number.
const APP_ICON: u16 = 1;

fn main() {
    println!("cargo:rerun-if-changed=assets/snipt.ico");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let ico = fs::read("assets/snipt.ico").expect("assets/snipt.ico (run assets/make_icon.ps1)");
    let res = PathBuf::from(env::var("OUT_DIR").unwrap()).join("snipt.res");
    fs::write(&res, icon_res(&ico)).unwrap();
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// One resource: a header naming it by number, then its data, padded to 4 bytes.
fn push_resource(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes()); // DataSize
    out.extend(32u32.to_le_bytes()); // HeaderSize
    out.extend([0xFF, 0xFF]);
    out.extend(kind.to_le_bytes()); // Type (ordinal)
    out.extend([0xFF, 0xFF]);
    out.extend(id.to_le_bytes()); // Name (ordinal)
    out.extend(0u32.to_le_bytes()); // DataVersion
    out.extend(flags.to_le_bytes()); // MemoryFlags
    out.extend(0u16.to_le_bytes()); // LanguageId: neutral
    out.extend(0u32.to_le_bytes()); // Version
    out.extend(0u32.to_le_bytes()); // Characteristics
    out.extend(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

/// Turns an .ico file into a .res: each image becomes an RT_ICON, and an
/// RT_GROUP_ICON lists them (same entries as the .ico directory, but with
/// resource ids instead of file offsets).
fn icon_res(ico: &[u8]) -> Vec<u8> {
    assert!(u16_at(ico, 0) == 0 && u16_at(ico, 2) == 1, "not an .ico file");
    let count = u16_at(ico, 4);
    let mut out = Vec::new();
    push_resource(&mut out, 0, 0, 0, &[]); // a .res file starts with an empty entry
    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes());
    group.extend(1u16.to_le_bytes());
    group.extend(count.to_le_bytes());
    for i in 0..count as usize {
        let e = 6 + 16 * i;
        let (len, offset) = (u32_at(ico, e + 8) as usize, u32_at(ico, e + 12) as usize);
        let id = i as u16 + 1;
        push_resource(&mut out, RT_ICON, id, 0x1010, &ico[offset..offset + len]);
        group.extend(&ico[e..e + 12]); // width, height, colours, reserved, planes, bits, size
        group.extend(id.to_le_bytes());
    }
    push_resource(&mut out, RT_GROUP_ICON, APP_ICON, 0x1030, &group);
    out
}
