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

