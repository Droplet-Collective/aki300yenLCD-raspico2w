//! 見本の背景 BMP を手続き生成する (写真ではない。著作権の心配が無い)。
//! BMP の読み込み (`ui::bmp`) の各経路を通るよう、大きさ / 形式を変えてある:
//!
//! | ファイル | 大きさ | 形式 | 読み込みの経路 |
//! |---|---|---|---|
//! | `SUNSET.BMP` | 400×96 | 24 bit、下から上 | そのまま (拡大縮小なし) |
//! | `CLOUDS.BMP` | 301×100 | 24 bit、上から下、行に詰め物 | 拡大 + 上下を切る |
//! | `BOKEH.BMP` | 640×200 | 32 bit、下から上 | 縮小 (面積平均) + 左右を切る |

use std::path::Path;

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1) ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    (h & 0xffff) as f32 / 65535.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// 値ノイズ (0..1)
fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (smooth(x - xi as f32), smooth(y - yi as f32));
    let a = hash(xi, yi, seed);
    let b = hash(xi + 1, yi, seed);
    let c = hash(xi, yi + 1, seed);
    let d = hash(xi + 1, yi + 1, seed);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    top + (bottom - top) * fy
}

fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let (mut v, mut amp, mut f) = (0.0, 0.5, 1.0);
    for o in 0..5 {
        v += amp * noise(x * f, y * f, seed + o);
        amp *= 0.5;
        f *= 2.0;
    }
    v
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn sunset(x: u32, y: u32, w: u32, h: u32) -> [f32; 3] {
    let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
    let horizon = 0.62;
    let sky = |v: f32| {
        let t = v / horizon;
        if t < 0.5 {
            mix([30.0, 40.0, 110.0], [190.0, 90.0, 120.0], t * 2.0)
        } else {
            mix([190.0, 90.0, 120.0], [255.0, 170.0, 80.0], (t - 0.5) * 2.0)
        }
    };
    let (sx, sy) = (0.68, horizon - 0.08);
    let sun_d = (((u - sx) * 4.17).powi(2) + (v - sy).powi(2)).sqrt();
    let mut c = sky(v.min(horizon));
    // 山並み (2 層)
    let ridge1 = horizon - 0.10 - 0.12 * fbm(u * 3.0, 0.5, 7);
    let ridge2 = horizon - 0.02 - 0.06 * fbm(u * 6.0, 1.5, 11);
    if v < horizon {
        let glow = (1.0 - sun_d * 3.0).max(0.0);
        c = mix(c, [255.0, 230.0, 170.0], glow * 0.8);
        if sun_d < 0.07 {
            c = [255.0, 244.0, 214.0];
        }
        if v > ridge1 {
            c = mix(c, [70.0, 40.0, 80.0], 0.85);
        }
        if v > ridge2 {
            c = mix(c, [40.0, 24.0, 52.0], 0.95);
        }
    } else {
        // 水面: 空の反射 + 揺らぎ
        let mv = horizon - (v - horizon) * 0.8;
        let mut r = sky(mv);
        let ripple = fbm(u * 40.0, v * 120.0, 3);
        let sun_col = (1.0 - ((u - sx) * 4.17).abs() * 6.0).max(0.0) * (ripple - 0.35).max(0.0) * 2.0;
        r = mix(r, [255.0, 220.0, 150.0], sun_col);
        c = mix(r, [20.0, 20.0, 50.0], 0.35 + (v - horizon) * 0.8);
    }
    c
}

fn clouds(x: u32, y: u32, w: u32, h: u32) -> [f32; 3] {
    let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
    let mut c = mix([110.0, 170.0, 240.0], [200.0, 228.0, 255.0], v);
    let n = fbm(u * 6.0, v * 3.0, 21);
    let cloud = ((n - 0.45) * 3.0).clamp(0.0, 1.0);
    c = mix(c, [252.0, 252.0, 255.0], cloud);
    let shade = ((n - 0.62) * 4.0).clamp(0.0, 1.0);
    c = mix(c, [210.0, 216.0, 232.0], shade * 0.5);
    // 丘
    let hill = 0.78 - 0.1 * fbm(u * 2.5, 3.0, 5);
    if v > hill {
        let g = fbm(u * 30.0, v * 30.0, 9);
        c = mix([90.0, 180.0, 70.0], [150.0, 220.0, 100.0], g);
    }
    c
}

fn bokeh(x: u32, y: u32, w: u32, h: u32) -> [f32; 3] {
    let (fx, fy) = (x as f32, y as f32);
    let mut c = mix([10.0, 12.0, 30.0], [40.0, 20.0, 50.0], fy / h as f32);
    let colors = [[255.0, 180.0, 90.0], [255.0, 110.0, 150.0], [120.0, 200.0, 255.0], [255.0, 230.0, 150.0]];
    for i in 0..60 {
        let cx = hash(i, 1, 99) * w as f32;
        let cy = (0.2 + 0.7 * hash(i, 2, 99)) * h as f32;
        let r = 8.0 + 26.0 * hash(i, 3, 99);
        let col = colors[(hash(i, 4, 99) * 4.0) as usize % 4];
        let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
        if d < r {
            let edge = if d > r - 2.0 { 1.0 } else { 0.55 };
            c = mix(c, col, 0.45 * edge);
        }
    }
    c
}

fn write_bmp(path: &Path, w: u32, h: u32, bpp: u16, top_down: bool, f: fn(u32, u32, u32, u32) -> [f32; 3]) -> Result<(), String> {
    let bytes = u32::from(bpp / 8);
    let row = (w * bytes + 3) & !3;
    let size = 54 + row * h;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(if top_down { -(h as i32) } else { h as i32 }).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&bpp.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(row * h).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for i in 0..h {
        let y = if top_down { i } else { h - 1 - i };
        let start = out.len();
        for x in 0..w {
            let [r, g, b] = f(x, y, w, h);
            out.push(b.clamp(0.0, 255.0) as u8);
            out.push(g.clamp(0.0, 255.0) as u8);
            out.push(r.clamp(0.0, 255.0) as u8);
            if bytes == 4 {
                out.push(0);
            }
        }
        while out.len() - start < row as usize {
            out.push(0);
        }
    }
    std::fs::write(path, &out).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("wrote {} ({}x{} {} bit {})", path.display(), w, h, bpp, if top_down { "top-down" } else { "bottom-up" });
    Ok(())
}

pub fn write_all(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    write_bmp(&dir.join("SUNSET.BMP"), 400, 96, 24, false, sunset)?;
    write_bmp(&dir.join("CLOUDS.BMP"), 301, 100, 24, true, clouds)?;
    write_bmp(&dir.join("BOKEH.BMP"), 640, 200, 32, false, bokeh)?;
    Ok(())
}
