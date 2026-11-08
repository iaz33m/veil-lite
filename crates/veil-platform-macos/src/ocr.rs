//! OS-native OCR with the Vision framework (`VNRecognizeTextRequest`, accurate level). Runs
//! synchronously on the calling (worker) thread.

use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_foundation::{NSArray, NSData, NSDictionary};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

/// Text in an encoded image (PNG, JPEG, TIFF, HEIC, …). `None` if Vision fails to read it.
pub fn recognize(image: &[u8]) -> Option<String> {
    if image.is_empty() {
        return None;
    }
    autoreleasepool(|_| {
        let data = NSData::with_bytes(image);
        let options: Retained<NSDictionary<VNImageOption, AnyObject>> = NSDictionary::new();
        let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &options);

        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);

        let base: &VNRequest = &request;
        let requests = NSArray::from_slice(&[base]);
        if let Err(e) = handler.performRequests_error(&requests) {
            tracing::debug!("Vision OCR failed: {e:?}");
            return None;
        }
        let results = request.results()?;
        let mut lines = Vec::new();
        for observation in results.iter() {
            let candidates = observation.topCandidates(1);
            if let Some(best) = candidates.iter().next() {
                lines.push(best.string().to_string());
            }
        }
        Some(lines.join("\n"))
    })
}
