#![allow(dead_code)]
use ratatui::style::Color;

/// Nord Color Palette (Official Palette Tokens)
/// Polar Night (Dark background tones)
pub const NORD0: Color = Color::Rgb(46, 52, 64);   // Darkest background (#2E3440)
pub const NORD1: Color = Color::Rgb(59, 66, 82);   // Lighter background (#3B4252)
pub const NORD2: Color = Color::Rgb(67, 76, 94);   // Selection background (#434C5E)
pub const NORD3: Color = Color::Rgb(76, 86, 106);  // Comments/Muted text (#4C566A)

/// Snow Storm (Foreground/Text tones)
pub const NORD4: Color = Color::Rgb(216, 222, 233); // Main text (#D8DEE9)
pub const NORD5: Color = Color::Rgb(229, 233, 240); // Subtle bright text (#E5E9F0)
pub const NORD6: Color = Color::Rgb(236, 239, 244); // Brightest text (#ECEFF4)

/// Frost (Blue & Cyan Accents)
pub const NORD7: Color = Color::Rgb(143, 188, 187); // Teal/Greenish Cyan (#8FBCBB)
pub const NORD8: Color = Color::Rgb(136, 192, 208); // Ice Cyan / Primary Accent (#88C0D0)
pub const NORD9: Color = Color::Rgb(129, 161, 193); // Soft Blue (#81A1C1)
pub const NORD10: Color = Color::Rgb(94, 129, 172); // Deep Blue (#5E81AC)

/// Aurora (Accent Highlights)
pub const NORD11: Color = Color::Rgb(191, 97, 106); // Red / Errors (#BF616A)
pub const NORD12: Color = Color::Rgb(208, 135, 112); // Orange (#D08770)
pub const NORD13: Color = Color::Rgb(235, 203, 139); // Yellow / Warnings (#EBCB8B)
pub const NORD14: Color = Color::Rgb(163, 190, 140); // Green / Success (#A3BE8C)
pub const NORD15: Color = Color::Rgb(180, 142, 173); // Purple (#B48EAD)

/// Metadata Editor Custom Palette
pub const META_LIGHT_LILAC: Color = Color::Rgb(216, 191, 212); // #D8BFD4 Light Lilac
pub const META_MUTED_PURPLE: Color = Color::Rgb(157, 146, 189); // #9D92BD Muted Slate Purple
pub const META_ROSE_PINK: Color = Color::Rgb(194, 139, 159);   // #C28B9F Dusty Rose Pink
pub const META_DEEP_MAUVE: Color = Color::Rgb(143, 108, 137);  // #8F6C89 Deep Mauve
pub const META_SNOW_BRIGHT: Color = Color::Rgb(236, 239, 244); // #ECEFF4 Snow Storm Brightest
pub const META_SNOW_MID: Color = Color::Rgb(229, 233, 240);    // #E5E9F0 Snow Storm Subtle Bright
pub const META_SNOW_MAIN: Color = Color::Rgb(216, 222, 233);   // #D8DEE9 Snow Storm Main Text
pub const META_BG_ACTIVE: Color = Color::Rgb(63, 49, 61);      // Deep Mauve Polar Night tint

/// Linear interpolation between two RGB color tuples
pub fn lerp_rgb(c1: (u8, u8, u8), c2: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let r = (c1.0 as f32 + (c2.0 as f32 - c1.0 as f32) * t).round() as u8;
    let g = (c1.1 as f32 + (c2.1 as f32 - c1.1 as f32) * t).round() as u8;
    let b = (c1.2 as f32 + (c2.2 as f32 - c1.2 as f32) * t).round() as u8;
    Color::Rgb(r, g, b)
}

/// Dynamic breathing pulse glow for the playback border during active playback.
/// Pulses between deep mauve (#785F87) and glowing bright lilac (#E1C8F5).
pub fn get_breathing_playback_color(elapsed_secs: f32) -> Color {
    let sin_val = ((elapsed_secs * 2.5).sin() + 1.0) / 2.0;
    let low = (120, 95, 135);   // Deep Mauve
    let high = (225, 200, 245); // Glowing Bright Lilac
    lerp_rgb(low, high, sin_val)
}

