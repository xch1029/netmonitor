use tauri::image::Image;

const WIDTH: usize = 32;
const HEIGHT: usize = 32;
const SCALE: usize = 2;
const FONT_WIDTH: usize = 3;
const FONT_HEIGHT: usize = 5;

const DIGITS: [[u8; FONT_HEIGHT]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

const LETTERS: [([u8; FONT_HEIGHT], char); 3] = [
    ([0b111, 0b101, 0b111, 0b101, 0b101], 'M'),
    ([0b110, 0b101, 0b110, 0b101, 0b110], 'K'),
    ([0b111, 0b101, 0b111, 0b101, 0b111], 'G'),
];

pub fn format_speed(bps: u64) -> String {
    if bps >= 1_000_000_000 {
        format!("{:.1} Gbps", bps as f64 / 1_000_000_000.0)
    } else if bps >= 1_000_000 {
        format!("{:.1} Mbps", bps as f64 / 1_000_000.0)
    } else if bps >= 1_000 {
        format!("{:.1} Kbps", bps as f64 / 1_000.0)
    } else {
        format!("{bps} bps")
    }
}

fn compact_speed(bps: u64) -> String {
    if bps >= 1_000_000_000 {
        format!("{:.0}G", bps as f64 / 1_000_000_000.0)
    } else if bps >= 100_000_000 {
        format!("{:.0}M", bps as f64 / 1_000_000.0)
    } else if bps >= 1_000_000 {
        format!("{:.1}M", bps as f64 / 1_000_000.0)
    } else if bps >= 100_000 {
        format!("{:.0}K", bps as f64 / 1_000.0)
    } else if bps >= 1_000 {
        format!("{:.1}K", bps as f64 / 1_000.0)
    } else {
        format!("{bps}")
    }
}

pub fn render_summary_icon(down_bps: u64, up_bps: u64) -> Image<'static> {
    let mut rgba = vec![0u8; WIDTH * HEIGHT * 4];

    fill_rect(&mut rgba, 0, 0, WIDTH, HEIGHT, [16, 19, 28, 245]);
    fill_rect(&mut rgba, 0, 0, WIDTH, HEIGHT / 2, [32, 87, 165, 255]);
    fill_rect(&mut rgba, 0, HEIGHT / 2, WIDTH, HEIGHT / 2, [169, 92, 38, 255]);

    draw_text(&mut rgba, 2, 3, &compact_speed(down_bps), [255, 255, 255, 255]);
    draw_text(&mut rgba, 2, 17, &compact_speed(up_bps), [255, 255, 255, 255]);

    Image::new_owned(rgba, WIDTH as u32, HEIGHT as u32)
}

fn fill_rect(rgba: &mut [u8], x: usize, y: usize, w: usize, h: usize, color: [u8; 4]) {
    for py in y..(y + h).min(HEIGHT) {
        for px in x..(x + w).min(WIDTH) {
            let offset = (py * WIDTH + px) * 4;
            rgba[offset..offset + 4].copy_from_slice(&color);
        }
    }
}

fn draw_text(rgba: &mut [u8], x: usize, y: usize, text: &str, color: [u8; 4]) {
    let mut cursor = x;
    for character in text.chars() {
        draw_glyph(rgba, cursor, y, character, color);
        cursor += (FONT_WIDTH + 1) * SCALE;
    }
}

fn draw_glyph(rgba: &mut [u8], x: usize, y: usize, character: char, color: [u8; 4]) {
    let glyph = glyph_for(character);
    for (row, bits) in glyph.iter().enumerate() {
        for column in 0..FONT_WIDTH {
            let mask = 1 << (FONT_WIDTH - 1 - column);
            if bits & mask != 0 {
                for scale_y in 0..SCALE {
                    for scale_x in 0..SCALE {
                        let px = x + column * SCALE + scale_x;
                        let py = y + row * SCALE + scale_y;
                        if px < WIDTH && py < HEIGHT {
                            let offset = (py * WIDTH + px) * 4;
                            rgba[offset..offset + 4].copy_from_slice(&color);
                        }
                    }
                }
            }
        }
    }
}

fn glyph_for(character: char) -> [u8; FONT_HEIGHT] {
    match character {
        '0'..='9' => DIGITS[character as usize - '0' as usize],
        '.' => [0b000, 0b000, 0b000, 0b010, 0b010],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        other => LETTERS
            .iter()
            .find_map(|(glyph, supported)| (*supported == other).then_some(*glyph))
            .unwrap_or([0b111, 0b101, 0b010, 0b000, 0b010]),
    }
}
