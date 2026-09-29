//! BMP (非圧縮 24 / 32 bit) を 400×96 の RGB565 背景へ「画面いっぱいに拡大縮小 + 中央を切り出し」で読む
//!
//! SD の読み出しは遅い (GPIO SPI) ので、ファイル全体を持たずに **行ごと・少しずつ** 流し込める形にしてある:
//!
//! 1. 先頭 [`HEADER_LEN`] バイトを [`BmpInfo::parse`] に渡す。
//! 2. [`Resampler::new`] が切り出し範囲 (縦横比を保って 400×96 を覆う最小の拡大率、中央) を決める。
//! 3. [`Resampler::next_row`] が次に読むべき行の (ファイル内の位置, バイト数) を返すので、その範囲を
//!    何回かに分けて読み、[`Resampler::push`] に順に渡す。1 行終わるたびに [`Resampler::end_row`]。
//!    行はファイルの並び順 (下から上の BMP なら画像の下の行から) に進むので、SD のシークは前向きだけ。
//! 4. 縮小は面積平均 (1 画素あたり最大 16×16 標本、それ以上は間引き)、拡大は最近傍。
//!    出力は Bayer 4×4 のディザ付きで RGB565 にする ([`color::rgb_dithered`])。
//!
//! ファームウェア (`ticker` のスライドショー) もシミュレータも同じこのコードで読む。

use super::color::{self, Color};
use super::{HEIGHT, WIDTH};

/// [`BmpInfo::parse`] に渡す先頭のバイト数 (ファイルヘッダ 14 + BITMAPINFOHEADER 40 + 色マスク 16)
pub const HEADER_LEN: usize = 70;
/// 受け付ける最大の幅 / 高さ (px)
pub const MAX_SIDE: u32 = 8192;

/// 1 画素のバイト配置
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 24 bit、B G R
    Bgr24,
    /// 32 bit、B G R X (BI_RGB、または BI_BITFIELDS で標準のマスク)
    Bgrx32,
}

impl PixelFormat {
    pub const fn bytes(self) -> u32 {
        match self {
            PixelFormat::Bgr24 => 3,
            PixelFormat::Bgrx32 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BmpInfo {
    pub width: u32,
    pub height: u32,
    /// true なら上の行から格納 (高さが負の BMP)
    pub top_down: bool,
    pub format: PixelFormat,
    /// 画素データの開始位置
    pub pixel_offset: u32,
    /// 1 行のバイト数 (4 の倍数に詰め物込み)
    pub row_bytes: u32,
}

#[inline]
fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

#[inline]
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl BmpInfo {
    /// 先頭 (最低 54 バイト、32 bit の BI_BITFIELDS なら 70 バイト) とファイル長から解釈する
    pub fn parse(header: &[u8], file_len: u32) -> Result<BmpInfo, &'static str> {
        if header.len() < 54 || &header[0..2] != b"BM" {
            return Err("not a BMP");
        }
        let pixel_offset = u32_at(header, 10);
        let dib_size = u32_at(header, 14);
        let width = u32_at(header, 18) as i32;
        let signed_height = u32_at(header, 22) as i32;
        let planes = u16_at(header, 26);
        let bpp = u16_at(header, 28);
        let compression = u32_at(header, 30);
        if dib_size < 40 || planes != 1 || width <= 0 || signed_height == 0 || signed_height == i32::MIN {
            return Err("bad BMP header");
        }
        let format = match (bpp, compression) {
            (24, 0) => PixelFormat::Bgr24,
            (32, 0) => PixelFormat::Bgrx32,
            (32, 3) => {
                // BI_BITFIELDS: マスクは BITMAPINFOHEADER の直後 (または V4/V5 ヘッダの中の同じ位置)
                if header.len() < 66 {
                    return Err("BMP masks missing");
                }
                let (r, g, b) = (u32_at(header, 54), u32_at(header, 58), u32_at(header, 62));
                if (r, g, b) != (0x00ff_0000, 0x0000_ff00, 0x0000_00ff) {
                    return Err("BMP masks unsupported");
                }
                PixelFormat::Bgrx32
            }
            _ => return Err("BMP: 24/32 bit only"),
        };
        let width = width as u32;
        let height = signed_height.unsigned_abs();
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err("BMP too large");
        }
        let row_bytes = (width * format.bytes() + 3) & !3;
        let end = row_bytes as u64 * height as u64 + pixel_offset as u64;
        if pixel_offset < 14 + dib_size || end > file_len as u64 {
            return Err("BMP truncated");
        }
        Ok(BmpInfo {
            width,
            height,
            top_down: signed_height < 0,
            format,
            pixel_offset,
            row_bytes,
        })
    }

    /// 画像の行 `row` (0 が上) のファイル内の位置
    pub fn row_offset(&self, row: u32) -> u32 {
        let file_row = if self.top_down { row } else { self.height - 1 - row };
        self.pixel_offset + file_row * self.row_bytes
    }
}

/// 1 軸の対応 (切り出した元画像の `crop` 画素 → 画面の `dst` 画素)
#[derive(Clone, Copy, Debug)]
struct Axis {
    /// 切り出しの開始 (元画像の座標)
    start: u32,
    /// 切り出しの長さ
    crop: u32,
    /// 出力の長さ
    dst: u32,
    /// 縮小時の間引き (1 なら全部の画素を使う)
    step: u32,
}

impl Axis {
    fn new(start: u32, crop: u32, dst: u32) -> Self {
        // 1 出力画素に入る標本を 1 軸 16 個までに抑える (u16 の累積があふれない: 16×16×255 < 65536)
        let step = if crop > dst * 15 { crop.div_ceil(dst * 15) } else { 1 };
        Self { start, crop, dst, step }
    }

