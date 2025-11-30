use std::{
    io::{Read, Write},
    marker::PhantomData,
    os::unix::fs::FileExt,
};

const MAGIC_BYTES: [u8; 4] = [0x54, 0x49, 0x4C, 0x45]; // "TILE"

pub struct PowerOfTwo<T>(T);

macro_rules! impl_power_of_two {
    ($ty: ty) => {
        impl PowerOfTwo<$ty> {
            pub fn ilog2(&self) -> u32 {
                self.0.ilog2()
            }
        }

        impl TryFrom<$ty> for PowerOfTwo<$ty> {
            type Error = ();

            fn try_from(value: $ty) -> Result<Self, Self::Error> {
                match value.is_power_of_two() {
                    true => Ok(Self(value)),
                    false => Err(()),
                }
            }
        }

        impl std::ops::Deref for PowerOfTwo<$ty> {
            type Target = $ty;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}
impl_power_of_two!(u8);
impl_power_of_two!(u16);
impl_power_of_two!(u32);
impl_power_of_two!(u64);
impl_power_of_two!(usize);

#[repr(u32)]
#[derive(Debug, Copy, Clone)]
pub enum PixelFormat {
    SY8,
    Y8,
    RGB8,
    SRGB8,
    Y16,
    RGB16,
    Y32,
    RGB32,
}

impl PixelFormat {
    // Returns the total number of bytes that a single texel in the given format uses.
    pub const fn texel_bytes(&self) -> usize {
        match self {
            PixelFormat::SY8 | PixelFormat::Y8 => 1,
            PixelFormat::Y16 => 2,
            PixelFormat::RGB8 | PixelFormat::SRGB8 => 3,
            PixelFormat::Y32 => 4,
            PixelFormat::RGB16 => 6,
            PixelFormat::RGB32 => 12,
        }
    }

    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => PixelFormat::SY8,
            1 => PixelFormat::Y8,
            2 => PixelFormat::RGB8,
            3 => PixelFormat::SRGB8,
            4 => PixelFormat::Y16,
            5 => PixelFormat::RGB16,
            6 => PixelFormat::Y32,
            7 => PixelFormat::RGB32,
            _ => panic!("Invalid pixel format"),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Luma<T>(T);
#[derive(Debug, PartialEq)]
pub struct Rgb<T>([T; 3]);
#[derive(Debug, PartialEq)]
pub struct Rgba<T>([T; 4]);

pub trait Texel {
    const STRIDE: usize;
}
impl<T> Texel for Luma<T> {
    const STRIDE: usize = size_of::<T>();
}
impl<T> Texel for Rgb<T> {
    const STRIDE: usize = size_of::<T>() * 3;
}
impl<T> Texel for Rgba<T> {
    const STRIDE: usize = size_of::<T>() * 4;
}

pub enum WrapMode {
    Clamp,
    Repeat,
    Mirror,
}

#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
pub struct Extent {
    width: u32,
    height: u32,
}

impl Extent {
    pub fn new(width: u32, height: u32) -> Self {
        Extent { width, height }
    }
}

// Within a tile, texels are laid out in scanline order.
#[derive(Debug, PartialEq)]
pub struct Tile<'a> {
    scanlines: Vec<&'a [u8]>,
}

// An iterator over a byte slice in (non-overlapping) chunks, starting at the beginning of the slice.
pub struct Tiles<'a> {
    // source data descriptors
    data: &'a [u8],
    width: u32,
    height: u32,
    scanline_stride: usize,
    // tile descriptors
    tile_size: u32,
    texel_stride: usize,
    // iterator state
    x_begin: u32,
    y_begin: u32,
}

impl<'a> Tiles<'a> {
    pub fn new<T: Texel>(data: &'a [u8], width: u32, height: u32, tile_size: u32) -> Self {
        Self {
            data,
            width,
            height,
            scanline_stride: width as usize * T::STRIDE,
            tile_size,
            texel_stride: T::STRIDE,
            x_begin: 0,
            y_begin: 0,
        }
    }
}

// Within a level, tiles are laid out left-to-right, top-to-bottom.
impl<'a> Iterator for Tiles<'a> {
    type Item = Tile<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.x_begin >= self.width {
            self.x_begin = 0;
            self.y_begin += self.tile_size;
        }
        if self.y_begin >= self.height {
            return None;
        }
        let scanlines = (0..self.tile_size)
            .filter_map(|scanline| {
                let y = self.y_begin + scanline;
                if y >= self.height {
                    None
                } else {
                    let start = y as usize * self.scanline_stride
                        + self.x_begin as usize * self.texel_stride;
                    let end = start
                        + self.tile_size.min(self.width - self.x_begin) as usize
                            * self.texel_stride;
                    Some(&self.data[start..end])
                }
            })
            .collect::<Vec<_>>();
        self.x_begin += self.tile_size;
        Some(Tile { scanlines })
    }
}

