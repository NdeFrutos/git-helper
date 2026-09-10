use gpui::Rgba;

pub const BACKGROUND_COLOR: Rgba = color(0x0018_181B);
pub const BORDER_COLOR: Rgba = color(0x003F_3F46);
pub const ELEVATED_BACKGROUND_COLOR: Rgba = color(0x0020_2024);
pub const INPUT_BACKGROUND_COLOR: Rgba = color(0x0011_1113);
pub const HOVER_BACKGROUND_COLOR: Rgba = color(0x002A_2A2F);
pub const SELECTED_BACKGROUND_COLOR: Rgba = color(0x0030_303A);
pub const ACCENT_COLOR: Rgba = color(0x0060_A5FA);
pub const SUCCESS_COLOR: Rgba = color(0x004A_DE80);
pub const WARNING_COLOR: Rgba = color(0x00FB_BF24);
pub const ERROR_COLOR: Rgba = color(0x00F8_7171);
pub const MUTED_TEXT_COLOR: Rgba = color(0x00A1_A1AA);
pub const PRIMARY_TEXT_COLOR: Rgba = color(0x00F4_F4F5);

#[allow(
    clippy::cast_precision_loss,
    reason = "Cada componente está limitado a u8 y su conversión a f32 es exacta"
)]
const fn color(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}
