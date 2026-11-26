// SPDX-License-Identifier: MIT
//! The launcher's list: the apps on the card, each a folder, /MBOOT/APPS/<ID>/, with BOOT.APP, and ICON.QOI and
//! META.TXT if it likes.
use kit::fat::{Entry, Error, Volume, until_zero};
use kit::load::{self, AppText, META_MAX};

pub const MOST: usize = 100;

/// An app on the card. Plain numbers and bytes only, as the list is in the apps' slot, which holds whatever was there.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct App {
    /// BOOT.APP
    pub file: Entry,
    /// ICON.QOI; size 0 without one
    pub icon: Entry,
    /// the folder's cluster
    pub folder: u32,
    /// the folder's 8.3 name
    pub id: [u8; 8],
    /// META.TXT, or the folder's name
    pub text: AppText,
    /// where to call it with its folder, or 0 (kit/app.rs's AppLaunch)
    pub launched_entry: u32,
}

impl App {
    pub const fn empty() -> App {
        let none = Entry { name: [0; 8], start: 0, size: 0 };
        App { file: none, icon: none, folder: 0, id: [0; 8], text: AppText::empty(), launched_entry: 0 }
    }
}

#[repr(C)]
pub struct Catalog {
    pub apps: [App; MOST],
    pub count: usize,
    pub folders: [Entry; MOST],
    /// a META.TXT, as it's read
    pub meta: [u8; META_MAX],
}

impl Catalog {
    /// For the tests on this computer: the launcher's is in the apps' slot.
    #[allow(dead_code)]
    pub const fn new() -> Catalog {
        Catalog { apps: [App::empty(); MOST], count: 0, folders: [Entry { name: [0; 8], start: 0, size: 0 }; MOST], meta: [0; META_MAX] }
    }
}

/// the 8.3 name, without its padding, as the name to show
fn name_from(to: &mut [u8], id: &[u8; 8]) {
    let n = id.iter().position(|&c| c == b' ').unwrap_or(8);
    to[..n].copy_from_slice(&id[..n]);
    to[n] = 0;
}

/// Ignoring ASCII case, up to each one's zero.
fn compare(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
    let (a, b) = (until_zero(a), until_zero(b));
    a.iter().map(u8::to_ascii_uppercase).cmp(b.iter().map(u8::to_ascii_uppercase))
}

/// The apps in `folder` (/MBOOT/APPS, on a mounted card), by name as shown, ignoring case. `more` says whether there
/// were more folders than MOST.
pub fn read(volume: &mut Volume, folder: u32, c: &mut Catalog, more: &mut bool) -> Result<(), Error> {
    c.count = 0;
    // A folder without BOOT.APP isn't an app's.
    let n;
    (n, *more) = volume.list_folders(folder, &mut c.folders)?;
    for i in 0..n {
        let start = c.folders[i].start;
        let Ok(Some(file)) = volume.find(start, b"BOOT    APP") else { continue };
        let a = &mut c.apps[c.count];
        *a = App::empty();
        a.file = file;
        a.folder = start;
        a.id = c.folders[i].name;
        if let Ok(Some(icon)) = volume.find(a.folder, b"ICON    QOI") {
            a.icon = icon;
        }
        a.launched_entry = load::launched_entry(volume, &a.file);
        name_from(&mut a.text.name, &a.id);
        if let Ok(Some(meta)) = volume.find(a.folder, b"META    TXT") {
            let text = &mut c.meta[..(meta.size as usize).min(META_MAX)];
            if volume.read(&meta, 0, text) {
                load::meta_parse(text, &mut a.text);
            }
        }
        c.count += 1;
        kit::app::feed();
    }
    // By the name shown, the way a person would look for it.
    for i in 1..c.count {
        let mut j = i;
        while j > 0 && compare(&c.apps[j - 1].text.name, &c.apps[j].text.name).is_gt() {
            c.apps.swap(j - 1, j);
            j -= 1;
        }
    }
    if c.count != 0 { Ok(()) } else { Err("NO APPS IN /MBOOT/APPS") }
}
