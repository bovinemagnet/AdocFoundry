//! HTML on the clipboard, with plain text beside it (#794). GPUI's clipboard holds text or a
//! picture, so this writes through `arboard`, on the platforms whose clipboard a program hands
//! its data to. On Linux the program that copied keeps the data and answers for it, which GPUI
//! does for its own windows, so formatted text is not offered there.

/// Whether formatted text can be copied on this platform.
pub(crate) const AVAILABLE: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// Puts `html` on the clipboard with `text` beside it, returning whether it did.
#[cfg(all(not(test), any(target_os = "macos", target_os = "windows")))]
pub(crate) fn write(html: &str, text: &str) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_html(html, Some(text)))
        .is_ok()
}

#[cfg(all(not(test), not(any(target_os = "macos", target_os = "windows"))))]
pub(crate) fn write(_: &str, _: &str) -> bool {
    false
}

#[cfg(test)]
thread_local! {
    /// What tests wrote, in place of the clipboard of the person running them.
    pub(crate) static WRITTEN: std::cell::RefCell<Option<(String, String)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn write(html: &str, text: &str) -> bool {
    WRITTEN.with(|written| *written.borrow_mut() = Some((html.to_owned(), text.to_owned())));
    true
}
