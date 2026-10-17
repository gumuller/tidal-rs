//! App-specific PCM capture and a 4096-point, Hann-windowed FFT. No desktop mix.
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
pub const BANDS: usize = 64;
const N: usize = 4096;
const HOP: usize = 800; // 48 kHz / 60 spectra per second.
pub const RATE: f32 = 48_000.0;
#[derive(Clone)]
pub struct Frame {
    pub bins: [f32; BANDS],
    pub sequence: u64,
    pub at: Option<Instant>,
    pub status: String,
}
impl Default for Frame {
    fn default() -> Self {
        Self {
            bins: [0.0; BANDS],
            sequence: 0,
            at: None,
            status: "Waiting for playback".into(),
        }
    }
}
pub fn shared() -> Arc<Mutex<Frame>> {
    static FRAME: OnceLock<Arc<Mutex<Frame>>> = OnceLock::new();
    FRAME
        .get_or_init(|| Arc::new(Mutex::new(Frame::default())))
        .clone()
}
pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    samples: Vec<f32>,
    buffer: Vec<Complex<f32>>,
    window: Vec<f32>,
    scratch: Vec<Complex<f32>>,
}
impl Analyzer {
    pub fn new() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(N);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        Self {
            fft,
            samples: vec![0.0; N],
            buffer: vec![Complex::default(); N],
            window: (0..N)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (N - 1) as f32).cos())
                .collect(),
            scratch,
        }
    }
    pub fn push(&mut self, input: &[f32]) -> [f32; BANDS] {
        let count = input.len().min(N);
        self.samples.copy_within(count.., 0);
        self.samples[N - count..].copy_from_slice(&input[input.len() - count..]);
        for ((out, sample), window) in self.buffer.iter_mut().zip(&self.samples).zip(&self.window) {
            *out = Complex::new(
                if sample.is_finite() {
                    sample * window
                } else {
                    0.0
                },
                0.0,
            );
        }
        self.fft
            .process_with_scratch(&mut self.buffer, &mut self.scratch);
        std::array::from_fn(|band| {
            let lo = (24.0_f32 * (18000.0_f32 / 24.0).powf(band as f32 / BANDS as f32) * N as f32
                / RATE)
                .floor()
                .max(1.0) as usize;
            let hi =
                (24.0_f32 * (18000.0_f32 / 24.0).powf((band + 1) as f32 / BANDS as f32) * N as f32
                    / RATE)
                    .ceil()
                    .max((lo + 1) as f32) as usize;
            let amplitude = self.buffer[lo..hi.min(N / 2)]
                .iter()
                .map(|v| v.norm() * 4.0 / N as f32)
                .fold(0.0_f32, f32::max);
            ((20.0 * amplitude.max(1e-8).log10() + 75.0) / 75.0).clamp(0.0, 1.0)
        })
    }
}
pub struct Monitor {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Monitor {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None::<Child>));
        let cancel = stop.clone();
        let process = child.clone();
        let frame = shared();
        *frame.lock().unwrap() = Frame::default();
        let worker = thread::spawn(move || {
            let name = format!("tidal-rs-{}", std::process::id());
            let mut analyzer = Analyzer::new();
            while !cancel.load(Ordering::Relaxed) {
                let inputs = Command::new("pactl")
                    .args(["-f", "json", "list", "sink-inputs"])
                    .output()
                    .ok()
                    .and_then(|r| serde_json::from_slice::<serde_json::Value>(&r.stdout).ok());
                let input = inputs.as_ref().and_then(|v| v.as_array()).and_then(|a| {
                    a.iter()
                        .find(|v| v["properties"]["application.name"].as_str() == Some(&name))
                });
                let Some(input) = input else {
                    thread::sleep(Duration::from_millis(200));
                    continue;
                };
                let Some(index) = input["index"].as_u64() else {
                    continue;
                };
                let sink = input["sink"].as_u64().unwrap_or(0);
                let sinks = Command::new("pactl")
                    .args(["-f", "json", "list", "sinks"])
                    .output()
                    .ok()
                    .and_then(|r| serde_json::from_slice::<serde_json::Value>(&r.stdout).ok());
                let source = sinks
                    .as_ref()
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.iter().find(|v| v["index"].as_u64() == Some(sink)))
                    .and_then(|s| s["name"].as_str())
                    .map(|name| format!("{name}.monitor"));
                let Some(source) = source else {
                    thread::sleep(Duration::from_millis(200));
                    continue;
                };
                let result = Command::new("parec")
                    .args([
                        "--raw",
                        "--format=float32le",
                        "--rate=48000",
                        "--channels=2",
                        "--latency-msec=20",
                        "--process-time-msec=10",
                        "--client-name=TIDAL Spectrum",
                        &format!("--monitor-stream={index}"),
                        &format!("--device={source}"),
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn();
                let Ok(mut recorder) = result else {
                    frame.lock().unwrap().status = "Spectrum requires parec".into();
                    break;
                };
                let mut pcm = recorder.stdout.take().unwrap();
                *process.lock().unwrap() = Some(recorder);
                let mut bytes = vec![0u8; HOP * 8];
                let mut samples = vec![0f32; HOP];
                while !cancel.load(Ordering::Relaxed) && pcm.read_exact(&mut bytes).is_ok() {
                    for (sample, pair) in samples.iter_mut().zip(bytes.chunks_exact(8)) {
                        let l = f32::from_le_bytes(pair[0..4].try_into().unwrap());
                        let r = f32::from_le_bytes(pair[4..8].try_into().unwrap());
                        *sample = (l + r) * 0.5;
                    }
                    let bins = analyzer.push(&samples);
                    let mut output = frame.lock().unwrap();
                    output.bins = bins;
                    output.sequence += 1;
                    output.at = Some(Instant::now());
                    output.status = "48 kHz · FFT 4096 · 60 Hz".into();
                }
                if let Some(mut child) = process.lock().unwrap().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                frame.lock().unwrap().bins = [0.0; BANDS];
                thread::sleep(Duration::from_millis(100));
            }
        });
        Self {
            stop,
            child,
            worker: Some(worker),
        }
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        *shared().lock().unwrap() = Frame::default();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_frequencies_and_silence() {
        fn tone(hz: f32) -> [f32; BANDS] {
            let mut a = Analyzer::new();
            let signal: Vec<_> = (0..N)
                .map(|i| (std::f32::consts::TAU * hz * i as f32 / RATE).sin() * 0.5)
                .collect();
            a.push(&signal)
        }
        let low = tone(100.0);
        let high = tone(4000.0);
        let peak = |v: &[f32; BANDS]| {
            v.iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0
        };
        assert!(peak(&low) < peak(&high));
        assert!(
            (24.0 * (18000.0_f32 / 24.0).powf((peak(&high) as f32 + 0.5) / BANDS as f32) - 4000.0)
                .abs()
                < 500.0
        );
        assert!(Analyzer::new().push(&[0.0; N]).iter().all(|v| *v == 0.0));
    }
    #[test]
    fn sixty_hops_cover_one_second() {
        assert_eq!(HOP * 60, RATE as usize);
    }
}
