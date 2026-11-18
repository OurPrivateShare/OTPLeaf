pub mod qr;

#[cfg(target_os = "linux")]
mod wayland;

#[cfg(target_os = "linux")]
mod linux_tray;
#[cfg(target_os = "linux")]
pub use linux_tray::Tray;

#[cfg(target_os = "windows")]
mod windows_tray;
#[cfg(target_os = "windows")]
pub use windows_tray::Tray;

#[derive(Clone, Debug)]
pub enum TrayAction {
    Show,
    ScanScreen,
    Copy(String),
    Quit,
}

const ICON_SIZE: i32 = 32;

/// Pixel colors for the compact leaf mark, shared by both native icon formats.
fn icon_pixel(x: i32, y: i32) -> [u8; 4] {
    let (x, y) = (x as f32 + 0.5 - 16., y as f32 + 0.5 - 16.);
    if x * x + y * y > 235. {
        return [0, 0, 0, 0];
    }
    let along = (x - y) * std::f32::consts::FRAC_1_SQRT_2;
    let across = (x + y) * std::f32::consts::FRAC_1_SQRT_2;
    if across.abs() < 6. * (1. - (along / 11.).powi(2)) && !(across.abs() < 0.9 && along < 5.) {
        [219, 245, 173, 255]
    } else {
        [29, 91, 65, 255]
    }
}
