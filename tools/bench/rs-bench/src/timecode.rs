//! 时间码 RSTC1。
//!
//! 本模块实现 TC1 时间码，与页面端
//! `tools/bench/page/tc.js` 使用同一份规格。

/// 视口的水平单位数。
pub const GRID_W: u32 = 64;
/// 视口的垂直单位数。
pub const GRID_H: u32 = 36;
/// 静区（黑）的单位矩形：`x ∈ [2, 12)`，`y ∈ [14, 22)`。
pub const QUIET_UNITS: [u32; 4] = [2, 14, 12, 22];
/// 数据区的第一个单位列。
pub const DATA_X0: u32 = 3;
/// 数据区的第一个单位行。
pub const DATA_Y0: u32 = 15;
/// 数据区的列数。
pub const DATA_COLS: u32 = 8;
/// 数据区的行数。
pub const DATA_ROWS: u32 = 6;
/// 时间码的位数。
pub const DATA_BITS: usize = 48;
/// RSTC1 的白化掩码。
pub const WHITEN_MASK: u64 = 0xA5C3_5A3C_96E1;
/// payload 的位数。
pub const PAYLOAD_BITS: u32 = 36;
/// payload 的掩码。
pub const PAYLOAD_MASK: u64 = (1 << PAYLOAD_BITS) - 1;
/// CALIB 的宽、高上限（不含）。
pub const DIM_LIMIT: u32 = 1 << 18;

/// 一个 RSTC1 时间码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    /// Host Unix 毫秒，按 2^36 取模。
    Time(u64),
    /// 页面画布的设备像素尺寸。
    Calib { width: u32, height: u32 },
    /// 页面时间线结束标记。
    End,
}

impl Code {
    /// 将 Unix 毫秒按 RSTC1 的 36 位 payload 取模。
    pub fn time_from_unix_ms(ms: u64) -> Code {
        Code::Time(ms & PAYLOAD_MASK)
    }

    /// 返回 4 位类型编号：TIME=1、CALIB=2、END=3。
    pub fn type_id(&self) -> u8 {
        match self {
            Code::Time(_) => 1,
            Code::Calib { .. } => 2,
            Code::End => 3,
        }
    }

    /// 返回未编码的 36 位 payload。
    pub fn payload(&self) -> u64 {
        match self {
            Code::Time(value) => value & PAYLOAD_MASK,
            Code::Calib { width, height } => ((*width as u64) << 18) | (*height as u64),
            Code::End => 0,
        }
    }
}

/// 计算 CRC-8（poly 0x07，init 0，不反射，xorout 0）。
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// 编码为白化后的 48 位字。
pub fn encode(code: Code) -> u64 {
    if let Code::Calib { width, height } = code {
        assert!(
            width < DIM_LIMIT && height < DIM_LIMIT,
            "CALIB dimensions must be less than 2^18"
        );
    }

    let v40 = ((code.type_id() as u64) << PAYLOAD_BITS) | (code.payload() & PAYLOAD_MASK);
    let bytes = v40.to_be_bytes();
    let crc = crc8(&bytes[3..8]);
    let word = (v40 << 8) | crc as u64;
    word ^ WHITEN_MASK
}

/// 解码白化后的 48 位字；非法字返回 `None`。
pub fn decode_word(whitened: u64) -> Option<Code> {
    if whitened >> DATA_BITS != 0 {
        return None;
    }

    let word = whitened ^ WHITEN_MASK;
    let v40 = word >> 8;
    let received_crc = (word & 0xff) as u8;
    let bytes = v40.to_be_bytes();
    if crc8(&bytes[3..8]) != received_crc {
        return None;
    }

    let type_id = ((v40 >> PAYLOAD_BITS) & 0x0f) as u8;
    let payload = v40 & PAYLOAD_MASK;
    match type_id {
        1 => Some(Code::Time(payload)),
        2 => {
            let width = (payload >> 18) as u32;
            let height = (payload & ((1 << 18) - 1)) as u32;
            if width == 0 || height == 0 {
                None
            } else {
                Some(Code::Calib { width, height })
            }
        }
        3 if payload == 0 => Some(Code::End),
        _ => None,
    }
}

/// 将白化字按行优先顺序展开为 48 个布尔位。
pub fn word_bits(whitened: u64) -> [bool; DATA_BITS] {
    let mut bits = [false; DATA_BITS];
    for (index, bit) in bits.iter_mut().enumerate() {
        *bit = ((whitened >> (47 - index)) & 1) != 0;
    }
    bits
}

/// 将行优先的 48 个布尔位重新组合成白化字。
pub fn bits_to_word(bits: &[bool; DATA_BITS]) -> u64 {
    let mut word = 0u64;
    for (index, &bit) in bits.iter().enumerate() {
        if bit {
            word |= 1u64 << (47 - index);
        }
    }
    word
}

