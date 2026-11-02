//! Effects run in the daemon as layers. They only exist while on: with the effect off there
//! is no loop or process left (battery).

pub mod anim;
mod battery;
mod clock;
mod smart;
mod sun;
mod equalizer;
mod pulse;

use super::compositor::{Layer, PRIORITY_AMBIENT};
use super::device::Core;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// How often a paused effect checks whether the screen is back on.
const BLANK_NAP: Duration = Duration::from_secs(1);

pub const EQUALIZER: &str = "equalizer";
pub const PULSE: &str = "pulse";

/// The music effects: not saved, because they depend on something playing.
pub fn is_audio(name: &str) -> bool {
    name == EQUALIZER || name == PULSE
}

pub fn sun_default_place() -> (f64, f64) {
    sun::default_place()
}

pub fn available() -> Vec<&'static str> {
    [EQUALIZER, PULSE].into_iter().chain(anim::KINDS.iter().map(|&(n, _)| n)).collect()
}

pub struct Running {
    pub name: String,
    body: Body,
}

enum Body {
    /// A daemon loop.
    Task(JoinHandle<()>),
    /// Animated by the MCU; the daemon does nothing.
    Hardware,
}

impl Running {
    pub fn is_alive(&self) -> bool {
        match &self.body {
            Body::Task(t) => !t.is_finished(),
            Body::Hardware => true,
        }
    }

    pub fn is_hardware(&self) -> bool {
        matches!(self.body, Body::Hardware)
    }

    fn task(name: &str, task: JoinHandle<()>) -> Self {
        Self { name: name.into(), body: Body::Task(task) }
    }
}

/// `name` is a music effect or one of [`anim::KINDS`]. If the MCU can animate it and
/// `hardware_effects` is set, it is left to the MCU and the daemon spends nothing.
pub async fn start(name: &str, speed: anim::Speed, core: Arc<Mutex<Core>>) -> Option<Running> {
    if is_audio(name) {
        let name = if name == EQUALIZER { EQUALIZER } else { PULSE };
        return Some(Running::task(name, tokio::spawn(run_audio(core, name))));
    }
    let kind = anim::Kind::parse(name)?;
    if kind == anim::Kind::Sensors {
        return Some(Running::task(kind.name(), tokio::spawn(run_smart(core))));
    }
    {
        let mut c = core.lock().await;
        if c.config.hardware_effects
            && let Some(program) = anim::hardware(kind, speed, &c.comp.base, &c.config.options(kind.name()))
        {
            match c.set_hardware(program) {
                Ok(()) => {
                    eprintln!("halo: effect {} ({}) on, animated by the MCU", kind.name(), speed.name());
                    return Some(Running { name: kind.name().into(), body: Body::Hardware });
                }
                Err(e) => eprintln!("halo: effect {} in software ({e})", kind.name()),
            }
        }
    }
    Some(Running::task(kind.name(), tokio::spawn(run_anim(core, kind, speed))))
}

/// Stops the effect and removes its layer (or takes its program away from the MCU).
pub async fn stop(running: Running, core: &Mutex<Core>) {
    let mut core = match running.body {
        Body::Task(task) => {
            task.abort();
            let _ = task.await;
            let mut core = core.lock().await;
            core.comp.remove_layer(&running.name);
            core
        }
        Body::Hardware => {
            let mut core = core.lock().await;
            if let Err(e) = core.clear_hardware() {
                eprintln!("halo: removing the hardware effect: {e}");
            }
            core
        }
    };
    if let Err(e) = core.push() {
        eprintln!("halo: stopping {}: {e}", running.name);
    }
}

/// Paints the effect as a layer and sleeps until the next change. The base color is read on every
/// frame, so changing it while the effect is on shows immediately.
async fn run_anim(core: Arc<Mutex<Core>>, kind: anim::Kind, speed: anim::Speed) {
    let start = tokio::time::Instant::now();
    let mut failing = false;
    eprintln!("halo: effect {} ({}) on", kind.name(), speed.name());
    loop {
        let t = start.elapsed().as_secs_f64();
        {
            let mut core = core.lock().await;
            // Screen off: nothing is computed; checks again in a second.
            if core.comp.blank {
                drop(core);
                tokio::time::sleep(BLANK_NAP).await;
                continue;
            }
            let options = core.config.options(kind.name());
            let frame = anim::render(kind, speed, &core.comp.base, t, &options);
            core.comp.set_layer(kind.name(), Layer { priority: PRIORITY_AMBIENT, zones: frame.map(Some) });
            match core.push() {
                Ok(_) => failing = false,
                // A single warning, not 25 per second; the next frame retries.
                Err(e) if !failing => {
                    eprintln!("halo: effect {}: {e}", kind.name());
                    failing = true;
                }
                Err(_) => {}
            }
        }
        tokio::time::sleep_until(start + Duration::from_secs_f64(anim::next(kind, speed, t))).await;
    }
}

