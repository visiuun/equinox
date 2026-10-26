//! Live spectrum of whatever is playing, via WASAPI loopback on the default
//! output. Capture runs on its own thread; the UI pulls a spectrum per frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};

const N: usize = 4096;

struct Ring {
    buf: Vec<f32>,
    pos: usize,
    rate: u32,
    last: Instant,
}

pub struct Analyzer {
    ring: Arc<Mutex<Ring>>,
    stop: Arc<AtomicBool>,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Analyzer {
    pub fn start() -> Analyzer {
        let ring = Arc::new(Mutex::new(Ring {
            buf: vec![0.0; N],
            pos: 0,
            rate: 48_000,
            last: Instant::now(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (r, s) = (ring.clone(), stop.clone());
        std::thread::Builder::new()
            .name("loopback".into())
            .spawn(move || capture_loop(r, s))
            .ok();
        let window = (0..N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos())
            .collect();
        Analyzer {
            ring,
            stop,
            window,
            re: vec![0.0; N],
            im: vec![0.0; N],
        }
    }

    /// Level in dB at each frequency (pink-tilted +3 dB/oct around 1 kHz so
    /// music looks level), or `None` when nothing has played for a moment.
    pub fn spectrum(&mut self, freqs: &[f32]) -> Option<Vec<f32>> {
        let rate = {
            let ring = self.ring.lock().ok()?;
            if ring.last.elapsed() > Duration::from_millis(400) {
                return None;
            }
            for i in 0..N {
                self.re[i] = ring.buf[(ring.pos + i) % N] * self.window[i];
            }
            ring.rate as f32
        };
        self.im.iter_mut().for_each(|v| *v = 0.0);
        fft(&mut self.re, &mut self.im);
        let norm = 2.0 / (N as f32 * 0.5);
        let bin_hz = rate / N as f32;
        let mag = |k: usize| (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * norm;
        let mut out = Vec::with_capacity(freqs.len());
        for (i, &f) in freqs.iter().enumerate() {
            // Each display point covers half-way to its neighbours.
            let lo = if i == 0 { f } else { (f * freqs[i - 1]).sqrt() };
            let hi = if i + 1 == freqs.len() { f } else { (f * freqs[i + 1]).sqrt() };
            let (k0, k1) = (
                ((lo / bin_hz).floor() as usize).max(1),
                ((hi / bin_hz).ceil() as usize).min(N / 2 - 1),
            );
            let m = if k1 <= k0 {
                // Below bin resolution: interpolate.
                let x = f / bin_hz;
                let k = (x.floor() as usize).clamp(1, N / 2 - 2);
                let t = x - k as f32;
                mag(k) * (1.0 - t) + mag(k + 1) * t
            } else {
                (k0..=k1).map(mag).fold(0.0, f32::max)
            };
            out.push(20.0 * m.max(1e-9).log10() + 3.0 * (f / 1000.0).log2());
        }
        Some(out)
    }
}

impl Drop for Analyzer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn capture_loop(ring: Arc<Mutex<Ring>>, stop: Arc<AtomicBool>) {
    crate::sys::com_init();
    while !stop.load(Ordering::Relaxed) {
        // Re-opens on default device changes and after errors (device unplugged...).
        let _ = unsafe { capture_session(&ring, &stop) };
        std::thread::sleep(Duration::from_millis(500));
    }
}

unsafe fn capture_session(ring: &Mutex<Ring>, stop: &AtomicBool) -> windows::core::Result<()> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let dev_id = dev.GetId()?.to_string().unwrap_or_default();
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = client.GetMixFormat()?;
        let (channels, bits, rate) = ((*fmt).nChannels as usize, (*fmt).wBitsPerSample, (*fmt).nSamplesPerSec);
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 1_000_000, 0, fmt, None);
        CoTaskMemFree(Some(fmt as _));
        init?;
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        if let Ok(mut r) = ring.lock() {
            r.rate = rate;
        }
        let mut last_check = Instant::now();
        let mut mono = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let mut n = cap.GetNextPacketSize()?;
            while n > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                mono.clear();
                let fr = frames as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() || channels == 0 {
                    mono.resize(fr, 0.0);
                } else if bits == 32 {
                    let s = std::slice::from_raw_parts(data as *const f32, fr * channels);
                    mono.extend(s.chunks_exact(channels).map(|c| c.iter().sum::<f32>() / channels as f32));
                } else if bits == 16 {
                    let s = std::slice::from_raw_parts(data as *const i16, fr * channels);
                    mono.extend(
                        s.chunks_exact(channels)
                            .map(|c| c.iter().map(|&v| v as f32 / 32768.0).sum::<f32>() / channels as f32),
                    );
                }
                cap.ReleaseBuffer(frames)?;
                if let Ok(mut r) = ring.lock() {
                    for &v in &mono {
                        let p = r.pos;
                        r.buf[p] = v;
                        r.pos = (p + 1) % N;
                    }
                    if !mono.is_empty() {
                        r.last = Instant::now();
                    }
                }
                n = cap.GetNextPacketSize()?;
            }
            if last_check.elapsed() > Duration::from_secs(1) {
                last_check = Instant::now();
                let now = en
                    .GetDefaultAudioEndpoint(eRender, eConsole)
                    .and_then(|d| d.GetId())
                    .map(|p| p.to_string().unwrap_or_default());
                if now.ok().as_deref() != Some(dev_id.as_str()) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        let _ = client.Stop();
        Ok(())
    }
}

/// In-place iterative radix-2 FFT.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (ws, wc) = ang.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wc - ci * ws;
                ci = cr * ws + ci * wc;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fft_finds_sine() {
        let n = 1024;
        let mut re: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 64.0 * i as f32 / n as f32).sin())
            .collect();
        let mut im = vec![0.0; n];
        super::fft(&mut re, &mut im);
        let mags: Vec<f32> = (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let peak = mags.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(peak, 64);
    }
}
