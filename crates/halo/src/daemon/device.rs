//! The daemon's live state and the device, reconnecting if the hidraw changed.

use super::compositor::Compositor;
use super::config::{self, Config};
use halo_core::backend::{self, Backend, Dummy, HwProgram, Vendor};
use halo_core::state::{self, State};
use halo_core::{Frame, Rgb};
use std::io;

pub struct Core {
    pub comp: Compositor,
    pub config: Config,
    pub brightness: u8,
    backend_name: String,
    dev: Option<Box<dyn Backend>>,
    /// Last frame that reached the MCU, so the same one is not sent twice.
    sent: Option<Frame>,
    /// What the MCU animates on its own (hardware effect). Visible when nothing needs software painting.
    hardware: Option<HwProgram>,
    /// The MCU is in autonomous mode showing `hardware`.
    autonomous: bool,
}

impl Core {
    pub fn new(backend_name: &str, initial: State) -> Self {
        let mut comp = Compositor::new(initial.frame);
        let config = config::load();
        comp.games_enabled = config.games;
        comp.blocked = config.blocked.iter().map(|g| g.to_lowercase()).collect();
        comp.dark = initial.brightness == 0;
        Self {
            comp,
            config,
            brightness: initial.brightness,
            backend_name: backend_name.to_string(),
            dev: None,
            sent: None,
            hardware: None,
            autonomous: false,
        }
    }

    /// Programs an animation into the MCU and hands it control if nothing needs software. Fails
    /// without LampArray + vendor (e.g. with the dummy backend): the caller stays in software.
    pub fn set_hardware(&mut self, program: HwProgram) -> io::Result<()> {
        if self.backend_name == "dummy" {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "the dummy backend does not animate"));
        }
        if self.dev.is_none() {
            self.try_push(&self.comp.output())?;
        }
        if self.backend() != "lamparray" {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "hardware effects need LampArray"));
        }
        Vendor::open()?.program(&program)?;
        self.hardware = Some(program);
        self.push().map(|_| ())
    }

    pub fn clear_hardware(&mut self) -> io::Result<()> {
        self.hardware = None;
        self.push().map(|_| ())
    }


    pub fn backend(&self) -> &str {
        self.dev.as_ref().map(|d| d.name()).unwrap_or("none")
    }

    fn open(&self) -> io::Result<Box<dyn Backend>> {
        match self.backend_name.as_str() {
            "auto" => backend::open_auto(),
            "lamparray" => backend::LampArray::open().map(|b| Box::new(b) as _),
            "vendor" => backend::Vendor::open().map(|b| Box::new(b) as _),
            "dummy" => Ok(Box::new(Dummy { echo: true, ..Default::default() })),
            other => Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unknown backend: {other}"))),
        }
    }

    /// Reopens the device, sets the brightness and sends the frame (startup, wake-up, error).
    /// A hardware effect is reprogrammed: the MCU may not keep it across wake-up.
    pub fn reclaim(&mut self) -> io::Result<Frame> {
        self.dev = None;
        self.sent = None;
        self.autonomous = false;
        if let Some(p) = self.hardware.clone() {
            self.try_push(&self.comp.output())?;
            if let Err(e) = Vendor::open().and_then(|mut v| v.program(&p)) {
                eprintln!("halo: could not reprogram the hardware effect ({e})");
            }
        }
        self.push()
    }

    /// Sends the compositor output if it changed. On failure, reopens once (the hidraw number changes).
    /// With a hardware effect and nothing on top, it leaves the rings to the MCU instead of painting.
    pub fn push(&mut self) -> io::Result<Frame> {
        let frame = self.comp.output();
        if self.hardware.is_some() && !self.comp.needs_software() {
            if !self.autonomous {
                if self.dev.is_none() {
                    self.try_push(&frame)?;
                }
                self.dev.as_mut().expect("open").release()?;
                self.autonomous = true;
                self.sent = None;
            }
            return Ok(frame);
        }
        self.autonomous = false;
        if self.sent == Some(frame) && self.dev.is_some() {
            return Ok(frame);
        }
        match self.try_push(&frame) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("halo: {e}; reopening the device");
                self.dev = None;
                self.try_push(&frame)?;
            }
        }
        self.sent = Some(frame);
        Ok(frame)
    }

    fn try_push(&mut self, frame: &Frame) -> io::Result<()> {
        if self.dev.is_none() {
            let mut d = self.open()?;
            if let Err(e) = d.set_brightness(self.brightness) {
                eprintln!("halo: warning: could not set the hardware brightness ({e})");
            }
            eprintln!("halo: device opened ({})", d.name());
            self.dev = Some(d);
        }
        self.dev.as_mut().expect("just opened").apply(frame)
    }

    /// Hardware brightness 0-3. At 0 the rings are also painted off, whatever games or effects
    /// want, so "Rings off" holds even where the MCU ignores the vendor brightness command.
    pub fn set_brightness(&mut self, level: u8) -> io::Result<Frame> {
        self.brightness = level;
        self.comp.dark = level == 0;
        if let Some(d) = self.dev.as_mut() {
            d.set_brightness(level)?;
        }
        self.save();
        self.push()
    }

    pub fn set_base(&mut self, frame: Frame) -> io::Result<Frame> {
        self.comp.base = frame;
        self.save();
        self.push()
    }

    /// "Games control the lights": saved and applied immediately.
    pub fn set_games(&mut self, on: bool) -> io::Result<Frame> {
        self.comp.games_enabled = on;
        self.config.games = on;
        self.save_config()?;
        self.push()
    }

    /// Allows or blocks a game (by name, case-insensitive) and saves it.
    pub fn set_game_allowed(&mut self, game: &str, allowed: bool) -> io::Result<Frame> {
        let key = game.to_lowercase();
        if allowed {
            self.comp.blocked.remove(&key);
            self.config.blocked.retain(|g| g.to_lowercase() != key);
        } else {
            self.comp.blocked.insert(key.clone());
            if !self.config.blocked.iter().any(|g| g.to_lowercase() == key) {
                self.config.blocked.insert(game.to_string());
            }
        }
        self.save_config()?;
        self.push()
    }

    pub fn save_config(&self) -> io::Result<()> {
        if self.backend_name == "dummy" {
            return Ok(());
        }
        config::save(&self.config)
    }

    /// Releases control: the firmware takes over again until the next change.
    pub fn release(&mut self) -> io::Result<()> {
        if let Some(mut d) = self.dev.take() {
            d.release()?;
        }
        self.sent = None;
        Ok(())
    }

    fn save(&self) {
        if self.backend_name == "dummy" {
            return;
        }
        let s = State { frame: self.comp.base, brightness: self.brightness };
        if let Err(e) = state::save(&s) {
            eprintln!("halo: could not save the state: {e}");
        }
    }
}

pub fn hex(frame: &Frame) -> Vec<String> {
    frame.iter().map(Rgb::to_string).collect()
}