#[inline(always)]
fn write_u32_le<W: Write>(w: &mut W, value: u32) -> Result<(), std::io::Error> {
    w.write_all(&value.to_le_bytes())
}

#[inline(always)]
fn write_u64_le<W: Write>(w: &mut W, value: u64) -> Result<(), std::io::Error> {
    w.write_all(&value.to_le_bytes())
}

#[inline(always)]
fn read_u32_le<R: Read>(r: &mut R) -> Result<u32, std::io::Error> {
    let mut bytes = [0; 4];
    r.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

#[inline(always)]
fn read_u64_le<R: Read>(r: &mut R) -> Result<u64, std::io::Error> {
    let mut bytes = [0; 8];
    r.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

#[repr(C)]
#[derive(Debug)]
struct Header {
    magic: [u8; 4],
    version: u32,
    log_tile_size: u32, // log2(tile_size)
    format: u32,        // an enum for pixel format
    first_in_memory_level: u32,
    level_resolution: Vec<Extent>,
    level_offset: Vec<u64>,
}

impl Header {
    fn byte_size(levels: usize) -> usize {
        size_of::<[u8; 4]>() // magic
        + size_of::<u32>() * 5 // fixed fields
        + size_of::<Extent>() * levels // variable fields
        + size_of::<u64>() * levels // additional field for level offset
    }

    fn write<W: Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        // magic (raw bytes)
        writer.write_all(&self.magic)?;

        // fixed fields
        write_u32_le(writer, self.version)?;
        write_u32_le(writer, self.log_tile_size)?;
        write_u32_le(writer, self.format)?;
        write_u32_le(writer, self.first_in_memory_level)?;
        write_u32_le(writer, self.level_resolution.len() as u32)?; // mip count

        // variable fields
        for extent in &self.level_resolution {
            write_u32_le(writer, extent.width)?;
            write_u32_le(writer, extent.height)?;
        }
        for offset in &self.level_offset {
            write_u64_le(writer, *offset as u64)?;
        }

        Ok(())
    }

    fn read<R: Read>(reader: &mut R) -> Result<Self, std::io::Error> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;

        let version = read_u32_le(reader)?;
        let log_tile_size = read_u32_le(reader)?;
        let format = read_u32_le(reader)?;
        let first_in_memory_level = read_u32_le(reader)?;
        let mip_count = read_u32_le(reader)?;

        // read Extent array
        let mut level_resolution = Vec::with_capacity(mip_count as usize);
        for _ in 0..mip_count {
            let width = read_u32_le(reader)?;
            let height = read_u32_le(reader)?;
            level_resolution.push(Extent::new(width, height));
        }
        let mut level_offset = Vec::with_capacity(mip_count as usize);
        for _ in 0..mip_count {
            level_offset.push(read_u64_le(reader)?);
        }

        Ok(Self {
            magic,
            version,
            log_tile_size,
            format,
            first_in_memory_level,
            level_resolution,
            level_offset,
        })
    }
}

pub struct TiledImage {
    file: std::fs::File,
    pixel_format: PixelFormat,
    log_tile_size: u32,
    first_in_memory_level: u32,
    level_resolution: Vec<Extent>,
    level_offset: Vec<usize>,
    in_memory_levels: Vec<Box<[u8]>>,
}

impl TiledImage {
    const TILE_DISK_ALIGNMENT: usize = 4096;
    const TOP_LEVELS_BYTES: usize = 3800;

