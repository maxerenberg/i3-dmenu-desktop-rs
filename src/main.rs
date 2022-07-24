use i3_dmenu_desktop_rs::XDGManager;

fn main() {
    let mgr = XDGManager::new(|s| std::env::var(s));
    if let Err(err) = mgr.start_app_launcher() {
        eprintln!("{:?}", err);
    }
}
