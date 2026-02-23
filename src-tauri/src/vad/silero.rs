use std::path::Path;

use ndarray::Array3;
use ort::session::Session;
use ort::value::{Tensor, TensorRef};

use super::{VadError, VoiceDetector, VAD_FRAME_SIZE};

/// Размер LSTM-состояния в Silero VAD v5.
const STATE_DIM: usize = 128;

/// Sample rate (Silero VAD работает на 16kHz).
const SAMPLE_RATE: i64 = 16000;

/// Silero VAD через ONNX Runtime.
///
/// Выполняет инференс модели Silero VAD v5 для детекции речи/тишины
/// по кадрам аудио (512 samples = 32ms при 16kHz).
pub struct SileroVad {
    session: Session,
    state: Array3<f32>,
    threshold: f32,
}

impl SileroVad {
    /// Загружает модель и создает VAD с заданным порогом вероятности речи.
    ///
    /// `threshold` - порог вероятности (0.0..1.0), стандарт: 0.5.
    pub fn new(model_path: &Path, threshold: f32) -> super::Result<Self> {
        let session = Session::builder()
            .map_err(|e| VadError::ModelLoadFailed(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| VadError::ModelLoadFailed(e.to_string()))?
            .commit_from_file(model_path)
            .map_err(|e| VadError::ModelLoadFailed(e.to_string()))?;

        Ok(Self {
            session,
            state: Array3::<f32>::zeros((2, 1, STATE_DIM)),
            threshold,
        })
    }

    /// Возвращает вероятность речи для кадра (0.0..1.0).
    pub fn speech_probability(&mut self, frame: &[f32]) -> super::Result<f32> {
        if frame.len() != VAD_FRAME_SIZE {
            return Err(VadError::InvalidFrameSize {
                expected: VAD_FRAME_SIZE,
                got: frame.len(),
            });
        }

        let input = TensorRef::from_array_view(([1_usize, frame.len()], frame))
            .map_err(|e| VadError::InferenceFailed(e.to_string()))?;

        let sr = Tensor::<i64>::from_array(([1_usize], vec![SAMPLE_RATE].into_boxed_slice()))
            .map_err(|e| VadError::InferenceFailed(e.to_string()))?;

        let state_view = TensorRef::from_array_view(self.state.view())
            .map_err(|e| VadError::InferenceFailed(e.to_string()))?;

        let outputs = self
            .session
            .run(ort::inputs! {
                "input" => input,
                "sr" => sr,
                "state" => state_view
            })
            .map_err(|e| VadError::InferenceFailed(e.to_string()))?;

        let (_, prob_data) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| VadError::InferenceFailed(e.to_string()))?;
        let speech_prob = prob_data[0];

        let new_state = outputs["stateN"]
            .try_extract_array::<f32>()
            .map_err(|e| VadError::InferenceFailed(e.to_string()))?;
        self.state = new_state
            .into_dimensionality::<ndarray::Ix3>()
            .map_err(|e| VadError::InferenceFailed(e.to_string()))?
            .to_owned();

        Ok(speech_prob)
    }
}

impl VoiceDetector for SileroVad {
    fn is_speech(&mut self, frame: &[f32]) -> super::Result<bool> {
        let prob = self.speech_probability(frame)?;
        Ok(prob >= self.threshold)
    }

    fn reset(&mut self) {
        self.state = Array3::<f32>::zeros((2, 1, STATE_DIM));
    }
}
