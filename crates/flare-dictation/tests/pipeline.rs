//! Dictation end to end with nothing but the device faked: a WAV plays through the microphone's file seam
//! (`MicSource::File`, the `FLARE_E2E_MIC` seam) in real time, the controller runs every rule on what arrives, a fake
//! engine hears the WAV the controller sends, and the paste is what it heard, cleaned, plus one space. Runs on
//! Windows and on macOS alike (port phase 7): no sound card, no permission, no window.

use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use flare_dictation::clean::clean_transcript;
use flare_dictation::controller::{Controller, Effect, Event, Phase};
use flare_dictation::engine::{TranscribeRequest, TranscribeResult};
use flare_dictation::hotkey::{default_hotkey, DictMode, KeyAction};
use flare_dictation::mic::{self, Capture, MicSource, TARGET_RATE};
use flare_dictation::prefs::DictPrefs;
use flare_dictation::wav::{encode_wav, level};

const TAB: &str = "tab-1";
const SAID: &str = "hello from the mac";
const TONE_HZ: f64 = 440.0;
/// The clip, at the rate a Mac's built-in microphone runs (48 kHz), resampled to 16 kHz by the seam.
const CLIP_RATE: u32 = 48_000;
const CLIP_SECONDS: f64 = 1.2;

fn prefs() -> DictPrefs {
    DictPrefs {
        enabled: true,
        mode: DictMode::Hold,
        hotkey: default_hotkey(),
        mic: String::new(),
        language: "en".into(),
        pause_media: false,
        sounds: false,
        model: "tiny".into(),
        anywhere: true,
    }
}

fn tone_wav(dir: &std::path::Path) -> std::path::PathBuf {
    let n = (f64::from(CLIP_RATE) * CLIP_SECONDS) as usize;
    let clip: Vec<f32> = (0..n)
        .map(|i| {
            (0.4 * (2.0 * std::f64::consts::PI * TONE_HZ * i as f64 / f64::from(CLIP_RATE)).sin())
                as f32
        })
        .collect();
    let path = dir.join("speech.wav");
    std::fs::write(&path, encode_wav(&clip, CLIP_RATE)).unwrap();
    path
}

/// The pitch of a 16 kHz clip, from its rising zero crossings.
fn pitch(samples: &[f32]) -> f64 {
    let ups = samples
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    ups as f64 / (samples.len() as f64 / f64::from(TARGET_RATE))
}

/// The fake engine: it reads the WAV it is sent (16-bit mono, 44-byte header) and answers what was said only when
/// the audio is the tone that was played, at 16 kHz; anything else is heard as something else.
fn fake_engine(req: &TranscribeRequest) -> TranscribeResult {
    let (pcm, rate) = mic::read_wav(&req.wav).expect("the controller sends a WAV");
    assert_eq!(rate, TARGET_RATE, "the engines are sent 16 kHz");
    let speech: Vec<f32> = pcm.into_iter().filter(|s| s.abs() > 0.0).collect();
    let hz = pitch(&speech);
    if (hz - TONE_HZ).abs() < TONE_HZ * 0.05 {
        TranscribeResult::Ok(SAID.into())
    } else {
        TranscribeResult::Ok(format!("something at {hz:.0} Hz"))
    }
}

struct Run {
    c: Controller,
    started: Instant,
    all: Vec<Effect>,
}

impl Run {
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
    fn ev(&mut self, e: Event) -> Vec<Effect> {
        let now = self.now();
        let fx = self.c.on(e, now);
        self.all.extend(fx.iter().cloned());
        fx
    }
    /// Answers every pass the effects ask for, as the engine would, and feeds the answers back.
    fn serve(&mut self, fx: Vec<Effect>) {
        let mut todo = fx;
        while !todo.is_empty() {
            let mut next = Vec::new();
            for e in todo {
                if let Effect::Transcribe { id, req } = e {
                    let result = fake_engine(&req);
                    next.extend(self.ev(Event::Heard { id, result }));
                }
            }
            todo = next;
        }
    }
}

