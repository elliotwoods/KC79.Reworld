fn main() {
    // Links the Win32 icon resource so Explorer, shortcuts and -- via WNDCLASSEX.hIcon, which
    // has to be right before the button is created -- the taskbar button all show a real mark.
    // Also best-effort registers this build for launcher discovery; failure is never fatal.
    av_app_icon::embed();
}
