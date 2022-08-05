use std::fmt::Debug;

pub fn join_path(s1: &str, s2: &str) -> String {
    if s1.ends_with('/') {
        format!("{}{}", s1, s2)
    } else {
        format!("{}/{}", s1, s2)
    }
}

pub fn log_warn(msg: &dyn Debug) {
    eprintln!("WARN: {:?}", msg);
}
