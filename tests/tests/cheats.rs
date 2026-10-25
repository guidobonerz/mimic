//! Cheat files applied to a real machine (sim6502): freezes and patches,
//! all or nothing; imports end to end.

use emu_cheats::{apply, from_hooks, CheatFile, CheatFreeze, CheatPatch};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Size};

#[test]
fn apply_freezes_and_patches_all_or_nothing() {
    let mut m = emu_machines::registry()
        .create("sim6502", &MachineConfig::default())
        .unwrap();
    let space = m.memory_spaces()[0].name.to_string();
    let mut hooks = DebugHooks::new();
    let mut file = CheatFile::default();
    file.freeze.push(CheatFreeze {
        label: None,
        space: space.clone(),
        addr: 0x10,
        size: 1,
        value: 7,
        mode: "block_writes".into(),
        enabled: true,
    });
    file.patch.push(CheatPatch {
        label: None,
        space: space.clone(),
        addr: 0x20,
        bytes: vec![1, 2, 3],
        enabled: true,
    });
    let applied = apply(&file, m.as_mut(), &mut hooks).unwrap();
    assert_eq!((applied.freezes.len(), applied.patches), (1, 1));
    let dev = m.devices()[0];
    let sp = m.memory_spaces()[0].id;
    let t = m.debug_target(dev).unwrap();
    assert_eq!(t.peek(sp, 0x10, Size::Byte), Some(7));
    assert_eq!(t.peek(sp, 0x22, Size::Byte), Some(3));
    assert_eq!(from_hooks(m.as_ref(), &hooks).freeze[0].value, 7);
    // A bad entry: nothing is added.
    file.freeze[0].space = "nowhere".into();
    let mut fresh = DebugHooks::new();
    assert!(apply(&file, m.as_mut(), &mut fresh).is_err());
    assert_eq!(fresh.freezes().count(), 0);
}

#[test]
fn an_imported_poke_list_freezes_the_value() {
    let mut m = emu_machines::registry()
        .create("sim6502", &MachineConfig::default())
        .unwrap();
    let file = emu_cheats::import::import("pokes", "10 POKE 4096,99: REM lives\n", "cpu").unwrap();
    let mut hooks = DebugHooks::new();
    apply(&file, m.as_mut(), &mut hooks).unwrap();
    let dev = m.devices()[0];
    let sp = m.memory_spaces()[0].id;
    assert_eq!(
        m.debug_target(dev).unwrap().peek(sp, 4096, Size::Byte),
        Some(99)
    );
    assert_eq!(hooks.freezes().count(), 1);
}
