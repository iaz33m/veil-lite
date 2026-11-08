//! OS-native OCR with `Windows.Media.Ocr` (uses the user's installed OCR language packs; no fallback).

use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode, ExifOrientationMode,
    SoftwareBitmap,
};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

/// Text in an encoded image (PNG, JPEG, BMP, GIF, TIFF…). `None` if OCR is unavailable or fails.
pub fn recognize(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    crate::com::ensure_mta();
    match run(bytes) {
        Ok(text) => Some(text),
        Err(e) => {
            tracing::debug!("OCR failed: {e}");
            None
        }
    }
}

/// Whether an OCR engine exists for the user's languages.
pub fn available() -> bool {
    crate::com::ensure_mta();
    OcrEngine::TryCreateFromUserProfileLanguages().is_ok()
}

fn run(bytes: &[u8]) -> windows::core::Result<String> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;

    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    writer.DetachStream()?;
    stream.Seek(0)?;

    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let (w, h) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    let max = OcrEngine::MaxImageDimension()?.max(1);
    let bitmap: SoftwareBitmap = if w > max || h > max {
        // Scale down to the engine's limit, keeping the aspect ratio.
        let scale = (max as f64 / w as f64).min(max as f64 / h as f64);
        let transform = BitmapTransform::new()?;
        transform.SetScaledWidth(((w as f64 * scale) as u32).max(1))?;
        transform.SetScaledHeight(((h as f64 * scale) as u32).max(1))?;
        decoder
            .GetSoftwareBitmapTransformedAsync(
                BitmapPixelFormat::Bgra8,
                BitmapAlphaMode::Premultiplied,
                &transform,
                ExifOrientationMode::RespectExifOrientation,
                ColorManagementMode::DoNotColorManage,
            )?
            .join()?
    } else {
        let b = decoder.GetSoftwareBitmapAsync()?.join()?;
        SoftwareBitmap::Convert(&b, BitmapPixelFormat::Bgra8)?
    };

    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    Ok(result.Text()?.to_string_lossy())
}
