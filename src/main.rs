#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod core;
mod platform;
mod storage;

slint::include_modules!();

fn main() -> std::process::ExitCode {
    if let Err(message) = app::run() {
        eprintln!("OTPLeaf: {message}");
        rfd::MessageDialog::new()
            .set_title("OTPLeaf")
            .set_description(&message)
            .set_level(rfd::MessageLevel::Error)
            .show();
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
