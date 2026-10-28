#[cfg(windows)]
fn main() {
    std::process::exit(host_win::win_session::main_with_args(
        std::env::args_os().skip(1),
    ));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("rs-host-session runs on Windows only");
    std::process::exit(2);
}
