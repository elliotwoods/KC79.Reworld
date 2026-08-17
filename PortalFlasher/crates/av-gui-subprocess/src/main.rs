//! Dedicated CEF renderer/GPU/utility-process entry point.

fn main() {
    let code = av_gui_cef_sys::execute_process().unwrap_or(0);
    std::process::exit(code);
}
