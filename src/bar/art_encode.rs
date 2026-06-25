//! Encode a PNG into colored terminal art using braille for detail and blocks for the brand.

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgba};
use ratatui::style::Color;

pub(crate) const TEXT_ROW_FRAC: f32 = 0.62;

const DOT_BITS: [u32; 8] = [0x01, 0x02, 0x04, 0x40, 0x08, 0x10, 0x20, 0x80];
const DOT_ORDER: [usize; 8] = [0, 1, 2, 6, 3, 4, 5, 7];
const OFFSETS: [(u32, u32); 8] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (0, 3),
    (1, 0),
    (1, 1),
    (1, 2),
    (1, 3),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb {
    const BLACK: Self = Self { r: 0, g: 0, b: 0 };
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtCell {
    ch: char,
    fg: Rgb,
}

type ArtGrid = Vec<Vec<ArtCell>>;

#[derive(Clone, Debug)]
struct EncodeParams {
    lit_alpha_min: u8,
    lit_luminance_min: f32,
    brand_min_lit_dots: u8,
    brand_luminance_min: f32,
    brand_uniformity_delta: i16,
    text_region_y0: f32,
    text_region_y1: f32,
    text_region_x0: f32,
    text_region_x1: f32,
    text_row_frac: f32,
    brand_text: String,
    brand_text_color: Rgb,
}

impl Default for EncodeParams {
    fn default() -> Self {
        Self {
            lit_alpha_min: 32,
            lit_luminance_min: 24.0,
            brand_min_lit_dots: 7,
            brand_luminance_min: 160.0,
            brand_uniformity_delta: 24,
            text_region_y0: 0.54,
            text_region_y1: 0.68,
            text_region_x0: 0.12,
            text_region_x1: 0.88,
            text_row_frac: TEXT_ROW_FRAC,
            brand_text: "sessions".into(),
            brand_text_color: Rgb {
                r: 187,
                g: 247,
                b: 208,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtCellRatatui {
    pub ch: char,
    pub fg: Color,
}

pub type ArtGridRatatui = Vec<Vec<ArtCellRatatui>>;

/// Encode `img` into `cols`×`rows` terminal cells (braille + brand blocks + text overlay).
pub fn encode_mixed_art(img: &DynamicImage, cols: u16, rows: u16) -> ArtGridRatatui {
    let grid = encode_mixed_art_core(img, cols, rows, &EncodeParams::default());
    grid.iter()
        .map(|row| {
            row.iter()
                .map(|cell| ArtCellRatatui {
                    ch: cell.ch,
                    fg: rgb_to_color(cell.fg),
                })
                .collect()
        })
        .collect()
}

fn encode_mixed_art_core(
    img: &DynamicImage,
    cols: u16,
    rows: u16,
    params: &EncodeParams,
) -> ArtGrid {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let pixel_w = u32::from(cols) * 2;
    let pixel_h = u32::from(rows) * 4;
    let rgba = resize_cover(img, pixel_w, pixel_h).to_rgba8();

    let mut grid = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let mut line = Vec::with_capacity(cols as usize);
        for col in 0..cols {
            if cell_in_text_region(col, row, cols, rows, params) {
                line.push(ArtCell {
                    ch: ' ',
                    fg: Rgb::BLACK,
                });
            } else {
                line.push(encode_cell(&rgba, col, row, params));
            }
        }
        grid.push(line);
    }
    apply_brand_text(&mut grid, cols, rows, params);
    grid
}

fn resize_cover(img: &DynamicImage, target_w: u32, target_h: u32) -> DynamicImage {
    let (sw, sh) = img.dimensions();
    if sw == 0 || sh == 0 || target_w == 0 || target_h == 0 {
        return img.clone();
    }
    let scale = (target_w as f32 / sw as f32).max(target_h as f32 / sh as f32);
    let nw = (sw as f32 * scale).ceil().max(1.0) as u32;
    let nh = (sh as f32 * scale).ceil().max(1.0) as u32;
    let resized = img.resize_exact(nw, nh, FilterType::Lanczos3);
    let left = nw.saturating_sub(target_w) / 2;
    let top = nh.saturating_sub(target_h) / 2;
    resized.crop_imm(left, top, target_w.min(nw), target_h.min(nh))
}

fn encode_cell(rgba: &image::RgbaImage, col: u16, row: u16, params: &EncodeParams) -> ArtCell {
    let base_x = u32::from(col) * 2;
    let base_y = u32::from(row) * 4;
    let mut samples = [Rgba([0, 0, 0, 0]); 8];
    for (i, (dx, dy)) in OFFSETS.iter().enumerate() {
        samples[i] = *rgba.get_pixel(base_x + dx, base_y + dy);
    }

    let mut code = 0x2800u32;
    let mut lit = Vec::new();
    for (i, sample) in samples.iter().enumerate() {
        if is_lit(*sample, params) {
            code |= DOT_BITS[DOT_ORDER[i]];
            lit.push(*sample);
        }
    }

    if lit.is_empty() {
        return ArtCell {
            ch: ' ',
            fg: Rgb::BLACK,
        };
    }

    if is_solid_brand(&lit, params) {
        return ArtCell {
            ch: '█',
            fg: average_color(&lit),
        };
    }

    ArtCell {
        ch: char::from_u32(code).unwrap_or(' '),
        fg: average_color(&lit),
    }
}

fn cell_in_text_region(col: u16, row: u16, cols: u16, rows: u16, params: &EncodeParams) -> bool {
    let nx = (f32::from(col) + 0.5) / f32::from(cols);
    let ny = (f32::from(row) + 0.5) / f32::from(rows);
    ny >= params.text_region_y0
        && ny <= params.text_region_y1
        && nx >= params.text_region_x0
        && nx <= params.text_region_x1
}

fn apply_brand_text(grid: &mut ArtGrid, cols: u16, rows: u16, params: &EncodeParams) {
    let text_row = (f32::from(rows) * params.text_row_frac).round() as usize;
    let Some(line) = grid.get_mut(text_row) else {
        return;
    };
    let chars: Vec<char> = params.brand_text.chars().collect();
    let len = chars.len();
    if len == 0 || len > cols as usize {
        return;
    }
    let start = (usize::from(cols).saturating_sub(len)) / 2;
    for cell in line.iter_mut() {
        *cell = ArtCell {
            ch: ' ',
            fg: Rgb::BLACK,
        };
    }
    for (i, ch) in chars.iter().enumerate() {
        if let Some(cell) = line.get_mut(start + i) {
            *cell = ArtCell {
                ch: *ch,
                fg: params.brand_text_color,
            };
        }
    }
}

fn is_lit(px: Rgba<u8>, params: &EncodeParams) -> bool {
    if px[3] < params.lit_alpha_min {
        return false;
    }
    luminance(px) >= params.lit_luminance_min
}

fn is_solid_brand(lit: &[Rgba<u8>], params: &EncodeParams) -> bool {
    if lit.len() < params.brand_min_lit_dots as usize {
        return false;
    }
    let bright = mean_luminance(lit);
    bright >= params.brand_luminance_min && is_uniform(lit, params.brand_uniformity_delta)
}

fn mean_luminance(samples: &[Rgba<u8>]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(|px| luminance(*px)).sum::<f32>() / samples.len() as f32
}

fn luminance(px: Rgba<u8>) -> f32 {
    0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32
}

fn average_color(samples: &[Rgba<u8>]) -> Rgb {
    let mut r = 0u32;
    let mut g = 0u32;
    let mut b = 0u32;
    let mut weight = 0u32;
    for px in samples {
        if px[3] < 32 {
            continue;
        }
        let w = u32::from(px[3]);
        r += u32::from(px[0]) * w;
        g += u32::from(px[1]) * w;
        b += u32::from(px[2]) * w;
        weight += w;
    }
    if weight == 0 {
        return Rgb::BLACK;
    }
    Rgb {
        r: (r / weight).min(255) as u8,
        g: (g / weight).min(255) as u8,
        b: (b / weight).min(255) as u8,
    }
}

fn is_uniform(samples: &[Rgba<u8>], delta: i16) -> bool {
    if samples.len() < 2 {
        return true;
    }
    let first = samples[0];
    samples.iter().skip(1).all(|px| {
        (i16::from(px[0]) - i16::from(first[0])).abs() <= delta
            && (i16::from(px[1]) - i16::from(first[1])).abs() <= delta
            && (i16::from(px[2]) - i16::from(first[2])).abs() <= delta
    })
}

fn rgb_to_color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba};

    #[test]
    fn braille_maps_two_by_four_pixels() {
        let mut img = image::RgbaImage::new(2, 4);
        for p in img.pixels_mut() {
            *p = Rgba([0, 0, 0, 0]);
        }
        *img.get_pixel_mut(0, 0) = Rgba([200, 200, 200, 255]);
        *img.get_pixel_mut(1, 3) = Rgba([200, 200, 200, 255]);
        let grid = encode_mixed_art(&DynamicImage::ImageRgba8(img), 1, 1);
        assert_eq!(grid.len(), 1);
        assert_eq!(grid[0].len(), 1);
        assert_ne!(grid[0][0].ch, ' ');
        assert!(grid[0][0].ch as u32 >= 0x2800);
    }

    #[test]
    fn sparse_dots_use_braille_not_blocks() {
        let mut img = image::RgbaImage::new(2, 4);
        for p in img.pixels_mut() {
            *p = Rgba([0, 0, 0, 0]);
        }
        *img.get_pixel_mut(0, 0) = Rgba([220, 220, 220, 255]);
        let grid = encode_mixed_art(&DynamicImage::ImageRgba8(img), 1, 1);
        assert_ne!(grid[0][0].ch, '█');
        assert!(grid[0][0].ch as u32 >= 0x2800);
    }

    #[test]
    fn encode_matches_target_dimensions() {
        let img = DynamicImage::new_rgba8(20, 20);
        let grid = encode_mixed_art(&img, 10, 5);
        assert_eq!(grid.len(), 5);
        assert!(grid.iter().all(|row| row.len() == 10));
    }

    #[test]
    fn solid_fill_uses_full_block() {
        let mut img = image::RgbaImage::new(2, 4);
        for p in img.pixels_mut() {
            *p = Rgba([240, 240, 240, 255]);
        }
        let grid = encode_mixed_art(&DynamicImage::ImageRgba8(img), 1, 1);
        assert_eq!(grid[0][0].ch, '█');
    }

    #[test]
    fn brand_text_is_centered() {
        let img = DynamicImage::new_rgba8(40, 40);
        let grid = encode_mixed_art(&img, 40, 20);
        let text_row = (20.0_f32 * TEXT_ROW_FRAC).round() as usize;
        let line: String = grid[text_row].iter().map(|c| c.ch).collect();
        assert!(line.contains("sessions"));
        let start = line.find("sessions").expect("brand text");
        let end = start + "sessions".len();
        assert_eq!(start, line.len().saturating_sub(end));
    }
}
