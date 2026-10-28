use opus::{Application, Channels, Encoder, ErrorCode, FrameSize};
use rs_audio::codec::{AudioDecoder, AudioEncoder, EncoderConfig};
use rs_audio::{AudioError, FRAME_LEN, FRAME_SAMPLES, MAX_PACKET, SAMPLE_RATE};

const AMPLITUDE: f32 = 0.5;
const FREQUENCY_HZ: f32 = 440.0;
const FRAME_COUNT: usize = 100;

fn sine_pcm(frame_count: usize) -> Vec<f32> {
    let mut pcm = Vec::with_capacity(frame_count * FRAME_LEN);
    for sample_index in 0..frame_count * FRAME_SAMPLES {
        let phase = std::f32::consts::TAU * FREQUENCY_HZ * sample_index as f32 / SAMPLE_RATE as f32;
        let sample = AMPLITUDE * phase.sin();
        pcm.push(sample);
        pcm.push(sample);
    }
    pcm
}

fn encode_frames(encoder: &mut AudioEncoder, pcm: &[f32]) -> Vec<Vec<u8>> {
    pcm.chunks_exact(FRAME_LEN)
        .map(|frame| {
            let mut packet = vec![0u8; MAX_PACKET];
            let len = encoder.encode(frame, &mut packet).expect("encode frame");
            assert!((1..=MAX_PACKET).contains(&len));
            packet.truncate(len);
            packet
        })
        .collect()
}

fn rms(samples: impl Iterator<Item = f32>) -> f32 {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for sample in samples {
        sum += f64::from(sample).powi(2);
        count += 1;
    }
    (sum / count as f64).sqrt() as f32
}

fn left_channel(pcm: &[f32], frame_count: usize) -> impl Iterator<Item = f32> + '_ {
    pcm.chunks_exact(FRAME_LEN)
        .take(frame_count)
        .flat_map(|frame| frame.iter().step_by(2).copied())
}

fn decode_frames(decoder: &mut AudioDecoder, packets: &[Vec<u8>]) -> Vec<f32> {
    let mut pcm = Vec::with_capacity(packets.len() * FRAME_LEN);
    for packet in packets {
        let mut frame = vec![0.0f32; FRAME_LEN];
        let samples = decoder.decode(packet, &mut frame).expect("decode frame");
        assert_eq!(samples, FRAME_SAMPLES);
        pcm.extend_from_slice(&frame);
    }
    pcm
}

#[test]
fn default_config_matches_protocol() {
    assert_eq!(
        EncoderConfig::default(),
        EncoderConfig {
            bitrate_bps: 96_000,
            complexity: 8,
            inband_fec: true,
            packet_loss_perc: 5,
            low_delay: false,
        }
    );

    let encoder = AudioEncoder::new(&EncoderConfig::default()).expect("encoder");
    assert!(encoder.lookahead_samples().expect("lookahead") > 0);
}

#[test]
fn invalid_config_is_reported_as_codec_error() {
    let invalid = EncoderConfig {
        complexity: 11,
        ..EncoderConfig::default()
    };
    assert!(matches!(
        AudioEncoder::new(&invalid),
        Err(AudioError::Codec {
            code: ErrorCode::BadArg,
            ..
        })
    ));
}

#[test]
fn malformed_packet_reports_invalid_packet_codec_error() {
    let mut decoder = AudioDecoder::new().expect("decoder");
    let mut out = vec![0.0f32; FRAME_LEN];
    // Code 3 requires a second framing byte, so a one-byte packet is invalid.
    let error = decoder.decode(&[0xff], &mut out).expect_err("bad packet");
    assert!(matches!(
        error,
        AudioError::Codec {
            code: ErrorCode::InvalidPacket,
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "opus opus_decoder_get_nb_samples: corrupted stream"
    );
}

#[test]
fn round_trip_bitrate_and_quality() {
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).expect("encoder");
    let mut decoder = AudioDecoder::new().expect("decoder");
    let pcm = sine_pcm(FRAME_COUNT);

    let packets = encode_frames(&mut encoder, &pcm);
    assert_eq!(packets.len(), FRAME_COUNT);

    let total_bytes: usize = packets.iter().map(Vec::len).sum();
    let bits_per_second = total_bytes as f64 * 8.0;
    assert!(
        (72_000.0..=120_000.0).contains(&bits_per_second),
        "bitrate {bits_per_second}"
    );

    let decoded = decode_frames(&mut decoder, &packets);
    assert_eq!(decoded.len(), FRAME_COUNT * FRAME_LEN);

    let skipped = 2;
    let source_rms = rms(left_channel(&pcm, FRAME_COUNT).skip(skipped * FRAME_SAMPLES));
    let decoded_rms = rms(left_channel(&decoded, FRAME_COUNT).skip(skipped * FRAME_SAMPLES));
    let difference_db = 20.0 * (decoded_rms / source_rms).log10();
    assert!(
        difference_db.abs() < 1.0,
        "decoded RMS {decoded_rms}, source RMS {source_rms}, difference {difference_db} dB"
    );
}

#[test]
fn rejects_wrong_frame_and_output_sizes() {
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).expect("encoder");
    let pcm = vec![0.0f32; FRAME_LEN];
    let mut out = vec![0u8; MAX_PACKET];

    assert!(matches!(
        encoder.encode(&pcm[..FRAME_LEN - 1], &mut out),
        Err(AudioError::Invalid("frame length"))
    ));
    assert!(matches!(
        encoder.encode(&pcm, &mut out[..100]),
        Err(AudioError::Invalid("output buffer"))
    ));

    let mut decoder = AudioDecoder::new().expect("decoder");
    let mut decoded = vec![0.0f32; FRAME_LEN - 1];
    assert!(matches!(
        decoder.conceal(&mut decoded),
        Err(AudioError::Invalid("output buffer"))
    ));
}

