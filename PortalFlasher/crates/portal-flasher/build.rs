//! The window caption and Alt-Tab icon come free from the shell, but the taskbar button and the
//! file in Explorer need a Win32 icon resource linked into this executable — which only the
//! crate producing it can do. One line does both.
fn main() {
    av_app_icon::embed();
}