/// 像素半开区间矩形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PxRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl PxRect {
    /// 矩形宽度。
    pub fn width(&self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    /// 矩形高度。
    pub fn height(&self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }
}

/// 将水平单位边界换算为像素边界。
pub fn unit_x(k: u32, width: u32) -> u32 {
    ((k as f64 * width as f64 / GRID_W as f64) + 0.5).floor() as u32
}

/// 将垂直单位边界换算为像素边界。
pub fn unit_y(k: u32, height: u32) -> u32 {
    ((k as f64 * height as f64 / GRID_H as f64) + 0.5).floor() as u32
}

/// 返回静区的像素矩形。
pub fn quiet_rect(width: u32, height: u32) -> PxRect {
    PxRect {
        x0: unit_x(QUIET_UNITS[0], width),
        y0: unit_y(QUIET_UNITS[1], height),
        x1: unit_x(QUIET_UNITS[2], width),
        y1: unit_y(QUIET_UNITS[3], height),
    }
}

/// 返回一个数据格的像素矩形。
pub fn cell_rect(col: u32, row: u32, width: u32, height: u32) -> PxRect {
    assert!(col < DATA_COLS, "data column out of range");
    assert!(row < DATA_ROWS, "data row out of range");
    PxRect {
        x0: unit_x(DATA_X0 + col, width),
        y0: unit_y(DATA_Y0 + row, height),
        x1: unit_x(DATA_X0 + col + 1, width),
        y1: unit_y(DATA_Y0 + row + 1, height),
    }
}

/// 返回数据格中心的归一化坐标。
pub fn cell_center_norm(col: u32, row: u32) -> (f64, f64) {
    assert!(col < DATA_COLS, "data column out of range");
    assert!(row < DATA_ROWS, "data row out of range");
    (
        (DATA_X0 as f64 + col as f64 + 0.5) / GRID_W as f64,
        (DATA_Y0 as f64 + row as f64 + 0.5) / GRID_H as f64,
    )
}

/// 返回静区四角探针的归一化坐标。
pub fn quiet_probes_norm() -> [(f64, f64); 4] {
    [
        (2.5 / GRID_W as f64, 14.5 / GRID_H as f64),
        (11.5 / GRID_W as f64, 14.5 / GRID_H as f64),
        (2.5 / GRID_W as f64, 21.5 / GRID_H as f64),
        (11.5 / GRID_W as f64, 21.5 / GRID_H as f64),
    ]
}

