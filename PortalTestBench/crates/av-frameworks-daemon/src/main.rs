fn main() {
    let auto_exit_when_idle = std::env::args().any(|arg| arg == "--auto-exit-when-idle");
    if let Err(error) = av_frameworks_daemon_core::run(auto_exit_when_idle) {
        eprintln!("av-frameworks-daemon: {error}");
        std::process::exit(1);
    }
}
