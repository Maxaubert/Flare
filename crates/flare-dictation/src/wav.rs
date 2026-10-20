//! WAV encoding and the arithmetic between the microphone and whisper-server (Task 6). Ported from
//! Prism Terminal's `core/renderer/lib/wav.ts` at e5848cb (#13), with `level` from `core/renderer/lib/micCapture.ts`
//! (min(1, rms * 6)) and `silent_warmup_wav` from `core/main/dictationEngine.ts` (`silentWav`).
//!
//! Pure on purpose: everything that can be wrong about the bytes is checked field by field here. The clip is
//! built in memory and handed on; nothing here writes audio to disk (rule 91).

/// The canonical RIFF/WAVE header written by `encode_wav`.
const HEADER_BYTES: usize = 44;
/// PCM16: what whisper-server reads without converting, and half the payload of float32.
const BYTES_PER_SAMPLE: usize = 2;
/// Half a second at Whisper's 16 kHz: the silent pass that warms a server.
const WARMUP_SAMPLES: usize = 8000;
const WHISPER_RATE: u32 = 16000;

/// A complete mono PCM16 little-endian WAV file: the canonical 44-byte RIFF header, then the samples.
/// Anything outside [-1, 1] is clamped (a hot microphone that wraps round is a full-scale click, which
/// Whisper hears as a word). NaN is written as silence.
pub fn encode_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_bytes = samples.len() * BYTES_PER_SAMPLE;
    let mut out = Vec::with_capacity(HEADER_BYTES + data_bytes);
    out.extend_from_slice(b"RIFF");
    // The file, less 'RIFF' and this field.
    out.extend_from_slice(&((HEADER_BYTES - 8 + data_bytes) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // the fmt chunk's own size
    out.extend_from_slice(&1u16.to_le_bytes()); // format 1: integer PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // channels
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(
        &sample_rate
            .saturating_mul(BYTES_PER_SAMPLE as u32)
            .to_le_bytes(),
    ); // bytes a second
    out.extend_from_slice(&(BYTES_PER_SAMPLE as u16).to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits a sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&pcm16(s).to_le_bytes());
    }
    out
}

/// One sample as int16: scaled by 32768 so -1 is the bottom rail exactly, rounded half up (as JavaScript's
/// `Math.round`, which the port is checked against), clamped to the int16 range, NaN as 0.
fn pcm16(s: f32) -> i16 {
    let scaled = (f64::from(s) * 32768.0 + 0.5).floor();
    if scaled.is_nan() {
        return 0;
    }
    scaled.clamp(-32768.0, 32767.0) as i16
}

/// The last `seconds` of a clip, for the live text (Whisper looks at 30 s; the spec sends the last 28).
/// A view into `samples`: the borrow keeps the caller from changing the buffer while the view is in use.
/// Empty for no time, negative time and NaN alike.
pub fn tail_samples(samples: &[f32], sample_rate: u32, seconds: f64) -> &[f32] {
    let want = (seconds * f64::from(sample_rate)).floor();
    if want.is_nan() || want <= 0.0 {
        return &samples[..0];
    }
    let want = if want >= samples.len() as f64 {
        samples.len()
    } else {
        want as usize
    };
    &samples[samples.len() - want..]
}

/// From the rate the audio hardware runs at to `to_rate`. Downsampling averages the input samples that fall in
/// each output slot (a box filter, enough on purpose: the alias is what a voice has above 8 kHz). Upsampling
/// (a slower source, a headset in hands-free mode) interpolates linearly. The slot edges are computed from the
/// index each time, never accumulated, so a long clip does not drift.
///
/// Panics when a rate is zero (a device never reports one; the caller would be wrong).
pub fn downsample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    assert!(
        from_rate > 0 && to_rate > 0,
        "downsample: not a sample rate: {from_rate} to {to_rate}"
    );
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = f64::from(from_rate) / f64::from(to_rate);
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);

    if ratio < 1.0 {
        for i in 0..out_len {
            let pos = i as f64 * ratio;
            let a = pos.floor() as usize;
            let b = (a + 1).min(input.len() - 1);
            let (x, y) = (f64::from(input[a]), f64::from(input[b]));
            out.push((x + (y - x) * (pos - a as f64)) as f32);
        }
        return out;
    }

    for i in 0..out_len {
        let start = (i as f64 * ratio).floor() as usize;
        let next = ((i + 1) as f64 * ratio).floor() as usize;
        let end = input.len().min((start + 1).max(next));
        let sum: f64 = input[start..end].iter().map(|&s| f64::from(s)).sum();
        out.push((sum / (end - start) as f64) as f32);
    }
    out
}

