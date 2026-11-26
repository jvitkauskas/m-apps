// SPDX-License-Identifier: MIT
//! libFuzzer: kit/fat.rs on any card image (kit::card on an image in memory), and the launcher's catalog on it
//! (apps/picker/catalog.rs): mounting, folders, long names, reading a file, META.TXT, then a file written and
//! replaced, as an app saving does. What's read must stay inside the card (a sector past it fails, as a card's would),
//! and nothing may hang (a loop in the FAT has to end in an error: libFuzzer's -timeout).
#![no_main]
#[macro_use]
#[path = "target.rs"]
mod target;
#[allow(dead_code)]
#[path = "../../apps/picker/catalog.rs"]
mod catalog;
use kit::card;
use kit::fat::{ATTR_ARCHIVE, Entry, Named, Volume};

const MOST: usize = 96 * 1024;

fuzz!(|data| {
    if data.len() > MOST {
        return;
    }
    // Exactly the sectors the input fills, so the card ends where it does (none for no input).
    let sectors = data.len().div_ceil(512);
    let image: &'static mut [u8] = Box::leak(vec![0u8; sectors * 512].into_boxed_slice());
    image[..data.len()].copy_from_slice(data);
    let image_at: *mut [u8] = image;
    card::attach(image);
    run(sectors as u32);
    // The card's next image replaces it before it's read again.
    card::attach(&mut []);
    // Safety: leaked above, and the card no longer holds it.
    drop(unsafe { Box::from_raw(image_at) });
});

fn run(sectors: u32) {
    let mut v = Box::new(Volume::new());
    if v.mount(sectors).is_err() {
        return;
    }
    let root = v.root();
    let mut folder = root;
    let mut named = [Named::empty(); 8];
    let mut entries = [Entry::default(); 8];
    let _ = v.list_named(root, b"TXT", &mut named);
    let _ = v.list_folders(root, &mut entries);
    let _ = v.list(root, b"TXT", &mut entries);
    let wifi = v.find(root, b"WIFI    TXT");
    if let Ok(Some(file)) = wifi {
        let mut text = [0u8; 4096];
        let n = (file.size as usize).min(text.len());
        v.read(&file, 0, &mut text[..n]);
    }
    let found = matches!(wifi, Ok(Some(_)));
    if v.folder(&mut folder, b"MBOOT      ", "").is_ok() && v.folder(&mut folder, b"APPS       ", "").is_ok() {
        let mut c = Box::new(catalog::Catalog::new());
        let mut more = false;
        let _ = catalog::read(&mut v, folder, &mut c, &mut more);
    }
    // A file written, as an app saves: a new one that reports success must read back as written. (A replaced one is
    // written too, but not checked: on a damaged FAT that had the old file's clusters free, the new file can be given
    // them and then lose them as the old chain is freed, which kit/fat.rs only partly guards against.)
    let Ok(start) = v.reserve(1000) else { return };
    let block: [u8; 1024] = core::array::from_fn(|i| (i * 7 + 1) as u8);
    // Replaced if it's there (add() is for a name the folder doesn't have), as the WiFi app saves.
    let name = if found { b"WIFI    TXT" } else { b"NEW     TXT" };
    // A folder that can't be searched is taken to have it, so it isn't added to.
    let exists = found || !matches!(v.find(root, name), Ok(None));
    if v.write(start, 0, &block)
        && if exists { v.swap(root, name, start, 1000) } else { v.add(root, name, ATTR_ARCHIVE, start, 1000) }.is_ok()
        && v.commit().is_ok()
        && !exists
    {
        let mut back = [0u8; 1000];
        let ok = match v.find(root, name) {
            Ok(Some(written)) => written.size == 1000 && v.read(&written, 0, &mut back) && back == block[..1000],
            _ => false,
        };
        if !ok {
            eprintln!("a file written isn't read back as written");
            std::process::abort();
        }
    }
}