fn open_mic(
    path: std::path::PathBuf,
    device: Option<String>,
) -> (Box<dyn Capture>, Receiver<Vec<f32>>) {
    let (frames_tx, frames) = channel::<Vec<f32>>();
    let (done_tx, done) = channel();
    mic::start_capture_async(
        MicSource::File(path),
        device,
        Box::new(move |f: &[f32]| {
            let _ = frames_tx.send(f.to_vec());
        }),
        Box::new(move |r| {
            let _ = done_tx.send(r);
        }),
    );
    let cap = done
        .recv_timeout(Duration::from_secs(10))
        .expect("the open answered")
        .map_err(|f| f.text())
        .expect("the file seam opened");
    (cap, frames)
}

#[test]
fn a_wav_played_as_the_microphone_is_heard_resampled_and_pasted_into_the_tab_in_front() {
    let dir = tempfile::tempdir().unwrap();
    let wav = tone_wav(dir.path());
    let mut run = Run {
        c: Controller::new(prefs()),
        started: Instant::now(),
        all: Vec::new(),
    };
    run.c.set_sink(true);
    run.ev(Event::Prefs(prefs()));
    run.ev(Event::Front(Some(TAB.into())));

    // The press: the controller asks for the system default microphone.
    let fx = run.ev(Event::Key(KeyAction::Start));
    let device = fx
        .iter()
        .find_map(|e| match e {
            Effect::OpenMic(d) => Some(d.clone()),
            _ => None,
        })
        .expect("a press opens the microphone");
    assert_eq!(device, None, "no microphone chosen: the system default");
    let (cap, frames) = open_mic(wav, device);
    run.ev(Event::MicOpened);
    assert_eq!(run.c.view().phase, Phase::Listening);

    // The clip arrives in real time; the app's 350 ms tick asks for partials, which are only shown.
    let want = (CLIP_SECONDS * f64::from(TARGET_RATE)) as usize;
    let (mut heard, mut last_tick) = (0usize, Instant::now());
    let end = Instant::now() + Duration::from_secs(20);
    while heard < want {
        assert!(
            Instant::now() < end,
            "the clip did not arrive: {heard} of {want}"
        );
        let Ok(f) = frames.recv_timeout(Duration::from_secs(5)) else {
            panic!("the microphone went quiet after {heard} samples");
        };
        heard += f.len();
        let l = level(&f);
        run.ev(Event::Frame {
            level: l,
            samples: f,
        });
        if last_tick.elapsed() >= Duration::from_millis(350) {
            last_tick = Instant::now();
            let fx = run.ev(Event::PartialTick);
            run.serve(fx);
        }
    }
    assert!(
        run.started.elapsed() >= Duration::from_millis((CLIP_SECONDS * 1000.0) as u64 - 50),
        "faster than the audio plays"
    );

    // The release: the microphone closes and the whole clip is the final pass.
    let fx = run.ev(Event::Key(KeyAction::Stop));
    assert!(fx.iter().any(|e| matches!(e, Effect::CloseMic)));
    let clip = cap.stop();
    assert!(clip.len() >= want, "{} of {want}", clip.len());
    let final_req = fx
        .iter()
        .find_map(|e| match e {
            Effect::Transcribe { req, .. } if req.final_ => Some(req.clone()),
            _ => None,
        })
        .expect("the release asks for the final pass");
    let (sent, _) = mic::read_wav(&final_req.wav).unwrap();
    assert!(
        sent.len() >= want && (pitch(&sent[..want]) - TONE_HZ).abs() < TONE_HZ * 0.05,
        "the final pass is the clip as played, at 16 kHz"
    );
    run.serve(fx);

    // The paste: what the engine heard, cleaned, plus one space, into the tab in front at the start; never Enter.
    let pastes: Vec<(String, String)> = run
        .all
        .iter()
        .filter_map(|e| match e {
            Effect::Paste { session, text } => Some((session.clone(), text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        pastes,
        vec![(TAB.to_string(), format!("{} ", clean_transcript(SAID)))]
    );
    assert!(!pastes[0].1.contains('\n') && !pastes[0].1.contains('\r'));
    assert_eq!(run.c.view().phase, Phase::Idle);
    assert!(!run.c.recording());
}
