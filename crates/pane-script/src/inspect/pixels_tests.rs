use super::*;

#[test]
fn png_preserves_rgba_and_omits_stride_padding() {
    let source = [255, 0, 17, 32, 99, 99, 99, 99, 2, 3, 4, 255];
    let image = Pixels::copy(1, 2, 8, &source).unwrap().encode().unwrap();
    assert_eq!(image["width"], 1);
    assert_eq!(image["height"], 2);
    let png = STANDARD.decode(image["data"].as_str().unwrap()).unwrap();
    assert_eq!(image["bytes"].as_u64().unwrap(), png.len() as u64);
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut decoded).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(
        &decoded[..info.buffer_size()],
        &[255, 0, 17, 32, 2, 3, 4, 255]
    );
}

#[test]
fn copied_pixels_outlive_the_borrowed_readback() {
    let mut source = [1, 2, 3, 4];
    let pixels = Pixels::copy(1, 1, 4, &source).unwrap();
    source.fill(0);
    assert_eq!(&*pixels.rgba, &[1, 2, 3, 4]);
}

#[test]
fn invalid_layouts_are_rejected_before_allocating() {
    assert!(Pixels::copy(0, 1, 0, &[]).is_err());
    assert!(Pixels::copy(1, 0, 0, &[]).is_err());
    assert!(Pixels::copy(2, 1, 4, &[0; 8]).is_err());
    assert!(Pixels::copy(1, 2, 8, &[0; 11]).is_err());
    assert!(Pixels::copy(1, 3, usize::MAX, &[]).is_err());
    assert!(Pixels::copy(u32::MAX, u32::MAX, usize::MAX, &[]).is_err());
    assert!(Pixels::copy(4096, 4097, 16384, &[]).is_err());
}

#[test]
fn png_limit_is_enforced_during_encoding() {
    let mut noise = vec![0; 1024 * 1024 * 4];
    let mut state = 0x12345678_u32;
    for pixel in noise.as_chunks_mut::<4>().0 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        pixel.copy_from_slice(&state.to_le_bytes());
    }
    let error = Pixels::copy(1024, 1024, 4096, &noise)
        .unwrap()
        .encode()
        .unwrap_err();
    assert!(error.contains("2 MiB"), "{error}");
}
