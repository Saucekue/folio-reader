use std::{env, fs, path::PathBuf};

const ICON: &[u8] = include_bytes!("assets/folio.ico");

fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn dword(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
}

fn resource(resources: &mut Vec<u8>, kind: u16, id: u16, language: u16, data: &[u8]) {
    resources.extend((data.len() as u32).to_le_bytes());
    resources.extend(32u32.to_le_bytes());
    for number in [0xffff, kind, 0xffff, id] {
        resources.extend(number.to_le_bytes());
    }
    resources.extend(0u32.to_le_bytes());
    resources.extend(0u16.to_le_bytes());
    resources.extend(language.to_le_bytes());
    resources.extend(0u64.to_le_bytes());
    resources.extend(data);
    resources.resize(resources.len().next_multiple_of(4), 0);
}

fn main() {
    println!("cargo:rerun-if-changed=assets/folio.ico");
    let mut resources = Vec::new();
    resource(&mut resources, 0, 0, 0, &[]);
    let mut group = ICON[..6].to_vec();
    for index in 0..word(ICON, 4) {
        let at = 6 + 16 * usize::from(index);
        let size = dword(ICON, at + 8);
        let offset = dword(ICON, at + 12);
        let id = index + 1;
        resource(&mut resources, 3, id, 0x0409, &ICON[offset..offset + size]);
        group.extend(&ICON[at..at + 12]);
        group.extend(id.to_le_bytes());
    }
    resource(&mut resources, 14, 101, 0x0409, &group);
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("folio.res");
    fs::write(&output, resources).unwrap();
    println!("cargo:rustc-link-arg-bin=folio-reader={}", output.display());
}
