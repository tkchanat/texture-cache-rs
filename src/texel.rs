#[repr(u32)]
#[derive(Debug, Copy, Clone)]
pub enum PixelFormat {
    Luma8,
    Luma16,
    Luma32,
    Rgb8,
    Rgb16,
    Rgb32,
    Rgba8,
    Rgba16,
    Rgba32,
}

impl PixelFormat {
    // Returns the total number of bytes that a single texel in the given format uses.
    pub const fn texel_bytes(&self) -> usize {
        match self {
            PixelFormat::Luma8 => 1,
            PixelFormat::Luma16 => 2,
            PixelFormat::Rgb8 => 3,
            PixelFormat::Luma32 | PixelFormat::Rgba8 => 4,
            PixelFormat::Rgb16 => 6,
            PixelFormat::Rgba16 => 8,
            PixelFormat::Rgb32 => 12,
            PixelFormat::Rgba32 => 16,
        }
    }

    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => PixelFormat::Luma8,
            1 => PixelFormat::Luma16,
            2 => PixelFormat::Luma32,
            3 => PixelFormat::Rgb8,
            4 => PixelFormat::Rgb16,
            5 => PixelFormat::Rgb32,
            6 => PixelFormat::Rgba8,
            7 => PixelFormat::Rgba16,
            8 => PixelFormat::Rgba32,
            _ => panic!("Invalid pixel format"),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Luma<T>(T);

impl<T> AsRef<T> for Luma<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

#[derive(Debug, PartialEq)]
pub struct Rgb<T>([T; 3]);

impl<T> AsRef<[T; 3]> for Rgb<T> {
    fn as_ref(&self) -> &[T; 3] {
        &self.0
    }
}

#[derive(Debug, PartialEq)]
pub struct Rgba<T>([T; 4]);

impl<T> AsRef<[T; 4]> for Rgba<T> {
    fn as_ref(&self) -> &[T; 4] {
        &self.0
    }
}

pub trait Texel {
    const STRIDE: usize;
    fn from_bytes(bytes: &[u8]) -> Self;
}
impl<T: bytemuck::Pod> Texel for Luma<T> {
    const STRIDE: usize = size_of::<T>();
    fn from_bytes(bytes: &[u8]) -> Self {
        let y = *bytemuck::from_bytes(bytes);
        Self(y)
    }
}
impl<T: bytemuck::Pod> Texel for Rgb<T> {
    const STRIDE: usize = size_of::<T>() * 3;
    fn from_bytes(bytes: &[u8]) -> Self {
        let rgb = *bytemuck::from_bytes(bytes);
        Self(rgb)
    }
}
impl<T: bytemuck::Pod> Texel for Rgba<T> {
    const STRIDE: usize = size_of::<T>() * 4;
    fn from_bytes(bytes: &[u8]) -> Self {
        let rgba = *bytemuck::from_bytes(bytes);
        Self(rgba)
    }
}
