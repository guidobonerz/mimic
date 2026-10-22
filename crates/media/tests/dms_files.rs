//! Unpack real DMS archives: `EMU_DMS_DIR` names a folder of `.dms` files
//! (not in the repository); every track's checksum and CRC must agree and
//! the result must be a boot-block-sized ADF.
//!
//!     EMU_DMS_DIR=/path cargo test -p media --test dms_files -- --ignored --nocapture

#[test]
#[ignore = "needs EMU_DMS_DIR with .dms files"]
fn real_archives_unpack_with_good_checksums() {
    let dir = std::env::var("EMU_DMS_DIR").expect("EMU_DMS_DIR");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("dms"))
        {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        // Which modes does the file use?
        let mut modes = std::collections::BTreeSet::new();
        let mut pos = 56;
        while pos + 20 <= bytes.len() {
            let len = usize::from(u16::from_be_bytes([bytes[pos + 6], bytes[pos + 7]]));
            modes.insert((bytes[pos + 13], bytes[pos + 12]));
            pos += 20 + len;
        }
        let result = media::dms::unpack(&bytes);
        println!(
            "{}: modes (mode, flags) {:?} -> {:?}",
            path.display(),
            modes,
            result.as_ref().map(|img| img.len())
        );
        let img = result.unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        assert!(img.len() == media::adf::ADF_SIZE || img.len() == media::adf::ADF_HD_SIZE);
        println!("  boot block starts {:02x?}", &img[..8]);
        n += 1;
    }
    assert!(n > 0, "no .dms files in {dir}");
}
