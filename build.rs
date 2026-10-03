// SPDX-License-Identifier: GPL-3.0-or-later

//! Puts the program icon into the exe. Windows shows that icon on the taskbar, in Explorer and in
//! the "Open with" list. The icon file is turned into a Windows resource file here, and the linker
//! takes it from there, so no extra tools or crates are needed.

use std::io::Write;
use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// One entry of a .res file: a numbered resource of a numbered type.
fn entry(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes());
    out.extend([0xFF, 0xFF]);
    out.extend(kind.to_le_bytes());
    out.extend([0xFF, 0xFF]);
    out.extend(id.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend(0x0409u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ui/img/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let ico = std::fs::read("ui/img/app.ico").expect("ui/img/app.ico");
    let count = u16_at(&ico, 4) as usize;

    let mut res = Vec::new();
    // A .res file starts with an empty entry.
    res.extend(0u32.to_le_bytes());
    res.extend(32u32.to_le_bytes());
    res.extend([0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0]);
    res.extend([0u8; 16]);

    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes());
    group.extend(1u16.to_le_bytes());
    group.extend((count as u16).to_le_bytes());
    for n in 0..count {
        let at = 6 + 16 * n;
        let size = u32_at(&ico, at + 8) as usize;
        let offset = u32_at(&ico, at + 12) as usize;
        let id = (n + 1) as u16;
        entry(&mut res, RT_ICON, id, 0x1010, &ico[offset..offset + size]);
        // The directory entry is the file's one, with the image offset replaced by the resource number.
        group.extend(&ico[at..at + 12]);
        group.extend(id.to_le_bytes());
    }
    entry(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);

    let path = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("app-icon.res");
    std::fs::File::create(&path).and_then(|mut f| f.write_all(&res)).expect("write the resource file");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
