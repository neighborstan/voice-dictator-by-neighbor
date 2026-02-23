/// Целевая частота дискретизации для STT.
#[allow(dead_code)]
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Размер кадра для анализа энергии (в миллисекундах).
#[allow(dead_code)]
const ENERGY_FRAME_MS: u32 = 20;

/// Порог RMS для определения тишины (подобрано эмпирически).
#[allow(dead_code)]
const SILENCE_RMS_THRESHOLD: f32 = 0.01;

/// Конвертирует multi-channel аудио в mono.
///
/// Если аудио уже mono (channels == 1), возвращает копию.
/// Для multi-channel усредняет значения по всем каналам.
#[allow(dead_code)]
pub fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }

    let ch = channels as usize;
    samples
        .chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Ресемплинг с линейной интерполяцией.
///
/// Для STT достаточно линейной интерполяции.
/// Если частоты совпадают, возвращает копию.
#[allow(dead_code)]
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = ((samples.len() as f64) / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;

        let sample = if idx + 1 < samples.len() {
            samples[idx] * (1.0 - frac) + samples[idx + 1] * frac
        } else {
            samples[samples.len() - 1]
        };
        output.push(sample);
    }

    output
}

/// Препроцессинг аудио: конвертация в mono + ресемплинг в 16 kHz.
#[allow(dead_code)]
pub fn preprocess(samples: &[f32], channels: u16, sample_rate: u32) -> Vec<f32> {
    let mono = to_mono(samples, channels);
    resample(&mono, sample_rate, TARGET_SAMPLE_RATE)
}

/// Вычисляет RMS энергию кадра.
#[allow(dead_code)]
pub fn calculate_energy(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = frame.iter().map(|&s| s * s).sum();
    (sum_sq / frame.len() as f32).sqrt()
}

/// Обрезает тишину в начале аудио.
///
/// Анализирует кадрами по `ENERGY_FRAME_MS` мс. Возвращает срез
/// начиная с первого кадра, где RMS превышает порог.
/// `min_silence_ms` - минимальная длительность тишины для обрезки.
#[allow(dead_code)]
pub fn trim_leading_silence(samples: &[f32], sample_rate: u32, min_silence_ms: u32) -> &[f32] {
    if samples.is_empty() {
        return samples;
    }

    let frame_size = (sample_rate * ENERGY_FRAME_MS / 1000) as usize;
    let min_silent_frames = (min_silence_ms / ENERGY_FRAME_MS) as usize;

    if frame_size == 0 {
        return samples;
    }

    let mut silent_frames = 0;
    let mut first_voice_sample = 0;

    for (i, frame) in samples.chunks(frame_size).enumerate() {
        let energy = calculate_energy(frame);
        if energy > SILENCE_RMS_THRESHOLD {
            first_voice_sample = i * frame_size;
            break;
        }
        silent_frames += 1;
        first_voice_sample = (i + 1) * frame_size;
    }

    if silent_frames < min_silent_frames {
        return samples;
    }

    if first_voice_sample >= samples.len() {
        return &samples[0..0];
    }

    &samples[first_voice_sample..]
}

/// Обрезает тишину в конце аудио.
///
/// Ищет последний кадр с энергией выше порога.
/// `min_silence_ms` - минимальная длительность хвостовой тишины для обрезки.
#[allow(dead_code)]
pub fn trim_trailing_silence(samples: &[f32], sample_rate: u32, min_silence_ms: u32) -> &[f32] {
    if samples.is_empty() {
        return samples;
    }

    let frame_size = (sample_rate * ENERGY_FRAME_MS / 1000) as usize;
    let min_silent_frames = (min_silence_ms / ENERGY_FRAME_MS) as usize;

    if frame_size == 0 {
        return samples;
    }

    let mut trailing_silence = 0;
    let mut last_voice_end = samples.len();

    for (i, frame) in samples.chunks(frame_size).enumerate().rev() {
        let energy = calculate_energy(frame);
        if energy > SILENCE_RMS_THRESHOLD {
            last_voice_end = (i + 1) * frame_size;
            break;
        }
        trailing_silence += 1;
        if i == 0 {
            last_voice_end = 0;
        }
    }

    if trailing_silence < min_silent_frames {
        return samples;
    }

    let end = last_voice_end.min(samples.len());
    &samples[..end]
}