/// Root mean square: 0 to 1 for a signal inside the rails. 0 for nothing at all.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// The level meter's number: rms times six, capped at one.
pub fn level(samples: &[f32]) -> f32 {
    (rms(samples) * 6.0).min(1.0)
}

/// Half a second of silence at 16 kHz, as a WAV file: the pass that loads the model and compiles the kernels
/// before anybody speaks. Nobody reads its text.
pub fn silent_warmup_wav() -> Vec<u8> {
    let silence = [0.0f32; WARMUP_SAMPLES];
    encode_wav(&silence, WHISPER_RATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii(bytes: &[u8], at: usize, len: usize) -> String {
        String::from_utf8_lossy(&bytes[at..at + len]).into_owned()
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }

    fn i16_at(bytes: &[u8], at: usize) -> i16 {
        i16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }

    /// A sine with a little phase, so no sample sits exactly on zero and a crossing is never counted twice.
    fn sine(hz: f64, rate: u32, seconds: f64, amp: f64) -> Vec<f32> {
        let n = (f64::from(rate) * seconds).round() as usize;
        (0..n)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate) + 0.3).sin())
                    as f32
            })
            .collect()
    }

    fn zero_crossings(s: &[f32]) -> usize {
        s.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    // encode_wav

    #[test]
    fn encode_is_44_bytes_of_header_and_two_bytes_a_sample() {
        let samples = [0.0f32, 0.5, -0.5, 1.0, -1.0];
        assert_eq!(encode_wav(&samples, 16000).len(), 44 + samples.len() * 2);
        assert_eq!(encode_wav(&[0.0f32; 16000], 16000).len(), 44 + 32000);
    }

    #[test]
    fn encode_writes_the_riff_header_field_by_field() {
        let wav = encode_wav(&[0.0, 0.5, -0.5, 1.0, -1.0], 16000);
        assert_eq!(ascii(&wav, 0, 4), "RIFF");
        assert_eq!(u32_at(&wav, 4), 36 + 10);
        assert_eq!(ascii(&wav, 8, 4), "WAVE");
        assert_eq!(ascii(&wav, 12, 4), "fmt ");
        assert_eq!(u32_at(&wav, 16), 16);
        assert_eq!(u16_at(&wav, 20), 1, "PCM");
        assert_eq!(u16_at(&wav, 22), 1, "mono");
        assert_eq!(u32_at(&wav, 24), 16000);
        assert_eq!(u32_at(&wav, 28), 32000, "bytes a second");
        assert_eq!(u16_at(&wav, 32), 2, "block align");
        assert_eq!(u16_at(&wav, 34), 16, "bits a sample");
        assert_eq!(ascii(&wav, 36, 4), "data");
        assert_eq!(u32_at(&wav, 40), 10);
    }

    #[test]
    fn encode_carries_the_rate_into_both_rate_fields() {
        let wav = encode_wav(&[0.0; 4], 48000);
        assert_eq!(u32_at(&wav, 24), 48000);
        assert_eq!(u32_at(&wav, 28), 96000);
    }

    #[test]
    fn encode_writes_little_endian_signed_16_bit_samples_full_scale_at_the_rails() {
        let wav = encode_wav(&[0.0, 0.5, -0.5, 1.0, -1.0], 16000);
        let at = |i: usize| i16_at(&wav, 44 + i * 2);
        assert_eq!(at(0), 0);
        assert_eq!(at(1), 16384);
        assert_eq!(at(2), -16384);
        assert_eq!(at(3), 32767);
        assert_eq!(at(4), -32768);
    }

    #[test]
    fn encode_clamps_what_is_outside_the_range_instead_of_wrapping_it() {
        let wav = encode_wav(
            &[1.7, -3.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN],
            16000,
        );
        let got: Vec<i16> = (0..5).map(|i| i16_at(&wav, 44 + i * 2)).collect();
        assert_eq!(got, vec![32767, -32768, 32767, -32768, 0]);
    }

    #[test]
    fn encode_an_empty_clip_is_a_valid_empty_file() {
        let empty = encode_wav(&[], 16000);
        assert_eq!(empty.len(), 44);
        assert_eq!(u32_at(&empty, 4), 36);
        assert_eq!(u32_at(&empty, 40), 0);
    }

    #[test]
    fn encode_reads_a_slice_from_where_it_starts() {
        let whole = [0.25f32, 0.25, -1.0, 1.0];
        let wav = encode_wav(&whole[2..], 16000);
        assert_eq!(u32_at(&wav, 40), 4);
        assert_eq!(i16_at(&wav, 44), -32768);
        assert_eq!(i16_at(&wav, 46), 32767);
    }

    #[test]
    fn warmup_is_44_plus_16000_bytes() {
        let wav = silent_warmup_wav();
        assert_eq!(wav.len(), 44 + 16000);
        assert_eq!(u32_at(&wav, 24), 16000);
        assert_eq!(u32_at(&wav, 40), 16000);
        assert!(wav[44..].iter().all(|&b| b == 0), "silence");
    }

    // tail_samples

    #[test]
    fn tail_is_the_last_n_seconds() {
        let ramp: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let tail = tail_samples(&ramp, 10, 3.0);
        assert_eq!(tail.len(), 30);
        assert_eq!(tail[0], 70.0);
        assert_eq!(tail[29], 99.0);
    }

    #[test]
    fn tail_is_whispers_28_s_window_of_a_long_clip() {
        let clip = vec![0.0f32; 16000 * 40];
        assert_eq!(tail_samples(&clip, 16000, 28.0).len(), 16000 * 28);
    }

    #[test]
    fn tail_is_the_whole_clip_when_the_clip_is_shorter_than_the_window() {
        let ramp: Vec<f32> = (0..100).map(|i| i as f32).collect();
        assert_eq!(tail_samples(&ramp, 10, 28.0), &ramp[..]);
        assert_eq!(tail_samples(&ramp, 10, 10.0).len(), 100);
    }

    #[test]
    fn tail_is_empty_for_no_time_negative_time_nan_and_an_empty_clip() {
        let ramp: Vec<f32> = (0..100).map(|i| i as f32).collect();
        assert!(tail_samples(&ramp, 10, 0.0).is_empty());
        assert!(tail_samples(&ramp, 10, -1.0).is_empty());
        assert!(tail_samples(&ramp, 10, f64::NAN).is_empty());
        assert!(tail_samples(&[], 16000, 28.0).is_empty());
    }

    #[test]
    fn tail_rounds_a_fraction_of_a_sample_down() {
        let ramp: Vec<f32> = (0..100).map(|i| i as f32).collect();
        assert_eq!(tail_samples(&ramp, 10, 0.25).len(), 2);
    }

    // downsample

    #[test]
    fn downsample_at_the_same_rate_gives_the_input_back() {
        let input = sine(440.0, 16000, 0.1, 0.8);
        assert_eq!(downsample(&input, 16000, 16000), input);
    }

    #[test]
    fn downsample_a_second_in_is_a_second_out() {
        for (from, to) in [
            (48000u32, 16000u32),
            (44100, 16000),
            (96000, 16000),
            (32000, 16000),
        ] {
            assert_eq!(
                downsample(&vec![0.0; from as usize], from, to).len(),
                to as usize,
                "{from} to {to}"
            );
            assert_eq!(
                downsample(&vec![0.0; (from / 2) as usize], from, to).len(),
                (to / 2) as usize,
                "half a second, {from} to {to}"
            );
        }
    }

    #[test]
    fn downsample_a_1_khz_sine_stays_a_1_khz_sine() {
        for from in [48000u32, 44100] {
            let out = downsample(&sine(1000.0, from, 1.0, 0.8), from, 16000);
            // Two crossings a cycle; the ends of the clip may cost one.
            assert!(
                zero_crossings(&out).abs_diff(2000) <= 2,
                "{from}: {}",
                zero_crossings(&out)
            );
            // And it is still a sine of about that size, not a thinned-out or clipped one.
            let r = rms(&out);
            assert!(
                r > (0.8 / std::f32::consts::SQRT_2) * 0.9,
                "{from}: rms {r}"
            );
            assert!(
                r < (0.8 / std::f32::consts::SQRT_2) * 1.01,
                "{from}: rms {r}"
            );
        }
    }

    #[test]
    fn downsample_averages_so_a_steady_level_in_is_the_same_level_out() {
        let out = downsample(&vec![0.5f32; 44100], 44100, 16000);
        assert!(out.iter().all(|&s| close(s, 0.5, 1e-6)));
    }

    #[test]
    fn downsample_averages_each_window_rather_than_picking_a_sample_out_of_it() {
        let out = downsample(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 48000, 16000);
        assert_eq!(out.len(), 2);
        assert!(close(out[0], 1.0 / 3.0, 1e-6));
        assert!(close(out[1], 1.0 / 3.0, 1e-6));
    }

    #[test]
    fn downsample_takes_a_slow_microphone_up_to_16_khz_as_well() {
        let out = downsample(&sine(300.0, 8000, 1.0, 0.8), 8000, 16000);
        assert_eq!(out.len(), 16000);
        assert!(zero_crossings(&out).abs_diff(600) <= 2);
    }

    #[test]
    fn downsample_copes_with_nothing() {
        assert!(downsample(&[], 48000, 16000).is_empty());
    }

    #[test]
    #[should_panic(expected = "not a sample rate")]
    fn downsample_refuses_a_zero_rate() {
        downsample(&[0.0; 4], 0, 16000);
    }

    #[test]
    #[should_panic(expected = "not a sample rate")]
    fn downsample_refuses_a_zero_target_rate() {
        downsample(&[0.0; 4], 48000, 0);
    }

    // rms and level

    #[test]
    fn rms_is_zero_for_silence_and_for_nothing_at_all() {
        assert_eq!(rms(&[0.0; 1000]), 0.0);
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn rms_is_the_level_of_a_steady_signal_whichever_way_it_points() {
        assert!(close(rms(&[0.5; 100]), 0.5, 1e-6));
        assert!(close(rms(&[-0.5; 100]), 0.5, 1e-6));
    }

    #[test]
    fn rms_is_amplitude_over_root_two_for_a_sine() {
        assert!(close(
            rms(&sine(1000.0, 16000, 1.0, 1.0)),
            std::f32::consts::FRAC_1_SQRT_2,
            1e-3
        ));
    }

    #[test]
    fn rms_is_one_for_a_full_scale_square_wave() {
        let square: Vec<f32> = (0..64)
            .map(|i| if i % 2 == 1 { 1.0 } else { -1.0 })
            .collect();
        assert!(close(rms(&square), 1.0, 1e-6));
    }

    #[test]
    fn level_is_rms_times_six_capped_at_one() {
        assert_eq!(level(&[]), 0.0);
        assert!(close(level(&[0.1; 100]), 0.6, 1e-6));
        assert_eq!(level(&sine(1000.0, 16000, 1.0, 1.0)), 1.0);
    }
}
