use i3_dmenu_desktop_rs::XDGManager;
use gettextrs::{LocaleCategory, setlocale};

fn main() {
    let buf = setlocale(LocaleCategory::LcMessages, "").unwrap();
    let locale = std::str::from_utf8(&buf).unwrap();

    let mgr = XDGManager::new(|s| std::env::var(s), locale);
    if let Err(err) = mgr.start_app_launcher() {
        eprintln!("{:?}", err);
    }
}
