//! Converts assets/Stunstick_Icon.png into a multi-size .ico and embeds it
//! (plus version info) into the Windows EXE, so the file itself shows the icon.

use std::io::Write;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/Stunstick_Icon.png");
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico_path = out.join("stunstick.ico");
    let img = image::open("assets/Stunstick_Icon.png").expect("assets/Stunstick_Icon.png").to_rgba8();

    // ICO with PNG-compressed entries (supported since Windows Vista)
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut pngs = Vec::new();
    for s in sizes {
        let resized = image::imageops::resize(&img, s, s, image::imageops::FilterType::Lanczos3);
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(resized).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).expect("encode png");
        pngs.push((s, buf));
    }
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (s, data) in &pngs {
        let dim = if *s >= 256 { 0u8 } else { *s as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &pngs {
        ico.extend_from_slice(data);
    }
    std::fs::File::create(&ico_path).and_then(|mut f| f.write_all(&ico)).expect("write ico");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().expect("utf8 path"));
    res.set("ProductName", "Golden Stunstick");
    res.set("FileDescription", "Golden Stunstick - Source engine modding workbench");
    res.set("OriginalFilename", "GoldenStunstick.exe");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the EXE icon: {e}");
    }
}
