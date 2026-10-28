//! English game text from `DataPC.forge` (LocalizationPackage_English,
//! 0x176ce0f5). Needs `ACBMP_SCHEMAS` and `ACBMP_MULTI`; skipped otherwise.

use std::path::PathBuf;

use acb_world::{load_schema, open_map};

#[test]
fn english_mode_text() {
    let (Some(schemas), Some(multi)) = (std::env::var_os("ACBMP_SCHEMAS"), std::env::var_os("ACBMP_MULTI")) else {
        eprintln!("skipped: set ACBMP_SCHEMAS and ACBMP_MULTI");
        return;
    };
    let forge = PathBuf::from(multi).join("DataPC.forge");
    let schema = load_schema(&PathBuf::from(schemas)).unwrap();
    let a = open_map(&forge, &schema).unwrap();
    let strings = a.localization("LocalizationPackage_English").unwrap();
    assert_eq!(strings.len(), 10703);
    let text = |id: u32| strings.iter().find(|s| s.id == id).map(|s| s.text.as_str()).unwrap();

    // GameMode Assassinate (0xebec9a91): NameString / DescriptionString OasisLineIDs
    let mode = a.root(0xebec9a91).unwrap();
    let line = |field: &str| {
        let ui = a.field(&mode.obj, field).and_then(|v| v.as_obj()).unwrap();
        a.field(ui, "OasisLineID").and_then(|v| v.as_u32()).unwrap()
    };
    assert_eq!(line("NameString"), 0x4bd31);
    assert_eq!(text(0x4bd31), "ASSASSINATE");
    assert_eq!(line("DescriptionString"), 0x4e102);
    assert!(text(0x4e102).contains("you must lock on a target"));
    assert_eq!(text(0x4bd32), "ESCORT");
}
