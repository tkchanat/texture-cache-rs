fn main() {
    println!("Hello, world!");
}

#[cfg(test)]
mod tests {
    use image::{EncodableLayout as _, ImageBuffer, Pixel, Rgb};
    use std::fs::File;
    use texture_cache_rs::*;

    #[test]
    fn test_maketiled() {
        let checker = image::open("assets/checker.png").expect("Unable to open test image");
        assert_eq!(1024, checker.width());
        assert_eq!(1024, checker.height());
        let rgb8 = checker.into_rgb8();
        let format = PixelFormat::RGB8;
        let tile_size = PowerOfTwo::try_from(64).unwrap();
        let file_path = "assets/checker.txp";
        let mut out_file = File::create(file_path).expect("Unable to create output file");
        assert!(
            TiledImage::create_with_mips(
                rgb8.as_bytes(),
                rgb8.width(),
                rgb8.height(),
                format,
                tile_size,
                WrapMode::Clamp,
                &mut out_file,
            )
            .is_ok()
        );

        let image = TiledImage::open(file_path).expect("Unable to open test image");
        let bytes = image.get_level(0).expect("Cannot get level 0");
        let out: ImageBuffer<Rgb<u8>, Vec<u8>> =
            image::ImageBuffer::from_raw(1024, 1024, bytes).unwrap();
        let mut out_file = File::create("assets/test.png").unwrap();
        out.write_to(&mut out_file, image::ImageFormat::Png)
            .unwrap();
    }
}
