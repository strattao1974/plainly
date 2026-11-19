use std::{env, fs, path::PathBuf};

#[path = "src/icon_art.rs"]
mod icon_art;

fn main() {
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ico_bytes = make_ico(&[16, 20, 24, 32, 40, 48, 64, 256]);
    let ico = out.join("plainly.ico");
    fs::write(&ico, &ico_bytes).unwrap();

    // The installer script needs a real .ico file on disk.
    let installer_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("installer");
    if installer_dir.is_dir() {
        let target = installer_dir.join("plainly.ico");
        if fs::read(&target).ok().as_deref() != Some(&ico_bytes[..]) {
            let _ = fs::write(target, &ico_bytes);
        }
    }

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let major = env::var("CARGO_PKG_VERSION_MAJOR").unwrap();
    let minor = env::var("CARGO_PKG_VERSION_MINOR").unwrap();
    let patch = env::var("CARGO_PKG_VERSION_PATCH").unwrap();
    // FileDescription is the name Windows shows on Plainly's notifications.
    let rc = format!(
        r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Plainly"
      VALUE "FileDescription", "Plainly"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "plainly"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "plainly.exe"
      VALUE "ProductName", "Plainly"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        ico = ico.display().to_string().replace('\\', "\\\\"),
    );
    let rc_path = out.join("plainly.rc");
    fs::write(&rc_path, rc).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE);
}

fn make_ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| bmp_entry(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (i, &s) in sizes.iter().enumerate() {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(images[i].len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += images[i].len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

fn bmp_entry(size: u32) -> Vec<u8> {
    let rgba = icon_art::render(size);
    let mask_row = ((size + 31) / 32) * 4;
    let mut out = Vec::new();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(size * size * 4 + mask_row * size).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    out.extend(std::iter::repeat(0u8).take((mask_row * size) as usize));
    out
}
