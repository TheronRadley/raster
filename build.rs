//! Embed the checked-in Windows icon without requiring an external resource compiler.
use std::env;
use std::fs;
use std::path::PathBuf;

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn put_u16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn resource(out: &mut Vec<u8>, kind: u16, name: u16, data: &[u8]) {
    out.resize((out.len() + 3) & !3, 0);
    put_u32(out, u32::try_from(data.len()).expect("resource too large"));
    put_u32(out, 32); // fixed-size ordinal resource header
    put_u16(out, 0xffff);
    put_u16(out, kind);
    put_u16(out, 0xffff);
    put_u16(out, name);
    put_u32(out, 0);
    put_u16(out, 0x1030);
    put_u16(out, 0x0409);
    put_u32(out, 0);
    put_u32(out, 0);
    out.extend_from_slice(data);
    out.resize((out.len() + 3) & !3, 0);
}

fn main() {
    println!("cargo:rerun-if-changed=assets/raster.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let ico = fs::read("assets/raster.ico").expect("missing Raster icon");
    assert!(
        ico.len() >= 6 && u16_at(&ico, 0) == 0 && u16_at(&ico, 2) == 1,
        "invalid icon"
    );
    let count = u16_at(&ico, 4);
    assert!(
        count > 0 && ico.len() >= 6 + usize::from(count) * 16,
        "invalid icon directory"
    );
    let mut output = Vec::new();
    // Required empty resource header, with language and flags zeroed.
    output.extend_from_slice(&[
        0, 0, 0, 0, 32, 0, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0,
    ]);
    let mut group = ico[..6].to_vec();
    for i in 0..count {
        let entry = 6 + usize::from(i) * 16;
        let size = u32_at(&ico, entry + 8) as usize;
        let offset = u32_at(&ico, entry + 12) as usize;
        let end = offset.checked_add(size).expect("invalid icon size");
        let image = ico.get(offset..end).expect("invalid icon image");
        resource(&mut output, 3, i + 1, image); // RT_ICON
        group.extend_from_slice(&ico[entry..entry + 12]);
        put_u16(&mut group, i + 1);
    }
    resource(&mut output, 14, 1, &group); // RT_GROUP_ICON
    let path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("raster-icon.res");
    fs::write(&path, output).expect("cannot write Windows icon resource");
    println!("cargo:rustc-link-arg-bin=raster={}", path.display());
}
