use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use opus::{Application, Channels, Encoder};

use super::{AudioError, Result};

/// Ожидаемая частота дискретизации (preprocess приводит к 16kHz).
#[allow(dead_code)]
const EXPECTED_SAMPLE_RATE: u32 = 16_000;

/// Размер кадра Opus: 20ms при 16 kHz = 320 samples.
#[allow(dead_code)]
const FRAME_SIZE: usize = 320;

/// Максимальный размер одного закодированного Opus-пакета.
#[allow(dead_code)]
const MAX_PACKET_SIZE: usize = 4000;

/// Opus pre-skip (3.5ms encoder delay при 48kHz = 312 samples, стандартное для libopus).
#[allow(dead_code)]
const PRE_SKIP: u16 = 312;

/// Serial number для OGG-потока.
#[allow(dead_code)]
const STREAM_SERIAL: u32 = 1;

/// Granule position increment per 20ms frame (48kHz Opus standard).
#[allow(dead_code)]
const GRANULE_PER_FRAME: u64 = 960;

/// Кодирует PCM mono 16kHz в OGG/Opus.
///
/// На входе ожидается mono 16kHz PCM после `preprocess()`.
/// На выходе - валидный OGG/Opus файл, готовый для отправки в OpenAI API.
/// Bitrate: 24 kbps (VoIP, достаточно для речи).
#[allow(dead_code)]
pub fn encode_ogg_opus(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    if sample_rate != EXPECTED_SAMPLE_RATE {
        return Err(AudioError::EncodingFailed(format!(
            "expected {EXPECTED_SAMPLE_RATE} Hz, got {sample_rate} Hz"
        )));
    }

    let mut encoder = Encoder::new(sample_rate, Channels::Mono, Application::Voip)
        .map_err(|e| AudioError::EncodingFailed(e.to_string()))?;

    encoder
        .set_bitrate(opus::Bitrate::Bits(24000))
        .map_err(|e| AudioError::EncodingFailed(e.to_string()))?;

    let mut out = Vec::new();
    {
        let mut writer = PacketWriter::new(&mut out);

        // OpusHead header (RFC 7845)
        let opus_head = build_opus_head(sample_rate);
        writer
            .write_packet(opus_head, STREAM_SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| AudioError::EncodingFailed(format!("OGG header write: {e}")))?;

        // OpusTags comment header
        let opus_tags = build_opus_tags();
        writer
            .write_packet(opus_tags, STREAM_SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| AudioError::EncodingFailed(format!("OGG tags write: {e}")))?;

        // Audio data packets (20ms frames)
        let total_frames = samples.len().div_ceil(FRAME_SIZE);
        let mut granule_pos: u64 = 0;

        for (i, chunk) in samples.chunks(FRAME_SIZE).enumerate() {
            let mut frame = [0.0f32; FRAME_SIZE];
            frame[..chunk.len()].copy_from_slice(chunk);

            let encoded = encoder
                .encode_vec_float(&frame, MAX_PACKET_SIZE)
                .map_err(|e| AudioError::EncodingFailed(e.to_string()))?;

            granule_pos += GRANULE_PER_FRAME;

            let end_info = if i == total_frames - 1 {
                PacketWriteEndInfo::EndStream
            } else {
                PacketWriteEndInfo::NormalPacket
            };

            writer
                .write_packet(encoded, STREAM_SERIAL, end_info, granule_pos)
                .map_err(|e| AudioError::EncodingFailed(format!("OGG data write: {e}")))?;
        }
    }

    tracing::debug!(
        input_samples = samples.len(),
        output_bytes = out.len(),
        compression_ratio = format_args!(
            "{:.1}x",
            std::mem::size_of_val(samples) as f64 / out.len() as f64
        ),
        "OGG/Opus encoding complete"
    );

    Ok(out)
}

/// Формирует OpusHead header по RFC 7845.
///
/// Структура (19 байт):
/// - magic: "OpusHead" (8 bytes)
/// - version: 1 (1 byte)
/// - channels: 1 (1 byte)
/// - pre-skip: u16 LE (2 bytes)
/// - input sample rate: u32 LE (4 bytes)
/// - output gain: 0 (2 bytes)
/// - channel mapping: 0 (1 byte)
#[allow(dead_code)]
fn build_opus_head(input_sample_rate: u32) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1); // version
    head.push(1); // channels (mono)
    head.extend_from_slice(&PRE_SKIP.to_le_bytes());
    head.extend_from_slice(&input_sample_rate.to_le_bytes());
    head.extend_from_slice(&0u16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family
    head
}

/// Формирует OpusTags comment header по RFC 7845.
///
/// Минимальный: vendor string + 0 comments.
#[allow(dead_code)]
fn build_opus_tags() -> Vec<u8> {
    let vendor = b"VoiceDictator";
    let mut tags = Vec::with_capacity(8 + 4 + vendor.len() + 4);
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes()); // 0 comments
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_tone(sample_rate: u32, duration_ms: u32, freq: f32, amplitude: f32) -> Vec<f32> {
        let num_samples = (sample_rate * duration_ms / 1000) as usize;
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                amplitude * (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect()
    }

    #[test]
    fn encode_should_produce_valid_ogg_with_magic_bytes() {
        // Given
        let tone = generate_tone(16000, 500, 440.0, 0.5);

        // When
        let result = encode_ogg_opus(&tone, 16000).expect("encoding should succeed");

        // Then: OGG файл начинается с "OggS"
        assert!(result.len() > 4);
        assert_eq!(&result[..4], b"OggS");
    }

    #[test]
    fn encode_should_compress_significantly() {
        // Given: 1 second mono 16kHz (raw PCM = 64000 bytes)
        let tone = generate_tone(16000, 1000, 440.0, 0.5);
        let raw_size = tone.len() * std::mem::size_of::<f32>();

        // When
        let encoded = encode_ogg_opus(&tone, 16000).expect("encoding should succeed");

        // Then: >5x compression
        let compression = raw_size as f64 / encoded.len() as f64;
        assert!(
            compression > 5.0,
            "expected >5x compression, got {compression:.1}x"
        );
    }

    #[test]
    fn encode_should_handle_short_audio() {
        // Given: 100ms (меньше одного полного фрейма)
        let tone = generate_tone(16000, 100, 440.0, 0.5);

        // When
        let result = encode_ogg_opus(&tone, 16000);

        // Then: не паника, успешное кодирование
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn encode_should_handle_empty_input() {
        // Given
        let empty: Vec<f32> = vec![];

        // When
        let result = encode_ogg_opus(&empty, 16000).expect("empty encoding should not fail");

        // Then
        assert!(result.is_empty());
    }

    #[test]
    fn opus_head_should_have_correct_structure() {
        // Given / When
        let head = build_opus_head(16000);

        // Then
        assert_eq!(head.len(), 19);
        assert_eq!(&head[..8], b"OpusHead");
        assert_eq!(head[8], 1); // version
        assert_eq!(head[9], 1); // mono
    }

    #[test]
    fn opus_tags_should_have_correct_structure() {
        // Given / When
        let tags = build_opus_tags();

        // Then
        assert_eq!(&tags[..8], b"OpusTags");
        let vendor_len = u32::from_le_bytes(tags[8..12].try_into().unwrap()) as usize;
        assert_eq!(&tags[12..12 + vendor_len], b"VoiceDictator");
    }

    #[test]
    fn encode_should_reject_wrong_sample_rate() {
        // Given
        let tone = generate_tone(44100, 500, 440.0, 0.5);

        // When
        let result = encode_ogg_opus(&tone, 44100);

        // Then
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("expected 16000 Hz"), "got: {err}");
    }
}
