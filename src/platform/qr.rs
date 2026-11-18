use image::{DynamicImage, GrayImage, ImageDecoder, ImageReader};
use rqrr::PreparedImage;
use std::path::Path;

const NO_QR: &str = "未找到 TOTP 二维码，请确认二维码完整、清晰且未被遮挡";
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

fn decode(gray: GrayImage, links: &mut Vec<String>) {
    let mut prepared = PreparedImage::prepare(gray);
    for grid in prepared.detect_grids() {
        if let Ok((_, text)) = grid.decode() {
            let text = text.trim();
            if text
                .get(.."otpauth://totp/".len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("otpauth://totp/"))
                && !links.iter().any(|link| link == text)
            {
                links.push(text.to_owned());
            }
        }
    }
}

pub fn from_image(path: &Path) -> Result<Vec<String>, String> {
    let reader = ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(|e| format!("无法打开图片：{e}"))?;
    let decoder = reader
        .into_decoder()
        .map_err(|e| format!("无法读取图片格式：{e}"))?;
    let (width, height) = decoder.dimensions();
    if !(1..=32768).contains(&width)
        || !(1..=32768).contains(&height)
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err("图片尺寸过大或为空，请裁剪二维码区域后导入".into());
    }
    let image = DynamicImage::from_decoder(decoder).map_err(|e| format!("无法解码图片：{e}"))?;
    // QR generators often export a transparent quiet zone and background.
    // Composite onto white instead of interpreting transparent black as black.
    let gray = if image.color().has_alpha() {
        let rgba = image.into_rgba8();
        GrayImage::from_fn(width, height, |x, y| {
            let [r, g, b, a] = rgba.get_pixel(x, y).0.map(u32::from);
            let luma = (77 * r + 150 * g + 29 * b) / 256;
            image::Luma([((luma * a + 255 * (255 - a)) / 255) as u8])
        })
    } else {
        image.into_luma8()
    };
    let mut links = Vec::new();
    decode(gray, &mut links);
    if links.is_empty() {
        Err(NO_QR.into())
    } else {
        Ok(links)
    }
}

pub fn from_screen() -> Result<Vec<String>, String> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|session| session.eq_ignore_ascii_case("wayland"))
    {
        return super::wayland::scan();
    }

    let screens = screenshots::Screen::all().map_err(|e| format!("无法枚举显示器：{e}"))?;
    if screens.is_empty() {
        return Err("未发现可截取的显示器".into());
    }
    let mut links = Vec::new();
    let mut failure = None;
    for screen in screens {
        let rgba = match screen.capture() {
            Ok(frame) => frame,
            Err(e) => {
                failure = Some(e.to_string());
                continue;
            }
        };
        let (width, height) = rgba.dimensions();
        let gray = GrayImage::from_fn(width, height, |x, y| {
            let [r, g, b, _] = rgba.get_pixel(x, y).0.map(u32::from);
            image::Luma([((77 * r + 150 * g + 29 * b) / 256) as u8])
        });
        drop(rgba);
        decode(gray, &mut links);
    }
    if !links.is_empty() {
        Ok(links)
    } else if let Some(error) = failure {
        Err(format!("未找到 TOTP 二维码，部分屏幕未能截取：{error}"))
    } else {
        Err(NO_QR.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_transparent_qr_and_deduplicates_repeated_links() {
        let image =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/platform/fixtures/totp-pair.png");
        assert_eq!(
            from_image(&image).unwrap(),
            ["otpauth://totp/Demo?secret=JBSWY3DPEHPK3PXP"]
        );
    }
}
