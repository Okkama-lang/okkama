//! PNG decoding, resizing and encoding off the VM thread.
use image::{DynamicImage, ImageFormat, imageops::FilterType};
use okkama_ext::{Failure, Shared, extension, native, state};
use std::io::Cursor;

#[state(shared)]
struct Image(DynamicImage);

#[native(blocking)]
fn decode(bytes: &[u8]) -> Result<Image, Failure> {
    image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map(Image).map_err(|error| Failure::new("bad_format", error.to_string()))
}

#[native(blocking)]
fn resize(image: Shared<Image>, width: i64, height: i64) -> Result<Image, Failure> {
    let dimension = |value| u32::try_from(value).ok().filter(|&value| value > 0 && value <= 4096);
    let (Some(width), Some(height)) = (dimension(width), dimension(height)) else {
        return Err(Failure::new("bad_size", "dimensions must be between 1 and 4096"));
    };
    Ok(Image(image.0.resize(width, height, FilterType::Triangle)))
}

#[native(blocking)]
fn encode(image: Shared<Image>) -> Result<Vec<u8>, Failure> {
    let mut output = Cursor::new(Vec::new());
    image.0.write_to(&mut output, ImageFormat::Png)
        .map_err(|error| Failure::new("encode_failed", error.to_string()))?;
    Ok(output.into_inner())
}

#[native]
fn width(image: &Image) -> i64 { i64::from(image.0.width()) }
#[native]
fn height(image: &Image) -> i64 { i64::from(image.0.height()) }
#[native(release)]
fn release(image: Shared<Image>) { drop(image); }

extension!("imagekit/native", [decode, resize, encode, width, height, release], states: [Image]);