    /// 切り出し内の位置 `i` (0..crop) が寄与する出力の範囲 [lo, hi)。間引きで使わない画素は空
    #[inline]
    fn range(&self, i: u32) -> (u32, u32) {
        if self.crop >= self.dst {
            if !i.is_multiple_of(self.step) {
                return (0, 0);
            }
            let d = (i as u64 * self.dst as u64 / self.crop as u64) as u32;
            (d, d + 1)
        } else {
            // 拡大: 出力 d は元の floor(d * crop / dst) を使う
            let lo = (i as u64 * self.dst as u64).div_ceil(self.crop as u64) as u32;
            let hi = ((i as u64 + 1) * self.dst as u64).div_ceil(self.crop as u64) as u32;
            (lo, hi.min(self.dst))
        }
    }
}

/// 読み込みの途中経過 (行単位)
pub struct Resampler {
    info: BmpInfo,
    x: Axis,
    y: Axis,
    /// 次に読む行 (ファイルの並び順で数えた番号、0..crop_h)
    next: u32,
    /// 今の行で次に `push` される画素 (切り出し内の x)
    col: u32,
    /// 1 画素の途中で分割されたときの残り
    partial: [u8; 4],
    partial_len: u8,
    acc: [[u16; 3]; WIDTH],
    count: [u16; WIDTH],
}

impl Resampler {
    pub fn new(info: BmpInfo) -> Self {
        let (w, h) = (info.width, info.height);
        let (dw, dh) = (WIDTH as u32, HEIGHT as u32);
        // 画面を覆う最小の拡大率 → 切り出す元画像の範囲 (中央)
        let (crop_w, crop_h) = if w as u64 * dh as u64 >= h as u64 * dw as u64 {
            // 横長: 高さ全部、幅を切る
            (((h as u64 * dw as u64 + dh as u64 / 2) / dh as u64).clamp(1, w as u64) as u32, h)
        } else {
            (w, ((w as u64 * dh as u64 + dw as u64 / 2) / dw as u64).clamp(1, h as u64) as u32)
        };
        Self {
            info,
            x: Axis::new((w - crop_w) / 2, crop_w, dw),
            y: Axis::new((h - crop_h) / 2, crop_h, dh),
            next: 0,
            col: 0,
            partial: [0; 4],
            partial_len: 0,
            acc: [[0; 3]; WIDTH],
            count: [0; WIDTH],
        }
    }

    pub fn info(&self) -> &BmpInfo {
        &self.info
    }

