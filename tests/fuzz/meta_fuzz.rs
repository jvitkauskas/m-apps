// SPDX-License-Identifier: MIT
//! libFuzzer: kit/load.rs's META.TXT reader (the launcher's names) on any bytes.
#![no_main]
#[macro_use]
#[path = "target.rs"]
mod target;
use kit::load::{AppText, meta_parse};

fuzz!(|data| {
    // As the launcher has it before: the folder's name, and the rest empty. Unused bytes are left as noise, so a value
    // that isn't ended shows.
    let mut text = AppText { name: [0x5a; 25], about: [0x5a; 61], version: [0x5a; 13] };
    text.name[..6].copy_from_slice(b"HELLO\0");
    text.about[0] = 0;
    text.version[0] = 0;
    meta_parse(data, &mut text);
    // Each one ended within its field, as the launcher shows them up to their zero. (Each is an array of its own, so
    // a value can't spill into the next one: the bounds check would panic first.)
    assert!(text.name.contains(&0) && text.about.contains(&0) && text.version.contains(&0));
});
