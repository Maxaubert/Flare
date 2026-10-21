//! The start and stop cues (Task 8). Ported from Prism Terminal's `core/renderer/lib/dictationSounds.ts` at
//! e5848cb: two short sine notes when the microphone opens (587.33 Hz then 880 Hz), the same pair reversed when
//! it closes, and one low note (392 Hz) on a cancel. Synthesised, so there is no asset to ship or license.
//!
//! The cue is built in memory as a PCM16 WAV and played by `PlaySoundW` on a worker thread, so the caller never
//! waits for the sound. The buffer lives on that worker until the next cue replaces it.

/// The three sounds the dictation makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    Start,
    Stop,
    Cancel,
}

/// The loudest a note gets (PT's gain ramps to 0.12).
pub const PEAK: f32 = 0.12;
/// The attack of each note, in milliseconds (PT ramps up over 12 ms, so a switched-on sine does not click).
pub const ATTACK_MS: f64 = 12.0;
/// The rate the cue is played at. Plenty for two sine tones.
const PLAY_RATE: u32 = 22050;

/// One sine note: a frequency, when it starts and how long it sounds, all in milliseconds from the cue's start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub hz: f64,
    pub at_ms: f64,
    pub len_ms: f64,
}

/// The notes of a cue. The start is the rising pair; the stop is the same pair with the frequencies swapped
/// (falling); the cancel is one low note.
pub fn notes(c: Cue) -> Vec<Note> {
    match c {
        Cue::Start => vec![
            Note {
                hz: 587.33,
                at_ms: 0.0,
                len_ms: 90.0,
            },
            Note {
                hz: 880.0,
                at_ms: 70.0,
                len_ms: 110.0,
            },
        ],
        Cue::Stop => vec![
            Note {
                hz: 880.0,
                at_ms: 0.0,
                len_ms: 90.0,
            },
            Note {
                hz: 587.33,
                at_ms: 70.0,
                len_ms: 110.0,
            },
        ],
        Cue::Cancel => vec![Note {
            hz: 392.0,
            at_ms: 0.0,
            len_ms: 120.0,
        }],
    }
}

/// The gain of a note at `t_ms` into its own length: a linear attack to `PEAK` over `ATTACK_MS`, then a linear
/// release to zero at the note's end. Zero outside the note.
pub fn envelope(t_ms: f64, len_ms: f64) -> f32 {
    if !(0.0..len_ms).contains(&t_ms) {
        return 0.0;
    }
    if t_ms < ATTACK_MS {
        return (PEAK as f64 * t_ms / ATTACK_MS) as f32;
    }
    (PEAK as f64 * (len_ms - t_ms) / (len_ms - ATTACK_MS)) as f32
}

/// The cue as mono float samples at `rate`: each note is a sine under its envelope, summed into one buffer as
/// long as the latest note ends.
pub fn synth(c: Cue, rate: u32) -> Vec<f32> {
    let total_ms = notes(c)
        .iter()
        .map(|n| n.at_ms + n.len_ms)
        .fold(0.0_f64, f64::max);
    let mut out = vec![0.0_f32; (total_ms * f64::from(rate) / 1000.0).ceil() as usize];
    for note in notes(c) {
        let start = (note.at_ms * f64::from(rate) / 1000.0).round() as usize;
        let len = (note.len_ms * f64::from(rate) / 1000.0).round() as usize;
        for n in 0..len {
            let t_ms = n as f64 * 1000.0 / f64::from(rate);
            let sine = (2.0 * std::f64::consts::PI * note.hz * t_ms / 1000.0).sin();
            let i = start + n;
            if i < out.len() {
                out[i] += envelope(t_ms, note.len_ms) * sine as f32;
            }
        }
    }
    out
}

/// Plays a cue. Returns at once: the WAV goes to the worker thread, which hands it to Windows.
pub fn play(c: Cue) {
    play_with(c, play_memory);
}

/// The sink a played cue's bytes go to. The real one is `play_memory`; a test passes its own.
pub type Sink = fn(&[u8]);

/// Sends a cue to `sink` on the worker thread, which synthesises and encodes it there. Never blocks the caller.
pub fn play_with(c: Cue, sink: Sink) {
    let _ = worker().send((c, sink));
}

/// The one worker that owns the bytes of the cue being played. Windows reads an `SND_MEMORY` buffer until the
/// sound stops or the next one starts, so the worker keeps the last buffer alive and swaps it only after the
/// next sink call has taken over.
fn worker() -> &'static std::sync::mpsc::Sender<(Cue, Sink)> {
    static WORKER: std::sync::OnceLock<std::sync::mpsc::Sender<(Cue, Sink)>> =
        std::sync::OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(Cue, Sink)>();
        let _ = std::thread::Builder::new()
            .name("flare-cues".into())
            .spawn(move || {
                let mut held: Vec<u8> = Vec::new();
                for (c, sink) in rx {
                    let bytes = crate::wav::encode_wav(&synth(c, PLAY_RATE), PLAY_RATE);
                    let old = std::mem::replace(&mut held, bytes);
                    sink(&held);
                    drop(old);
                }
            });
        tx
    })
}