/// Reads the temperature and CPU usage every [`smart::EVERY`] s. The frame is only sent when it changes.
async fn run_smart(core: Arc<Mutex<Core>>) {
    let name = anim::Kind::Sensors.name();
    let sensor = smart::find_sensor();
    eprintln!("halo: effect {name} on (sensor {})", sensor.as_deref().map_or("none".into(), |p| p.display().to_string()));
    let mut prev = smart::read_cpu();
    let mut failing = false;
    let mut tick = tokio::time::interval(Duration::from_secs_f64(smart::EVERY));
    loop {
        tick.tick().await;
        if core.lock().await.comp.blank {
            prev = None; // the next read starts fresh, without averaging the pause
            continue;
        }
        let now = smart::read_cpu();
        let usage = prev.zip(now).and_then(|(a, b)| smart::usage(a, b));
        prev = now.or(prev);
        let frame = smart::frame(sensor.as_deref().and_then(smart::read_temp), usage);
        let mut core = core.lock().await;
        core.comp.set_layer(name, Layer { priority: PRIORITY_AMBIENT, zones: frame.map(Some) });
        match core.push() {
            Ok(_) => failing = false,
            Err(e) if !failing => {
                eprintln!("halo: effect {name}: {e}");
                failing = true;
            }
            Err(_) => {}
        }
    }
}

async fn run_audio(core: Arc<Mutex<Core>>, name: &'static str) {
    if let Err(e) = audio_loop(&core, name).await {
        eprintln!("halo: {name} stopped: {e}");
    }
    let mut core = core.lock().await;
    core.comp.remove_layer(name);
    let _ = core.push();
}

type AudioFx = dyn FnMut(&[f32]) -> halo_core::Frame + Send;

/// `name` is [`EQUALIZER`] (one band per zone) or [`PULSE`] (the bass on all 4).
async fn audio_loop(core: &Mutex<Core>, name: &'static str) -> std::io::Result<()> {
    use equalizer::{Equalizer, FPS, RATE};
    // The default output's monitor (PipeWire-Pulse). kill_on_drop: if the task is aborted, parec dies.
    let mut rec = Command::new("parec")
        .args(["--device=@DEFAULT_MONITOR@", "--format=s16le", &format!("--rate={RATE}"), "--channels=1", "--latency-msec=20", "--raw"])
        .stdout(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut out = rec.stdout.take().expect("stdout is piped");
    let eq = Equalizer::new((RATE / FPS) as usize);
    let block = eq.block();
    let colors = core.lock().await.config.options(name).colors;
    let mut process: Box<AudioFx> = if name == PULSE {
        let mut p = pulse::Pulse::new(eq, colors);
        Box::new(move |s| p.process(s))
    } else {
        let mut eq = eq;
        Box::new(move |s| eq.process(s))
    };
    let mut raw = vec![0u8; block * 2];
    let mut samples = vec![0f32; block];
    let mut last = None;
    eprintln!("halo: {name} on");
    loop {
        out.read_exact(&mut raw).await?;
        // Screen off: keep reading so parec does not stall, but compute nothing.
        if core.lock().await.comp.blank {
            last = None;
            continue;
        }
        for (s, b) in samples.iter_mut().zip(raw.as_chunks::<2>().0) {
            *s = i16::from_le_bytes(*b) as f32 / 32768.0;
        }
        let frame = process(&samples);
        if last == Some(frame) {
            continue; // in silence nothing is sent
        }
        last = Some(frame);
        let mut core = core.lock().await;
        core.comp.set_layer(name, Layer { priority: PRIORITY_AMBIENT, zones: frame.map(Some) });
        core.push()?;
    }
}
