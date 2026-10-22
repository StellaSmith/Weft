pub fn get() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

pub fn set(text: &str) -> bool {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())).is_ok()
}
