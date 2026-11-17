//! Local text recognition with `ocrs`, a pure-Rust OCR engine. Used by
//! automatic cards to notice new text in the focused window without sending
//! anything off the computer.

use std::path::{Path, PathBuf};

use image::buffer::ConvertBuffer;
use image::imageops::{FilterType, resize};
use image::{RgbImage, RgbaImage};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;

use crate::Result;

pub const DETECTION: &str = "text-detection.onnx";
pub const RECOGNITION: &str = "text-recognition.onnx";
/// Wider images are scaled down first. Chat text stays readable at this size
/// and recognition time grows with area.
const MAX_WIDTH: u32 = 1800;

pub struct Reader {
    engine: OcrEngine,
}

impl Reader {
    /// Loads the two models from `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let detection = Model::load_file(dir.join(DETECTION))?;
        let recognition = Model::load_file(dir.join(RECOGNITION))?;
        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection),
            recognition_model: Some(recognition),
            ..Default::default()
        })?;
        Ok(Self { engine })
    }

    /// The lines of text in `image`, top to bottom.
    pub fn lines(&self, image: &RgbaImage) -> Result<Vec<String>> {
        let rgb: RgbImage = if image.width() > MAX_WIDTH {
            let height = (u64::from(image.height()) * u64::from(MAX_WIDTH)
                / u64::from(image.width()))
            .max(1) as u32;
            resize(image, MAX_WIDTH, height, FilterType::Triangle).convert()
        } else {
            image.convert()
        };
        let source = ImageSource::from_bytes(rgb.as_raw(), rgb.dimensions())?;
        let input = self.engine.prepare_input(source)?;
        let words = self.engine.detect_words(&input)?;
        let lines = self.engine.find_text_lines(&input, &words);
        let texts = self.engine.recognize_text(&input, &lines)?;
        Ok(texts
            .into_iter()
            .flatten()
            .map(|line| line.to_string().trim().to_owned())
            // Very short lines are usually spurious detections.
            .filter(|line| line.chars().count() > 1)
            .collect())
    }
}

/// Where the models are: next to the installed app, or in the repository's
/// `models/` folder for development builds.
pub fn model_dir(resources: Option<PathBuf>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = resources
        .into_iter()
        .map(|dir| dir.join("models"))
        .collect();
    if cfg!(debug_assertions) {
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));
    }
    candidates
        .into_iter()
        .find(|dir| dir.join(DETECTION).exists() && dir.join(RECOGNITION).exists())
}
