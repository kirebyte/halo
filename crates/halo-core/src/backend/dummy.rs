//! Backend without hardware: records what it receives (tests, CI, `--backend dummy`).

use super::Backend;
use crate::Frame;
use std::io;

#[derive(Default)]
pub struct Dummy {
    pub frames: Vec<Frame>,
    pub brightness: Option<u8>,
    /// Print every frame to stdout.
    pub echo: bool,
}

impl Backend for Dummy {
    fn name(&self) -> &'static str {
        "dummy"
    }

    fn apply(&mut self, frame: &Frame) -> io::Result<()> {
        if self.echo {
            let z: Vec<String> = frame.iter().map(|c| c.to_string()).collect();
            println!("dummy: {}", z.join(" "));
        }
        self.frames.push(*frame);
        Ok(())
    }

    fn set_brightness(&mut self, level: u8) -> io::Result<()> {
        self.brightness = Some(level);
        Ok(())
    }
}