    pub fn open<P: AsRef<std::path::Path>>(path: P) -> Result<Self, std::io::Error> {
        let mut file = std::fs::File::open(path.as_ref())?;
        let header = Header::read(&mut file)?;
        let pixel_format = PixelFormat::from_u32(header.format);
        let mip_levels = header.level_offset.len();
        let first_in_memory_level = header.first_in_memory_level;
        let level_resolution = header.level_resolution;
        let mut in_memory_levels = Vec::with_capacity(mip_levels);
        for level in first_in_memory_level as usize..mip_levels {
            let Extent { width, height } = level_resolution[level];
            let offset = header.level_offset[level];
            let mut buf = vec![0u8; (width * height) as usize * pixel_format.texel_bytes()];
            file.read_exact_at(&mut buf, offset)?;
            in_memory_levels.push(buf.into_boxed_slice());
        }
        assert!(in_memory_levels.len() == mip_levels - first_in_memory_level as usize);
        Ok(Self {
            file,
            pixel_format,
            log_tile_size: header.log_tile_size,
            first_in_memory_level,
            level_resolution,
            level_offset: header
                .level_offset
                .into_iter()
                .map(|o| o as usize)
                .collect(),
            in_memory_levels,
        })
    }

    // Takes an image pyramid, rearranges the texels into tiles with width and height equal to the given tile_size, and writes them to disk.
    pub fn create<W: Write>(
        mut levels: Vec<Box<[u8]>>,
        level_resolution: Vec<Extent>,
        format: PixelFormat,
        tile_size: PowerOfTwo<u32>,
        writer: &mut W,
    ) -> Result<(), std::io::Error> {
        assert!(!levels.is_empty());
        assert!(levels.len() == level_resolution.len());

        let mut acc = 0;
        let mut first_in_memory_level = 0;
        levels.reverse();
        for (level, data) in levels.iter().enumerate() {
            acc += data.len();
            if acc > Self::TOP_LEVELS_BYTES {
                first_in_memory_level = level as u32;
                break;
            }
        }

        let mip_levels = levels.len();
        let header_offset = Header::byte_size(mip_levels) as u64;
        // eprintln!("Header offset: {}", header_offset);
        let mut level_offset = vec![0; mip_levels];
        let mut offset = header_offset;
        for (i, data) in levels.iter().enumerate() {
            let level = mip_levels - i - 1;
            level_offset[level] = offset;
            offset += data.len() as u64;
            // eprintln!("Level {} offset: {}", level, level_offset[level]);
        }

        let in_memory_levels = levels.split_off(first_in_memory_level as usize);
        let in_header_levels = &levels[0..first_in_memory_level as usize];

        let header = Header {
            magic: MAGIC_BYTES,
            version: 0,
            log_tile_size: tile_size.ilog2(),
            format: format as u32,
            first_in_memory_level,
            level_resolution: level_resolution.clone(),
            level_offset,
        };

        // Write header
        header.write(writer)?;

        // Write in-header levels
        for data in in_header_levels {
            writer.write(data)?;
        }

        // Write tiled data
        for (i, data) in in_memory_levels.iter().enumerate() {
            let level = first_in_memory_level as usize - i;
            let Extent { width, height } = level_resolution[level];
            // eprintln!(
            //     "writing tiled data for level {}, in {}x{}",
            //     level, width, height
            // );
            let tiles = match format {
                PixelFormat::Y8 => Tiles::new::<Luma<u8>>(&data, width, height, *tile_size as u32),
                PixelFormat::RGB8 => Tiles::new::<Rgb<u8>>(&data, width, height, *tile_size as u32),
                _ => todo!(),
            };
            for tile in tiles {
                for scanline in tile.scanlines {
                    writer.write(scanline)?;
                    // pad the tile
                    let padding = *tile_size as usize * format.texel_bytes() - scanline.len();
                    if padding != 0 {
                        writer.write(&vec![0u8; padding])?;
                    }
                }
            }
        }

        Ok(())
    }

