use alloc::vec::Vec;
use miniz_oxide::inflate::decompress_to_vec_zlib;

const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

#[derive(Clone, Copy)]
pub enum ColorType {
    Grayscale = 0,
    Rgb = 2,
    Indexed = 3,
    GrayscaleAlpha = 4,
    Rgba = 6,
}

pub struct PngImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

fn u32_be(data: &[u8], off: usize) -> u32 {
    if off + 4 > data.len() {
        return 0;
    }
    (data[off] as u32) << 24
        | (data[off + 1] as u32) << 16
        | (data[off + 2] as u32) << 8
        | data[off + 3] as u32
}

fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i16;
    let b = b as i16;
    let c = c as i16;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

pub fn decode_png(data: &[u8]) -> Option<PngImage> {
    if data.len() < 8 {
        return None;
    }
    if data[..8] != PNG_SIG {
        return None;
    }

    let mut pos = 8;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut color_type = ColorType::Rgba;
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut idat_chunks: Vec<Vec<u8>> = Vec::new();
    let mut has_ihdr = false;

    while pos + 8 <= data.len() {
        let chunk_len = u32_be(data, pos) as usize;
        let chunk_type = &data[pos + 4..pos + 8];
        let chunk_data_start = pos + 8;
        let chunk_data_end = chunk_data_start + chunk_len;
        if chunk_data_end > data.len() {
            return None;
        }

        let type_str = core::str::from_utf8(chunk_type).unwrap_or("");
        match type_str {
            "IHDR" => {
                if chunk_len < 13 {
                    return None;
                }
                width = u32_be(data, chunk_data_start);
                height = u32_be(data, chunk_data_start + 4);
                let bit_depth = data[chunk_data_start + 8];
                let ct = data[chunk_data_start + 9];
                if bit_depth != 8 {
                    return None;
                }
                color_type = match ct {
                    0 => ColorType::Grayscale,
                    2 => ColorType::Rgb,
                    3 => ColorType::Indexed,
                    4 => ColorType::GrayscaleAlpha,
                    6 => ColorType::Rgba,
                    _ => return None,
                };
                has_ihdr = true;
            }
            "PLTE" => {
                if !chunk_len.is_multiple_of(3) {
                    return None;
                }
                for i in 0..chunk_len / 3 {
                    let off = chunk_data_start + i * 3;
                    palette.push([data[off], data[off + 1], data[off + 2], 0xFF]);
                }
            }
            "tRNS" => {
                for i in 0..chunk_len.min(palette.len()) {
                    palette[i][3] = data[chunk_data_start + i];
                }
            }
            "IDAT" => {
                idat_chunks.push(data[chunk_data_start..chunk_data_end].to_vec());
            }
            "IEND" => {
                break;
            }
            _ => {}
        }

        pos = chunk_data_end + 4;
    }

    if !has_ihdr {
        return None;
    }
    if idat_chunks.is_empty() {
        return None;
    }

    let mut compressed = Vec::new();
    for chunk in &idat_chunks {
        compressed.extend_from_slice(chunk);
    }

    let raw = decompress_to_vec_zlib(&compressed).ok()?;

    let bytes_per_pixel = match color_type {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Indexed => 1,
        ColorType::Rgba => 4,
    };

    let row_len = 1 + width as usize * bytes_per_pixel;
    let expected = row_len * height as usize;
    if raw.len() < expected {
        return None;
    }

    let mut pixels = Vec::with_capacity((width * height) as usize);
    let mut prev_row: Vec<u8> = alloc::vec![0u8; width as usize * bytes_per_pixel];

    for y in 0..height as usize {
        let off = y * row_len;
        let filter = raw[off];
        let row = &raw[off + 1..off + row_len];

        let mut unfiltered = Vec::with_capacity(width as usize * bytes_per_pixel);

        for x in 0..(width as usize * bytes_per_pixel) {
            let a = if x >= bytes_per_pixel {
                unfiltered[x - bytes_per_pixel]
            } else {
                0
            };
            let b = prev_row[x];
            let c = if x >= bytes_per_pixel {
                prev_row[x - bytes_per_pixel]
            } else {
                0
            };

            let val = match filter {
                0 => row[x],
                1 => row[x].wrapping_add(a),
                2 => row[x].wrapping_add(b),
                3 => row[x].wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => row[x].wrapping_add(paeth_predictor(a, b, c)),
                _ => return None,
            };
            unfiltered.push(val);
        }

        // Convert to RGBA
        match color_type {
            ColorType::Rgba => {
                for x in 0..width as usize {
                    let off = x * 4;
                    let r = unfiltered[off];
                    let g = unfiltered[off + 1];
                    let b = unfiltered[off + 2];
                    let a = unfiltered[off + 3];
                    pixels.push((a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32);
                }
            }
            ColorType::Rgb => {
                for x in 0..width as usize {
                    let off = x * 3;
                    let r = unfiltered[off];
                    let g = unfiltered[off + 1];
                    let b = unfiltered[off + 2];
                    pixels.push(0xFF000000 | (r as u32) << 16 | (g as u32) << 8 | b as u32);
                }
            }
            ColorType::Indexed => {
                for &v in unfiltered.iter().take(width as usize) {
                    let idx = v as usize;
                    if idx < palette.len() {
                        let rgba = palette[idx];
                        pixels.push(
                            (rgba[3] as u32) << 24
                                | (rgba[0] as u32) << 16
                                | (rgba[1] as u32) << 8
                                | rgba[2] as u32,
                        );
                    } else {
                        pixels.push(0xFFFF00FF);
                    }
                }
            }
            ColorType::Grayscale => {
                for &g in unfiltered.iter().take(width as usize) {
                    pixels.push(0xFF000000 | (g as u32) << 16 | (g as u32) << 8 | g as u32);
                }
            }
            ColorType::GrayscaleAlpha => {
                for x in 0..width as usize {
                    let off = x * 2;
                    let g = unfiltered[off];
                    let a = unfiltered[off + 1];
                    pixels.push((a as u32) << 24 | (g as u32) << 16 | (g as u32) << 8 | g as u32);
                }
            }
        }

        prev_row = unfiltered;
    }

    Some(PngImage {
        width,
        height,
        pixels,
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Build a PNG chunk (length + type + data + CRC slot). The decoder skips
    /// does not verify it.
    fn chunk(typ: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&(data.len() as u32).to_be_bytes());
        c.extend_from_slice(typ);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0u8; 4]); // CRC slot; decoder skips it unverified
        c
    }

    fn ihdr(width: u32, height: u32, bit_depth: u8, color_type: u8) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&width.to_be_bytes());
        d.extend_from_slice(&height.to_be_bytes());
        d.push(bit_depth);
        d.push(color_type);
        d.extend_from_slice(&[0, 0, 0]); // compression, filter, interlace
        d
    }

    /// Assemble a full PNG: signature + IHDR + optional PLTE/tRNS + IDAT + IEND.
    fn png(
        width: u32,
        height: u32,
        bit_depth: u8,
        color_type: u8,
        raw_rows: &[u8],
        palette: Option<&[u8]>,
        trns: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut p = PNG_SIG.to_vec();
        p.extend_from_slice(&chunk(b"IHDR", &ihdr(width, height, bit_depth, color_type)));
        if let Some(plte) = palette {
            p.extend_from_slice(&chunk(b"PLTE", plte));
        }
        if let Some(t) = trns {
            p.extend_from_slice(&chunk(b"tRNS", t));
        }
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(raw_rows, 6);
        p.extend_from_slice(&chunk(b"IDAT", &compressed));
        p.extend_from_slice(&chunk(b"IEND", &[]));
        p
    }

    #[test]
    fn test_paeth_predictor() {
        // Reference vectors (the PNG spec's filter-type-4 predictor).
        assert_eq!(paeth_predictor(10, 20, 30), 10);
        assert_eq!(paeth_predictor(0, 255, 0), 255);
        assert_eq!(paeth_predictor(0, 0, 0), 0);
        assert_eq!(paeth_predictor(255, 255, 255), 255);
        assert_eq!(paeth_predictor(5, 5, 0), 5); // pa == pb -> left (a)
    }

    #[test]
    fn test_u32_be() {
        assert_eq!(u32_be(&[0x01, 0x02, 0x03, 0x04], 0), 0x01020304);
        assert_eq!(u32_be(&[0xFF; 8], 4), 0xFFFFFFFF);
        assert_eq!(u32_be(&[1, 2, 3], 0), 0); // out of bounds -> 0
    }

    #[test]
    fn test_decode_rgba() {
        // 2x2 RGBA, filter 0 on every row.
        let rows = [
            0, 10, 20, 30, 255, 40, 50, 60, 255, //
            0, 70, 80, 90, 255, 100, 110, 120, 255,
        ];
        let img = decode_png(&png(2, 2, 8, 6, &rows, None, None)).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixels.len(), 4);
        assert_eq!(img.pixels[0], 0xFF000000 | (10 << 16) | (20 << 8) | 30);
        assert_eq!(img.pixels[3], 0xFF000000 | (100 << 16) | (110 << 8) | 120);
    }

    #[test]
    fn test_decode_rgb() {
        let img = decode_png(&png(1, 1, 8, 2, &[0, 200, 100, 50], None, None)).unwrap();
        assert_eq!(img.pixels[0], 0xFF000000 | (200 << 16) | (100 << 8) | 50);
    }

    #[test]
    fn test_decode_grayscale() {
        let img = decode_png(&png(1, 1, 8, 0, &[0, 200], None, None)).unwrap();
        assert_eq!(img.pixels[0], 0xFF000000 | (200 << 16) | (200 << 8) | 200);
    }

    #[test]
    fn test_decode_grayscale_alpha() {
        let img = decode_png(&png(1, 1, 8, 4, &[0, 200, 128], None, None)).unwrap();
        assert_eq!(img.pixels[0], (128 << 24) | (200 << 16) | (200 << 8) | 200);
    }

    #[test]
    fn test_decode_indexed_with_palette_and_trns() {
        // 3-entry palette; tRNS overrides the alpha of entry 1.
        let plte = [255, 0, 0, 0, 255, 0, 0, 0, 255];
        let trns = [0xFF, 0x80, 0xFF];
        let img = decode_png(&png(1, 1, 8, 3, &[0, 1], Some(&plte), Some(&trns))).unwrap();
        assert_eq!(img.pixels[0], (0x80 << 24) | (0 << 16) | (255 << 8) | 0);
    }

    #[test]
    fn test_decode_paeth_filter_row() {
        // One 2-pixel RGBA row encoded with filter type 4. NOTE: this
        // decoder's paeth predictor is per-channel -- `a` is the same
        // channel of the previous pixel (unfiltered[x - bpp]), not the
        // adjacent byte -- so with a zero prev row pixel 0's channels all
        // predict 0 (its raw bytes ARE the pixel), and pixel 1's deltas are
        // against pixel 0's same-channel values. Pinned deliberately.
        let raw = [4, 10, 20, 30, 255, 30, 30, 30, 0];
        let img = decode_png(&png(2, 1, 8, 6, &raw, None, None)).unwrap();
        assert_eq!(img.pixels[0], 0xFF000000 | (10 << 16) | (20 << 8) | 30);
        assert_eq!(img.pixels[1], 0xFF000000 | (40 << 16) | (50 << 8) | 60);
    }

    #[test]
    fn test_decode_rejects_bad_signature() {
        assert!(decode_png(b"not a png").is_none());
        assert!(decode_png(b"").is_none());
        assert!(decode_png(&[0u8; 8]).is_none());
    }

    #[test]
    fn test_decode_rejects_truncated() {
        // IHDR claims data beyond the end of the file.
        let mut p = PNG_SIG.to_vec();
        p.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        p.extend_from_slice(&[0xFF; 8]); // partial IDAT chunk header
        assert!(decode_png(&p).is_none());
    }

    #[test]
    fn test_decode_rejects_missing_ihdr() {
        // Signature + IDAT + IEND with no IHDR chunk at all.
        let mut p = PNG_SIG.to_vec();
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&[], 6);
        p.extend_from_slice(&chunk(b"IDAT", &compressed));
        p.extend_from_slice(&chunk(b"IEND", &[]));
        assert!(decode_png(&p).is_none());
    }

    #[test]
    fn test_decode_rejects_bad_bit_depth() {
        assert!(decode_png(&png(1, 1, 16, 6, &[0, 0, 0, 0, 0, 0, 0, 0, 0], None, None)).is_none());
    }

    #[test]
    fn test_decode_rejects_bad_color_type() {
        assert!(decode_png(&png(1, 1, 8, 1, &[0, 0], None, None)).is_none());
    }

    #[test]
    fn test_decode_rejects_unknown_filter() {
        assert!(decode_png(&png(1, 1, 8, 6, &[5, 0, 0, 0, 0], None, None)).is_none());
    }

    #[test]
    fn test_decode_rejects_no_idat() {
        let mut p = PNG_SIG.to_vec();
        p.extend_from_slice(&chunk(b"IHDR", &ihdr(1, 1, 8, 6)));
        p.extend_from_slice(&chunk(b"IEND", &[]));
        assert!(decode_png(&p).is_none());
    }

    #[test]
    fn test_decode_rejects_garbage_idat() {
        let mut p = PNG_SIG.to_vec();
        p.extend_from_slice(&chunk(b"IHDR", &ihdr(1, 1, 8, 6)));
        p.extend_from_slice(&chunk(b"IDAT", b"this is not zlib"));
        p.extend_from_slice(&chunk(b"IEND", &[]));
        assert!(decode_png(&p).is_none());
    }
}
