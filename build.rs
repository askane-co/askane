use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn le16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn le32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_coff_icon_object(ico: &[u8], out_path: &Path) -> Result<(), String> {
    if ico.len() < 6 {
        return Err("ICO file too small".to_string());
    }
    let ico_type = u16::from_le_bytes([ico[2], ico[3]]);
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if ico_type != 1 || count == 0 || ico.len() < 6 + count * 16 {
        return Err("Invalid ICO header".to_string());
    }

    let mut frames: Vec<(&[u8; 16], &[u8])> = Vec::with_capacity(count);
    for i in 0..count {
        let off = 6 + i * 16;
        let entry: &[u8; 16] = ico[off..off + 16].try_into().unwrap();
        let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let img_off = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        if img_off.saturating_add(size) > ico.len() {
            return Err("ICO frame out of bounds".to_string());
        }
        frames.push((entry, &ico[img_off..img_off + size]));
    }

    let mut grp_data = Vec::with_capacity(6 + count * 14);
    le16(&mut grp_data, 0);
    le16(&mut grp_data, 1);
    le16(&mut grp_data, count as u16);
    for (idx, (entry, _)) in frames.iter().enumerate() {
        grp_data.extend_from_slice(&entry[0..12]);
        le16(&mut grp_data, (idx + 1) as u16);
    }

    // Build .rsrc section tree:
    // Root dir (2 id entries: 3=RT_ICON, 14=RT_GROUP_ICON)
    let root_off = 0usize;
    let icon_type_dir_off = 16 + 2 * 8; // 32
    let grp_type_dir_off = icon_type_dir_off + 16 + count * 8;
    let lang_dirs_off = grp_type_dir_off + 16 + 8;
    let data_entries_off = lang_dirs_off + (count + 1) * (16 + 8);
    let payloads_off = data_entries_off + (count + 1) * 16;

    let mut rsrc = vec![0u8; payloads_off];

    let put_u16 = |buf: &mut [u8], pos: usize, v: u16| {
        buf[pos..pos + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put_u32 = |buf: &mut [u8], pos: usize, v: u32| {
        buf[pos..pos + 4].copy_from_slice(&v.to_le_bytes());
    };

    // Root directory
    put_u16(&mut rsrc, root_off + 14, 2);
    put_u32(&mut rsrc, root_off + 16, 3);
    put_u32(
        &mut rsrc,
        root_off + 20,
        0x8000_0000 | (icon_type_dir_off as u32),
    );
    put_u32(&mut rsrc, root_off + 24, 14);
    put_u32(
        &mut rsrc,
        root_off + 28,
        0x8000_0000 | (grp_type_dir_off as u32),
    );

    // Type=3 (RT_ICON) name/id directory
    put_u16(&mut rsrc, icon_type_dir_off + 14, count as u16);
    for i in 0..count {
        let e = icon_type_dir_off + 16 + i * 8;
        let lang_dir = lang_dirs_off + i * 24;
        put_u32(&mut rsrc, e, (i + 1) as u32);
        put_u32(&mut rsrc, e + 4, 0x8000_0000 | (lang_dir as u32));
    }

    // Type=14 (RT_GROUP_ICON) name/id directory
    put_u16(&mut rsrc, grp_type_dir_off + 14, 1);
    let grp_lang_dir = lang_dirs_off + count * 24;
    put_u32(&mut rsrc, grp_type_dir_off + 16, 1);
    put_u32(
        &mut rsrc,
        grp_type_dir_off + 20,
        0x8000_0000 | (grp_lang_dir as u32),
    );

    // Language directories (0x0409) pointing to IMAGE_RESOURCE_DATA_ENTRY
    for i in 0..=count {
        let ld = lang_dirs_off + i * 24;
        let de = data_entries_off + i * 16;
        put_u16(&mut rsrc, ld + 14, 1);
        put_u32(&mut rsrc, ld + 16, 0x0409);
        put_u32(&mut rsrc, ld + 20, de as u32);
    }

    let mut reloc_offsets = Vec::with_capacity(count + 1);
    for (i, (_, frame_bytes)) in frames.iter().enumerate() {
        let de = data_entries_off + i * 16;
        let data_rsrc_off = rsrc.len() as u32;
        rsrc.extend_from_slice(frame_bytes);
        while !rsrc.len().is_multiple_of(4) {
            rsrc.push(0);
        }
        put_u32(&mut rsrc, de, data_rsrc_off);
        put_u32(&mut rsrc, de + 4, frame_bytes.len() as u32);
        reloc_offsets.push(de as u32);
    }

    let grp_de = data_entries_off + count * 16;
    let grp_rsrc_off = rsrc.len() as u32;
    rsrc.extend_from_slice(&grp_data);
    while !rsrc.len().is_multiple_of(4) {
        rsrc.push(0);
    }
    put_u32(&mut rsrc, grp_de, grp_rsrc_off);
    put_u32(&mut rsrc, grp_de + 4, grp_data.len() as u32);
    reloc_offsets.push(grp_de as u32);

    // Emit COFF object (.o)
    let raw_data_ptr = 20 + 40;
    let reloc_ptr = raw_data_ptr + rsrc.len();
    let symtab_ptr = reloc_ptr + reloc_offsets.len() * 10;

    let mut obj = Vec::with_capacity(symtab_ptr + 22);
    // IMAGE_FILE_HEADER
    le16(&mut obj, 0x8664); // IMAGE_FILE_MACHINE_AMD64
    le16(&mut obj, 1); // NumberOfSections
    le32(&mut obj, 0); // TimeDateStamp
    le32(&mut obj, symtab_ptr as u32);
    le32(&mut obj, 1); // NumberOfSymbols
    le16(&mut obj, 0); // SizeOfOptionalHeader
    le16(&mut obj, 0); // Characteristics

    // IMAGE_SECTION_HEADER (.rsrc)
    obj.extend_from_slice(b".rsrc\0\0\0");
    le32(&mut obj, 0); // VirtualSize
    le32(&mut obj, 0); // VirtualAddress
    le32(&mut obj, rsrc.len() as u32);
    le32(&mut obj, raw_data_ptr as u32);
    le32(&mut obj, reloc_ptr as u32);
    le32(&mut obj, 0); // PointerToLinenumbers
    le16(&mut obj, reloc_offsets.len() as u16);
    le16(&mut obj, 0); // NumberOfLinenumbers
    le32(&mut obj, 0x4000_0040); // IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ

    obj.extend_from_slice(&rsrc);

    // Relocations: IMAGE_REL_AMD64_ADDR32NB = 3
    for r_va in reloc_offsets {
        le32(&mut obj, r_va);
        le32(&mut obj, 0);
        le16(&mut obj, 3);
    }

    // Symbol table: ".rsrc" section symbol (index 0)
    obj.extend_from_slice(b".rsrc\0\0\0");
    le32(&mut obj, 0);
    le16(&mut obj, 1); // SectionNumber = 1
    le16(&mut obj, 0); // Type
    obj.push(3); // IMAGE_SYM_CLASS_STATIC
    obj.push(0); // NumberOfAuxSymbols

    // Empty string table (4-byte length = 4)
    le32(&mut obj, 4);

    fs::write(out_path, obj).map_err(|e| e.to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=docs/askane.ico");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_os != "windows" || target_arch != "x86_64" {
        return;
    }
    let ico_path = Path::new("docs/askane.ico");
    let Ok(ico_bytes) = fs::read(ico_path) else {
        return;
    };
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let obj_path = out_dir.join("askane_icon.o");
    write_coff_icon_object(&ico_bytes, &obj_path).expect("failed to build Windows icon resource");
    println!("cargo:rustc-link-arg-bins={}", obj_path.display());
}