#[test]
fn non_finite_input_is_encoded_as_silence() {
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).expect("encoder");
    let mut decoder = AudioDecoder::new().expect("decoder");
    let nan_frame = vec![f32::NAN; FRAME_LEN];
    let infinity_frame = vec![f32::INFINITY; FRAME_LEN];

    let mut packets = Vec::new();
    for frame in [&nan_frame, &infinity_frame, &nan_frame] {
        let mut packet = vec![0u8; MAX_PACKET];
        let len = encoder.encode(frame, &mut packet).expect("encode frame");
        assert!((1..=MAX_PACKET).contains(&len));
        packet.truncate(len);
        packets.push(packet);
    }

    let decoded = decode_frames(&mut decoder, &packets);
    let peak = decoded
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(peak < 1e-3, "decoded peak {peak}");
}

#[test]
fn clipped_input_stays_in_range() {
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).expect("encoder");
    let mut decoder = AudioDecoder::new().expect("decoder");
    let clipped_frame: Vec<f32> = (0..FRAME_LEN)
        .map(|index| if index % 2 == 0 { 8.0 } else { -8.0 })
        .collect();
    let mut packet = vec![0u8; MAX_PACKET];
    let len = encoder
        .encode(&clipped_frame, &mut packet)
        .expect("encode clipped frame");
    let mut decoded = vec![0.0f32; FRAME_LEN];
    assert_eq!(
        decoder
            .decode(&packet[..len], &mut decoded)
            .expect("decode clipped frame"),
        FRAME_SAMPLES
    );
    assert!(decoded.iter().all(|sample| sample.is_finite()));
}

#[test]
fn empty_packet_and_concealment() {
    let mut decoder = AudioDecoder::new().expect("decoder");
    let mut out = vec![0.0f32; FRAME_LEN + 16];
    assert!(matches!(
        decoder.decode(&[], &mut out),
        Err(AudioError::Invalid("empty packet"))
    ));

    let tail = vec![0.25f32; 16];
    out[FRAME_LEN..].copy_from_slice(&tail);
    for _ in 0..3 {
        assert_eq!(decoder.conceal(&mut out).expect("conceal"), FRAME_SAMPLES);
        assert_eq!(&out[FRAME_LEN..], tail.as_slice());
    }
}

#[test]
fn fec_recovery_after_dropped_packet() {
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).expect("encoder");
    let mut decoder = AudioDecoder::new().expect("decoder");
    let pcm = sine_pcm(52);
    let packets = encode_frames(&mut encoder, &pcm);

    for packet in &packets[..50] {
        let mut out = vec![0.0f32; FRAME_LEN];
        assert_eq!(
            decoder.decode(packet, &mut out).expect("decode"),
            FRAME_SAMPLES
        );
    }

    let mut out = vec![0.25f32; FRAME_LEN + 16];
    assert_eq!(
        decoder.recover(&packets[51], &mut out).expect("recover"),
        FRAME_SAMPLES
    );
    assert!(out[FRAME_LEN..].iter().all(|sample| *sample == 0.25));
}

#[test]
fn rejects_twenty_millisecond_packets() {
    let mut encoder =
        Encoder::new(SAMPLE_RATE, Channels::Stereo, Application::Audio).expect("raw encoder");
    encoder
        .set_expert_frame_duration(FrameSize::Ms20)
        .expect("20 ms frame duration");
    let pcm = vec![0.0f32; FRAME_LEN * 2];
    let mut packet = vec![0u8; MAX_PACKET];
    let len = encoder.encode_float(&pcm, &mut packet).expect("encode");

    let mut decoder = AudioDecoder::new().expect("decoder");
    let mut out = vec![0.0f32; FRAME_LEN];
    assert!(matches!(
        decoder.decode(&packet[..len], &mut out),
        Err(AudioError::Invalid("frame duration"))
    ));
}

#[test]
fn reset_restores_encoding_and_decoding() {
    let mut encoder = AudioEncoder::new(&EncoderConfig::default()).expect("encoder");
    let mut decoder = AudioDecoder::new().expect("decoder");
    let pcm = sine_pcm(12);

    let first = encode_frames(&mut encoder, &pcm);
    let first_decoded = decode_frames(&mut decoder, &first);
    assert_eq!(first_decoded.len(), 12 * FRAME_LEN);

    encoder.reset().expect("encoder reset");
    decoder.reset().expect("decoder reset");

    let second = encode_frames(&mut encoder, &pcm);
    let second_decoded = decode_frames(&mut decoder, &second);
    assert_eq!(second_decoded.len(), 12 * FRAME_LEN);
    let decoded_rms = rms(left_channel(&second_decoded, 12).skip(2 * FRAME_SAMPLES));
    let source_rms = rms(left_channel(&pcm, 12).skip(2 * FRAME_SAMPLES));
    let difference_db = 20.0 * (decoded_rms / source_rms).log10();
    assert!(
        difference_db.abs() < 1.0,
        "difference after reset: {difference_db} dB"
    );

    encoder.set_bitrate(64_000).expect("set bitrate");
    let third = encode_frames(&mut encoder, &pcm);
    let third_decoded = decode_frames(&mut decoder, &third);
    assert_eq!(third_decoded.len(), 12 * FRAME_LEN);
}

#[test]
fn codec_types_are_send() {
    fn assert_send<T: Send>() {}

    assert_send::<AudioEncoder>();
    assert_send::<AudioDecoder>();
}
