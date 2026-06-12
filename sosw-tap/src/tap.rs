use anyhow::Result;
use std::io::ErrorKind::WouldBlock;
use tappers::{Interface, Tap};

pub struct TapInterface {
    tap: Tap,
}

impl TapInterface {
    pub fn create(name: Option<&str>) -> Result<Self> {
        let mut tap = match name {
            Some(n) => Tap::new_named(Interface::new(n)?)?,
            None => Tap::new()?,
        };
        tap.set_up()?;
        Ok(Self { tap })
    }

    pub fn iface_name(&self) -> Result<String> {
        let iface = self.tap.name()?;
        Ok(iface.name().to_string_lossy().into_owned())
    }

    pub fn send(&self, buf: &[u8]) -> Result<usize> {
        Ok(self.tap.send(buf)?)
    }

    pub fn recv(&self, buf: &mut [u8]) -> Result<Option<usize>> {
        match self.tap.recv(buf) {
            Ok(n) => Ok(Some(n)),
            Err(e) if e.kind() == WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_nonblocking(&mut self, nb: bool) -> Result<()> {
        self.tap.set_nonblocking(nb).map_err(Into::into)
    }

    pub fn name_string(&self) -> String {
        self.iface_name().unwrap_or_else(|_| "sosw".into())
    }
}