    pub fn create_with_mips<W: Write>(
        base: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        tile_size: PowerOfTwo<u32>,
        wrap_mode: WrapMode,
        writer: &mut W,
    ) -> Result<(), std::io::Error> {
        let pixel_size = format.texel_bytes();
        assert!(base.len() == (width * height) as usize * pixel_size);

        let mut levels: Vec<Box<[u8]>> = Vec::new();
        let mut level_resolution = Vec::new();
        levels.push(base.into());
        level_resolution.push(Extent { width, height });

        let mut curr_w = width;
        let mut curr_h = height;

        while curr_w > 1 || curr_h > 1 {
            let next_w = (curr_w.max(1) + 1) / 2;
            let next_h = (curr_h.max(1) + 1) / 2;

            let prev = &levels[levels.len() - 1];
            let mut next = vec![0u8; (next_w * next_h) as usize * pixel_size];

            let fetch_texel = |mut x: u32, mut y: u32| -> &[u8] {
                match wrap_mode {
                    WrapMode::Clamp => {
                        x = x.min(curr_w - 1);
                        y = y.min(curr_h - 1);
                    }
                    WrapMode::Repeat => {
                        x %= curr_w;
                        y %= curr_h;
                    }
                    WrapMode::Mirror => {
                        x %= curr_w * 2;
                        y %= curr_h * 2;
                        if x >= curr_w {
                            x = curr_w - (x % curr_w) - 1;
                        }
                        if y >= curr_h {
                            y = curr_h - (y % curr_h) - 1;
                        }
                    }
                }

                let index = (y * curr_w + x) as usize * pixel_size;
                &prev[index..index + pixel_size]
            };

            for y in 0..next_h {
                for x in 0..next_w {
                    // Sample four texels
                    let samples = [
                        fetch_texel(x * 2, y * 2),
                        fetch_texel(x * 2 + 1, y * 2),
                        fetch_texel(x * 2, y * 2 + 1),
                        fetch_texel(x * 2 + 1, y * 2 + 1),
                    ];

                    // Default to box filter
                    let dst_index = (y * next_w + x) as usize * pixel_size;
                    for c in 0..pixel_size {
                        let mut s = 0u32;
                        for i in 0..4 {
                            s += samples[i][c] as u32;
                        }
                        next[dst_index + c] = (s / 4) as u8;
                    }
                }
            }

            levels.push(next.into_boxed_slice());
            level_resolution.push(Extent {
                width: next_w,
                height: next_h,
            });

            curr_w = next_w;
            curr_h = next_h;
        }
        Self::create(levels, level_resolution, format, tile_size, writer)
    }

    // Returns the total number of bytes needed to store all of the texels in a single tile of the texture.
    pub fn tile_bytes(&self) -> usize {
        (1 << self.log_tile_size) * (1 << self.log_tile_size) * self.pixel_format.texel_bytes()
    }

    // Returns the number of bytes that each texture tile uses on disk, accounting for memory alignment.
    pub fn tile_disk_bytes(&self) -> usize {
        (self.tile_bytes() + Self::TILE_DISK_ALIGNMENT - 1) & !(Self::TILE_DISK_ALIGNMENT - 1)
    }

    // Given a pixel coordinate (x, y), return the coordinates for the tile containing the pixel.
    pub fn tile_index(&self, x: u32, y: u32) -> (u32, u32) {
        let tile_x = x >> self.log_tile_size;
        let tile_y = y >> self.log_tile_size;
        (tile_x, tile_y)
    }

    // Given a pixel coordinate (x, y), return the offset of the corresponding tile.
    pub fn tile_offset(&self, x: u32, y: u32) -> usize {
        let tile_mask = (1 << self.log_tile_size) - 1;
        let tilep = (x & tile_mask, y & tile_mask);
        let tile_width = 1 << self.log_tile_size;
        self.pixel_format.texel_bytes() * (tilep.1 * tile_width + tilep.0) as usize
    }

    // Returns the offset in the file where the tile at coordinates (x, y) at a given level starts.
    pub fn file_offset(&self, x: u32, y: u32, level: u32) -> usize {
        let tile_width = 1 << self.log_tile_size;
        let x_tiles =
            (self.level_resolution[level as usize].width + tile_width - 1) / self.log_tile_size;
        self.level_offset[level as usize] + self.tile_disk_bytes() * (y * x_tiles + x) as usize
    }