/// Hands the WAV to Windows, asynchronously, with no default sound on failure.
fn play_memory(bytes: &[u8]) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    // SAFETY: with SND_MEMORY the buffer is read until the sound stops or the next play. The worker keeps `bytes`
    // alive for exactly that span (see `worker`), and the flags ask for no window, no module and no default.
    unsafe {
        PlaySoundW(
            bytes.as_ptr().cast(),
            std::ptr::null_mut(),
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16000;

    /// The power of `samples` at `hz`, by the Goertzel recurrence (cheap, and exact for one tone).
    fn goertzel(samples: &[f32], rate: u32, hz: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * hz / f64::from(rate);
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0_f64, 0.0_f64);
        for &x in samples {
            let s = f64::from(x) + coeff * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    /// The samples from `from_ms` to `to_ms` of a cue at `RATE`.
    fn window(samples: &[f32], from_ms: f64, to_ms: f64) -> &[f32] {
        let a = (from_ms * f64::from(RATE) / 1000.0) as usize;
        let b = ((to_ms * f64::from(RATE) / 1000.0) as usize).min(samples.len());
        &samples[a..b]
    }

    #[test]
    fn start_has_two_tones_at_the_pt_frequencies() {
        let s = synth(Cue::Start, RATE);
        // The first tone (587.33 Hz) sounds alone from 10 to 60 ms; the second (880 Hz) from 100 to 170 ms.
        let first = window(&s, 10.0, 60.0);
        assert!(goertzel(first, RATE, 587.33) > 20.0 * goertzel(first, RATE, 880.0));
        let second = window(&s, 100.0, 170.0);
        assert!(goertzel(second, RATE, 880.0) > 20.0 * goertzel(second, RATE, 587.33));
    }

    #[test]
    fn peak_is_0_12_and_attack_is_12_ms() {
        assert!((envelope(12.0, 90.0) - PEAK).abs() < 1e-6);
        assert!(envelope(0.0, 90.0).abs() < 1e-6);
        assert!(envelope(6.0, 90.0) > 0.05 && envelope(6.0, 90.0) < 0.07);
        assert!(envelope(11.9, 90.0) < PEAK);
        assert!(envelope(90.0, 90.0).abs() < 1e-6);
        // A single note never exceeds the peak.
        let c = synth(Cue::Cancel, RATE);
        let max = c.iter().fold(0.0_f32, |m, &x| m.max(x.abs()));
        assert!(max <= PEAK + 1e-4, "cancel peaks at {max}");
        assert!(max > 0.10, "cancel peaks at {max}, too quiet");
    }

    #[test]
    fn stop_is_start_reversed() {
        // The same timing, the two frequencies swapped: the stop rises to 880 Hz first and falls to 587.33 Hz.
        let s = synth(Cue::Stop, RATE);
        let first = window(&s, 10.0, 60.0);
        assert!(goertzel(first, RATE, 880.0) > 20.0 * goertzel(first, RATE, 587.33));
        let second = window(&s, 100.0, 170.0);
        assert!(goertzel(second, RATE, 587.33) > 20.0 * goertzel(second, RATE, 880.0));
        assert_eq!(
            notes(Cue::Stop),
            vec![
                Note {
                    hz: 880.0,
                    at_ms: 0.0,
                    len_ms: 90.0
                },
                Note {
                    hz: 587.33,
                    at_ms: 70.0,
                    len_ms: 110.0
                },
            ]
        );
    }

    #[test]
    fn cancel_is_one_392_tone_of_120_ms() {
        assert_eq!(
            notes(Cue::Cancel),
            vec![Note {
                hz: 392.0,
                at_ms: 0.0,
                len_ms: 120.0
            }]
        );
        let s = synth(Cue::Cancel, RATE);
        assert_eq!(s.len(), (0.120 * f64::from(RATE)).ceil() as usize);
        assert!(goertzel(window(&s, 20.0, 100.0), RATE, 392.0) > 500.0);
    }

    static SINK_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_sink(bytes: &[u8]) {
        assert_eq!(&bytes[0..4], b"RIFF");
        SINK_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn silent_sink(_: &[u8]) {}

    #[test]
    fn play_returns_at_once() {
        // Warm the worker up first, so the timed call is the send and not the thread's start.
        play_with(Cue::Start, silent_sink);
        let t = std::time::Instant::now();
        play_with(Cue::Stop, silent_sink);
        play_with(Cue::Cancel, silent_sink);
        assert!(
            t.elapsed() < std::time::Duration::from_millis(1),
            "play took {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn played_cue_is_a_riff_wav_handed_to_the_sink() {
        play_with(Cue::Start, count_sink);
        // The worker is asynchronous: wait for it, bounded.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while SINK_CALLS.load(std::sync::atomic::Ordering::SeqCst) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(SINK_CALLS.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    }

    #[test]
    #[ignore = "plays a sound"]
    fn play_plays_the_start_cue() {
        play(Cue::Start);
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}