/// 行紧排的 8 位灰度图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl GrayImage {
    /// 创建一张以 `fill` 填充的图像。
    pub fn new(width: u32, height: u32, fill: u8) -> Self {
        let width_usize = width as usize;
        let height_usize = height as usize;
        let len = match width_usize.checked_mul(height_usize) {
            Some(len) => len,
            None => panic!("image dimensions overflow addressable memory"),
        };
        Self {
            width,
            height,
            data: vec![fill; len],
        }
    }

    /// 从行紧排数据创建图像。
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Result<Self, String> {
        let expected = match (width as usize).checked_mul(height as usize) {
            Some(len) => len,
            None => return Err("image dimensions overflow addressable memory".to_owned()),
        };
        if data.len() != expected {
            return Err(format!(
                "raw image length mismatch: expected {expected} bytes, got {}",
                data.len()
            ));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    fn index(&self, x: u32, y: u32) -> usize {
        assert!(x < self.width, "image x coordinate out of bounds");
        assert!(y < self.height, "image y coordinate out of bounds");
        (y as usize) * (self.width as usize) + x as usize
    }

    /// 读取一个像素，越界时 panic。
    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.data[self.index(x, y)]
    }

    /// 写入一个像素，越界时 panic。
    pub fn set(&mut self, x: u32, y: u32, v: u8) {
        let index = self.index(x, y);
        self.data[index] = v;
    }

    /// 填充矩形与图像相交的部分。
    pub fn fill_rect(&mut self, r: PxRect, v: u8) {
        let x0 = r.x0.min(self.width);
        let mut x1 = r.x1.min(self.width);
        let y0 = r.y0.min(self.height);
        let mut y1 = r.y1.min(self.height);
        if x1 < x0 {
            x1 = x0;
        }
        if y1 < y0 {
            y1 = y0;
        }
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let width = self.width as usize;
        for y in y0..y1 {
            let start = (y as usize) * width + x0 as usize;
            let end = (y as usize) * width + x1 as usize;
            self.data[start..end].fill(v);
        }
    }

    /// 裁剪矩形到图像范围并复制出新图像。
    pub fn crop(&self, r: PxRect) -> GrayImage {
        let x0 = r.x0.min(self.width);
        let mut x1 = r.x1.min(self.width);
        let y0 = r.y0.min(self.height);
        let mut y1 = r.y1.min(self.height);
        if x1 < x0 {
            x1 = x0;
        }
        if y1 < y0 {
            y1 = y0;
        }
        let out_width = x1 - x0;
        let out_height = y1 - y0;
        let mut data = Vec::with_capacity((out_width as usize) * (out_height as usize));
        let source_width = self.width as usize;
        for y in y0..y1 {
            let start = (y as usize) * source_width + x0 as usize;
            let end = (y as usize) * source_width + x1 as usize;
            data.extend_from_slice(&self.data[start..end]);
        }
        GrayImage {
            width: out_width,
            height: out_height,
            data,
        }
    }

    /// 编码为 PGM P5。
    pub fn to_pgm(&self) -> Vec<u8> {
        let header = format!("P5\n{} {}\n255\n", self.width, self.height);
        let mut bytes = Vec::with_capacity(header.len() + self.data.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&self.data);
        bytes
    }

    /// 从 PGM P5 读取灰度图。
    pub fn from_pgm(bytes: &[u8]) -> Result<GrayImage, String> {
        let mut cursor = 0usize;
        let magic = next_pgm_token(bytes, &mut cursor)?;
        if magic != b"P5" {
            return Err("unsupported PGM format: expected P5".to_owned());
        }
        let width_token = next_pgm_token(bytes, &mut cursor)?;
        let width = parse_pgm_u32(width_token, "width")?;
        let height_token = next_pgm_token(bytes, &mut cursor)?;
        let height = parse_pgm_u32(height_token, "height")?;
        let maxval_token = next_pgm_token(bytes, &mut cursor)?;
        if parse_pgm_u32(maxval_token, "maxval")? != 255 {
            return Err("unsupported PGM maxval: expected 255".to_owned());
        }

        let separator = bytes.get(cursor).copied().ok_or_else(|| {
            "invalid PGM header: maxval must be followed by one whitespace byte".to_owned()
        })?;
        if !is_pgm_whitespace(separator) {
            return Err(
                "invalid PGM header: maxval must be followed by one whitespace byte".to_owned(),
            );
        }
        cursor += 1;

        let expected = match (width as usize).checked_mul(height as usize) {
            Some(len) => len,
            None => return Err("PGM dimensions overflow addressable memory".to_owned()),
        };
        if bytes.len().saturating_sub(cursor) < expected {
            return Err(format!(
                "PGM data is too short: expected {expected} bytes, got {}",
                bytes.len().saturating_sub(cursor)
            ));
        }
        Self::from_raw(width, height, bytes[cursor..cursor + expected].to_vec())
    }
}

fn is_pgm_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn next_pgm_token<'a>(bytes: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], String> {
    loop {
        while *cursor < bytes.len() && is_pgm_whitespace(bytes[*cursor]) {
            *cursor += 1;
        }
        if *cursor < bytes.len() && bytes[*cursor] == b'#' {
            while *cursor < bytes.len() && bytes[*cursor] != b'\n' {
                *cursor += 1;
            }
            continue;
        }
        break;
    }
    let start = *cursor;
    while *cursor < bytes.len() && !is_pgm_whitespace(bytes[*cursor]) && bytes[*cursor] != b'#' {
        *cursor += 1;
    }
    if start == *cursor {
        return Err("invalid PGM header: missing token".to_owned());
    }
    Ok(&bytes[start..*cursor])
}

fn parse_pgm_u32(token: &[u8], name: &str) -> Result<u32, String> {
    if token.is_empty() {
        return Err(format!("invalid PGM {name}: empty value"));
    }
    let mut value = 0u32;
    for &byte in token {
        if !byte.is_ascii_digit() {
            return Err(format!("invalid PGM {name}: expected decimal integer"));
        }
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add((byte - b'0') as u32))
            .ok_or_else(|| format!("invalid PGM {name}: value is too large"))?;
    }
    Ok(value)
}

/// 将时间码绘制到图像中。
pub fn render_code(img: &mut GrayImage, code: Code) {
    let quiet = quiet_rect(img.width, img.height);
    img.fill_rect(quiet, 0);
    let bits = word_bits(encode(code));
    for (index, &bit) in bits.iter().enumerate() {
        let col = (index as u32) % DATA_COLS;
        let row = (index as u32) / DATA_COLS;
        let cell = cell_rect(col, row, img.width, img.height);
        img.fill_rect(cell, if bit { 255 } else { 0 });
    }
}

/// 创建标定页并绘制 CALIB 时间码。
pub fn render_calib(width: u32, height: u32, white: bool) -> GrayImage {
    let mut image = GrayImage::new(width, height, if white { 255 } else { 0 });
    render_code(&mut image, Code::Calib { width, height });
    image
}
