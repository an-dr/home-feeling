//! Colour maths for the settings page's picker: `#rrggbb` text, and hue,
//! saturation, and brightness for the sliders.

/// The colours offered as one-click swatches.
pub const PRESETS: [&str; 12] = [
    "#00bcd4", "#2196f3", "#3f51b5", "#9c27b0", "#e91e63", "#f44336", "#ff9800", "#ffb300",
    "#4caf50", "#7f8c8d", "#ffffff", "#000000",
];

pub fn parse(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Hue in degrees (0 to 360), saturation and brightness from 0 to 1.
pub fn to_hsv([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

pub fn from_hsv(hue: f32, saturation: f32, value: f32) -> [u8; 3] {
    let (s, v) = (saturation.clamp(0.0, 1.0), value.clamp(0.0, 1.0));
    let sector = (hue.rem_euclid(360.0)) / 60.0;
    let chroma = v * s;
    let x = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = v - chroma;
    let byte = |c: f32| ((c + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        assert_eq!(parse("#00bcd4"), Some([0, 188, 212]));
        assert_eq!(hex([0, 188, 212]), "#00bcd4");
        assert_eq!(parse("00bcd4"), None);
        assert_eq!(parse("#00bcd"), None);
    }

    #[test]
    fn hsv_round_trips_for_the_presets() {
        for preset in super::PRESETS {
            let rgb = parse(preset).unwrap();
            let (h, s, v) = to_hsv(rgb);
            assert_eq!(from_hsv(h, s, v), rgb, "{preset}");
        }
    }

    #[test]
    fn primary_hues_land_where_expected() {
        assert_eq!(from_hsv(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(from_hsv(120.0, 1.0, 1.0), [0, 255, 0]);
        assert_eq!(from_hsv(240.0, 1.0, 1.0), [0, 0, 255]);
    }
}
