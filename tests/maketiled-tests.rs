use std::fs::File;

use image::EncodableLayout as _;
use texture_cache_rs::*;

#[test]
fn test() {
    let checker = image::open("tests/checker.png").expect("Unable to open test image");
    assert_eq!(1024, checker.width());
    assert_eq!(1024, checker.height());
    let rgb8 = checker.into_rgb8();
    let format = PixelFormat::RGB8;
    let tile_size = PowerOfTwo::try_from(64).unwrap();
    let mut out_file = File::create("tests/checker.txp").expect("Unable to create output file");
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
}