    pub fn get_texel(&self, x: u32, y: u32, level: u32) -> Option<&[u8]> {
        if level < self.first_in_memory_level {
            return None;
        }
        let level_start = &self.in_memory_levels[(level - self.first_in_memory_level) as usize];
        let width = self.level_resolution[level as usize].width;
        let start = (y * width + x) as usize * self.pixel_format.texel_bytes();
        let end = start + self.pixel_format.texel_bytes();
        Some(&level_start[start..end])
    }

    // to be removed
    pub fn get_level(&self, level: u32) -> Option<Vec<u8>> {
        let Extent { width, height } = self.level_resolution[level as usize];
        let mut buf = vec![0u8; (width * height) as usize * self.pixel_format.texel_bytes()];
        let offset = self.level_offset[level as usize];
        self.file.read_exact_at(&mut buf, offset as u64).ok()?;
        Some(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Seek;

    #[test]
    fn test_tile_iterator() {
        #[rustfmt::skip]
        let data: [u8; 16] = [
            0, 1, 2, 3,
            4, 5, 6, 7,
            8, 9, 10, 11,
            12, 13, 14, 15
        ];
        let mut iter = Tiles::new::<Luma<u8>>(&data, 4, 4, 2);
        assert_eq!(
            Some(Tile {
                scanlines: vec![&[0, 1], &[4, 5]],
            }),
            iter.next()
        );
        assert_eq!(
            Some(Tile {
                scanlines: vec![&[2, 3], &[6, 7]],
            }),
            iter.next()
        );
        assert_eq!(
            Some(Tile {
                scanlines: vec![&[8, 9], &[12, 13]],
            }),
            iter.next()
        );
        assert_eq!(
            Some(Tile {
                scanlines: vec![&[10, 11], &[14, 15]],
            }),
            iter.next()
        );
        assert_eq!(None, iter.next());

        let mut iter = Tiles::new::<Luma<u8>>(&data, 4, 4, 3);
        assert_eq!(
            Some(Tile {
                scanlines: vec![&[0, 1, 2], &[4, 5, 6], &[8, 9, 10]],
            }),
            iter.next()
        );
        assert_eq!(
            Some(Tile {
                scanlines: vec![&[3], &[7], &[11]],
            }),
            iter.next()
        );
        assert_eq!(
            Some(Tile {
                scanlines: vec![&[12, 13, 14]],
            }),
            iter.next()
        );
        assert_eq!(
            Some(Tile {
                scanlines: vec![&[15]],
            }),
            iter.next()
        );
        assert_eq!(None, iter.next());
    }

    #[test]
    fn test_header_roundtrip() {
        let mut temp = tempfile::tempfile().expect("Failed to create temporary file");
        {
            let header = Header {
                magic: MAGIC_BYTES,
                version: 1,
                log_tile_size: 32,
                format: PixelFormat::RGB8 as u32,
                first_in_memory_level: 4,
                level_resolution: vec![
                    Extent::new(1024, 768),
                    Extent::new(512, 384),
                    Extent::new(256, 192),
                    Extent::new(128, 96),
                    Extent::new(64, 48),
                    Extent::new(32, 24),
                    Extent::new(16, 12),
                    Extent::new(8, 6),
                    Extent::new(4, 3),
                    Extent::new(2, 1),
                    Extent::new(1, 1),
                ],
                level_offset: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            };
            header.write(&mut temp).expect("Failed to write header");
        }
        temp.rewind().expect("Failed to rewind");
        {
            let header = Header::read(&mut temp).expect("Failed to read header");
            assert_eq!(header.magic, MAGIC_BYTES);
            assert_eq!(header.version, 1);
            assert_eq!(header.log_tile_size, 32);
            assert_eq!(header.format, PixelFormat::RGB8 as u32);
            assert_eq!(
                header.level_resolution,
                vec![
                    Extent::new(1024, 768),
                    Extent::new(512, 384),
                    Extent::new(256, 192),
                    Extent::new(128, 96),
                    Extent::new(64, 48),
                    Extent::new(32, 24),
                    Extent::new(16, 12),
                    Extent::new(8, 6),
                    Extent::new(4, 3),
                    Extent::new(2, 1),
                    Extent::new(1, 1),
                ]
            );
            assert_eq!(header.level_offset, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        }
    }
}