    /// 切り出し範囲 (x, y, w, h)。ログ / シミュレータの表示用
    pub fn crop(&self) -> (u32, u32, u32, u32) {
        (self.x.start, self.y.start, self.x.crop, self.y.crop)
    }

    /// 読み込みの進み具合 (0..=256)
    pub fn progress(&self) -> u32 {
        self.next * 256 / self.y.crop.max(1)
    }

    /// ファイルの並び順で `n` 番目に読む切り出し内の行 (0 が切り出しの上端)
    fn crop_row(&self, n: u32) -> u32 {
        if self.info.top_down { n } else { self.y.crop - 1 - n }
    }

    /// 次に読む行の (ファイル内の位置, バイト数)。全部読み終えたら None。
    /// 間引きで使わない行は飛ばす。
    pub fn next_row(&mut self) -> Option<(u32, u32)> {
        while self.next < self.y.crop {
            let row = self.crop_row(self.next);
            if self.y.range(row).0 < self.y.range(row).1 {
                let offset = self.info.row_offset(self.y.start + row) + self.x.start * self.info.format.bytes();
                return Some((offset, self.x.crop * self.info.format.bytes()));
            }
            self.next += 1;
        }
        None
    }

    /// 今の行の続きのバイト列 (`next_row` の範囲を先頭から順に、何回に分けてもよい)
    pub fn push(&mut self, mut bytes: &[u8]) {
        let bpp = self.info.format.bytes() as usize;
        // 前回の途中の画素を埋める
        if self.partial_len > 0 {
            let need = bpp - self.partial_len as usize;
            let take = need.min(bytes.len());
            self.partial[self.partial_len as usize..self.partial_len as usize + take].copy_from_slice(&bytes[..take]);
            self.partial_len += take as u8;
            bytes = &bytes[take..];
            if (self.partial_len as usize) < bpp {
                return;
            }
            let p = self.partial;
            self.pixel(p[2], p[1], p[0]);
            self.partial_len = 0;
        }
        let mut chunks = bytes.chunks_exact(bpp);
        for px in &mut chunks {
            self.pixel(px[2], px[1], px[0]);
        }
        let rest = chunks.remainder();
        self.partial[..rest.len()].copy_from_slice(rest);
        self.partial_len = rest.len() as u8;
    }

    #[inline]
    fn pixel(&mut self, r: u8, g: u8, b: u8) {
        let (lo, hi) = self.x.range(self.col);
        for d in lo..hi {
            let a = &mut self.acc[d as usize];
            a[0] += u16::from(r);
            a[1] += u16::from(g);
            a[2] += u16::from(b);
            self.count[d as usize] += 1;
        }
        self.col += 1;
    }

    /// 1 行を読み終えた。出力行が確定したら `dst` (400×96 RGB565) へ書く
    pub fn end_row(&mut self, dst: &mut [Color]) {
        let row = self.crop_row(self.next);
        let range = self.y.range(row);
        self.next += 1;
        self.col = 0;
        self.partial_len = 0;
        // 次に読む行が別の出力行に入るなら、ここで確定する
        let next_range = self.peek_next_range();
        if next_range != Some(range) {
            self.flush(range, dst);
        }
    }

    fn peek_next_range(&self) -> Option<(u32, u32)> {
        let mut n = self.next;
        while n < self.y.crop {
            let r = self.y.range(self.crop_row(n));
            if r.0 < r.1 {
                return Some(r);
            }
            n += 1;
        }
        None
    }

    fn flush(&mut self, (lo, hi): (u32, u32), dst: &mut [Color]) {
        for y in lo..hi {
            let base = y as usize * WIDTH;
            for x in 0..WIDTH {
                let n = self.count[x].max(1);
                let [r, g, b] = self.acc[x];
                let (r, g, b) = ((r / n) as u8, (g / n) as u8, (b / n) as u8);
                dst[base + x] = color::rgb_dithered(r, g, b, x, y as usize);
            }
        }
        self.acc = [[0; 3]; WIDTH];
        self.count = [0; WIDTH];
    }

    /// 全部読み終えたか
    pub fn done(&self) -> bool {
        self.next >= self.y.crop
    }
}
