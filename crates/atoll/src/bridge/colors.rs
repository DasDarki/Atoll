use atoll_core::IslandColor;
use slint::Color;

/// The palette for island colours, matching `Theme.island-colors` in the user interface.
pub fn island_color(color: IslandColor) -> Color {
    let rgb = match color {
        IslandColor::Violet => 0x8b5cf6,
        IslandColor::Blue => 0x3b82f6,
        IslandColor::Cyan => 0x06b6d4,
        IslandColor::Teal => 0x14b8a6,
        IslandColor::Green => 0x22c55e,
        IslandColor::Amber => 0xf59e0b,
        IslandColor::Rose => 0xf43f5e,
        IslandColor::Slate => 0x64748b,
    };
    Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}