/// Обрезает тишину в начале и конце аудио.
///
/// Дефолтные пороги: 200ms для начала, 500ms для конца.
#[allow(dead_code)]
pub fn trim_silence(samples: &[f32], sample_rate: u32) -> &[f32] {
    let trimmed = trim_leading_silence(samples, sample_rate, 200);
    trim_trailing_silence(trimmed, sample_rate, 500)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Генерирует тишину (нули) заданной длительности.
    fn generate_silence(sample_rate: u32, duration_ms: u32) -> Vec<f32> {
        vec![0.0; (sample_rate * duration_ms / 1000) as usize]
    }

    /// Генерирует синусоидальный тон.
    fn generate_tone(sample_rate: u32, duration_ms: u32, freq: f32, amplitude: f32) -> Vec<f32> {
        let num_samples = (sample_rate * duration_ms / 1000) as usize;
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                amplitude * (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect()
    }

    /// Генерирует стерео данные: каждый mono-сэмпл дублируется в 2 канала.
    fn make_stereo(mono: &[f32]) -> Vec<f32> {
        mono.iter().flat_map(|&s| [s, s]).collect()
    }

    // --- to_mono ---

    #[test]
    fn to_mono_should_return_same_for_mono_input() {
        // Given
        let mono = vec![0.1, 0.2, 0.3, 0.4];

        // When
        let result = to_mono(&mono, 1);

        // Then
        assert_eq!(result.len(), 4);
        assert_eq!(result, mono);
    }

    #[test]
    fn to_mono_should_average_stereo_channels() {
        // Given: stereo [L, R, L, R] => [(L+R)/2, (L+R)/2]
        let stereo = vec![0.2, 0.8, 0.4, 0.6];

        // When
        let result = to_mono(&stereo, 2);

        // Then
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.5).abs() < 1e-6);
        assert!((result[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn to_mono_should_halve_length_for_stereo() {
        // Given
        let stereo = vec![0.0; 1000];

        // When
        let result = to_mono(&stereo, 2);

        // Then
        assert_eq!(result.len(), 500);
    }

    // --- resample ---

    #[test]
    fn resample_should_return_same_when_rates_equal() {
        // Given
        let samples = vec![0.1, 0.2, 0.3];

        // When
        let result = resample(&samples, 16000, 16000);

        // Then
        assert_eq!(result.len(), 3);
        assert_eq!(result, samples);
    }

    #[test]
    fn resample_should_produce_correct_length_44100_to_16000() {
        // Given: 1 second of 44100 Hz
        let samples = vec![0.0; 44100];

        // When
        let result = resample(&samples, 44100, 16000);

        // Then: ~16000 samples (1 second at 16kHz)
        let expected = (44100.0_f64 / (44100.0_f64 / 16000.0_f64)).ceil() as usize;
        assert_eq!(result.len(), expected);
    }

    #[test]
    fn resample_should_handle_empty_input() {
        // Given
        let samples: Vec<f32> = vec![];

        // When
        let result = resample(&samples, 44100, 16000);

        // Then
        assert!(result.is_empty());
    }

    // --- preprocess ---

    #[test]
    fn preprocess_should_return_mono_16khz() {
        // Given: 1 second stereo 44100 Hz
        let mono = vec![0.5; 44100];
        let stereo = make_stereo(&mono);

        // When
        let result = preprocess(&stereo, 2, 44100);

        // Then: mono 16kHz => ~16000 samples
        let expected_len = (44100.0_f64 / (44100.0_f64 / 16000.0_f64)).ceil() as usize;
        assert_eq!(result.len(), expected_len);
    }

    // --- calculate_energy ---

    #[test]
    fn calculate_energy_should_return_zero_for_silence() {
        // Given
        let silence = vec![0.0; 320];

        // When
        let energy = calculate_energy(&silence);

        // Then
        assert_eq!(energy, 0.0);
    }

    #[test]
    fn calculate_energy_should_return_positive_for_signal() {
        // Given
        let tone = generate_tone(16000, 20, 440.0, 0.5);

        // When
        let energy = calculate_energy(&tone);

        // Then
        assert!(energy > 0.0);
    }

    #[test]
    fn calculate_energy_should_handle_empty_frame() {
        // Given
        let empty: Vec<f32> = vec![];

        // When
        let energy = calculate_energy(&empty);

        // Then
        assert_eq!(energy, 0.0);
    }

    // --- trim_leading_silence ---

    #[test]
    fn trim_should_remove_leading_silence() {
        // Given: 500ms silence + 500ms tone
        let mut audio = generate_silence(16000, 500);
        let tone = generate_tone(16000, 500, 440.0, 0.5);
        audio.extend_from_slice(&tone);
        let original_len = audio.len();

        // When
        let result = trim_leading_silence(&audio, 16000, 200);

        // Then
        assert!(result.len() < original_len);
    }

    #[test]
    fn trim_should_remove_trailing_silence() {
        // Given: 500ms tone + 800ms silence
        let mut audio = generate_tone(16000, 500, 440.0, 0.5);
        let silence = generate_silence(16000, 800);
        audio.extend_from_slice(&silence);
        let original_len = audio.len();

        // When
        let result = trim_trailing_silence(&audio, 16000, 500);

        // Then
        assert!(result.len() < original_len);
    }

    #[test]
    fn trim_should_preserve_speech_signal() {
        // Given: только тон, без тишины
        let tone = generate_tone(16000, 1000, 440.0, 0.5);

        // When
        let result = trim_silence(&tone, 16000);

        // Then: ничего не обрезано (или почти ничего)
        assert!(result.len() >= tone.len() - 320); // допуск в 1 кадр
    }

    #[test]
    fn trim_should_handle_all_silence() {
        // Given: полная тишина
        let silence = generate_silence(16000, 2000);

        // When
        let result = trim_silence(&silence, 16000);

        // Then: пустой буфер (не паника)
        assert!(result.is_empty());
    }

    #[test]
    fn trim_should_handle_no_silence() {
        // Given: только громкий сигнал
        let tone = generate_tone(16000, 500, 440.0, 0.8);

        // When
        let result = trim_silence(&tone, 16000);

        // Then: ничего не обрезано
        assert_eq!(result.len(), tone.len());
    }

    #[test]
    fn trim_should_handle_empty_input() {
        // Given
        let empty: Vec<f32> = vec![];

        // When
        let result = trim_silence(&empty, 16000);

        // Then
        assert!(result.is_empty());
    }

    #[test]
    fn trim_silence_should_remove_both_leading_and_trailing() {
        // Given: 400ms silence + 500ms tone + 700ms silence
        let mut audio = generate_silence(16000, 400);
        let tone = generate_tone(16000, 500, 440.0, 0.5);
        let trailing = generate_silence(16000, 700);
        audio.extend_from_slice(&tone);
        audio.extend_from_slice(&trailing);
        let original_len = audio.len();

        // When
        let result = trim_silence(&audio, 16000);

        // Then: должно быть обрезано с обеих сторон
        assert!(result.len() < original_len);
        // Результат примерно равен длине тона
        let tone_len = tone.len();
        let tolerance = (16000 * ENERGY_FRAME_MS / 1000) as usize * 2;
        assert!(result.len() <= tone_len + tolerance);
    }
}
