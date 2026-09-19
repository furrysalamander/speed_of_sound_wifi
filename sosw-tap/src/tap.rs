use anyhow::Result;
use std::io::ErrorKind::WouldBlock;
use tappers::{Interface, Tap};

/// Trait abstracting a TAP-like network interface for testing.
pub trait Tappable: Send + Sync {
    fn iface_name(&self) -> Result<String>;
    fn send(&self, buf: &[u8]) -> Result<usize>;
    fn recv(&self, buf: &mut [u8]) -> Result<Option<usize>>;
    fn set_nonblocking(&mut self, nb: bool) -> Result<()>;
    fn name_string(&self) -> String {
        self.iface_name().unwrap_or_else(|_| "sosw".into())
    }
}

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
        tap.set_nonblocking(true)?;
        Ok(Self { tap })
    }
}

impl Tappable for TapInterface {
    fn iface_name(&self) -> Result<String> {
        let iface = self.tap.name()?;
        Ok(iface.name().to_string_lossy().into_owned())
    }

    fn send(&self, buf: &[u8]) -> Result<usize> {
        Ok(self.tap.send(buf)?)
    }

    fn recv(&self, buf: &mut [u8]) -> Result<Option<usize>> {
        match self.tap.recv(buf) {
            Ok(n) => Ok(Some(n)),
            Err(e) if e.kind() == WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set_nonblocking(&mut self, nb: bool) -> Result<()> {
        self.tap.set_nonblocking(nb).map_err(Into::into)
    }
}

/// A mock TAP interface for testing. Packets sent through it are appended to
/// an internal queue and can be retrieved by the test harness.
#[derive(Clone)]
pub struct MockTap {
    name: String,
    recv_queue: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<Vec<u8>>>>,
    sent_queue: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<Vec<u8>>>>,
}

impl MockTap {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            recv_queue: std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            sent_queue: std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        }
    }

    pub fn inject(&self, packet: Vec<u8>) {
        self.recv_queue.lock().unwrap().push_back(packet);
    }

    pub fn sent(&self) -> Option<Vec<u8>> {
        self.sent_queue.lock().unwrap().pop_front()
    }

    pub fn sent_count(&self) -> usize {
        self.sent_queue.lock().unwrap().len()
    }
}

impl Tappable for MockTap {
    fn iface_name(&self) -> Result<String> {
        Ok(self.name.clone())
    }

    fn send(&self, buf: &[u8]) -> Result<usize> {
        self.sent_queue.lock().unwrap().push_back(buf.to_vec());
        Ok(buf.len())
    }

    fn recv(&self, buf: &mut [u8]) -> Result<Option<usize>> {
        if let Some(pkt) = self.recv_queue.lock().unwrap().pop_front() {
            let n = pkt.len().min(buf.len());
            buf[..n].copy_from_slice(&pkt[..n]);
            Ok(Some(n))
        } else {
            Ok(None)
        }
    }

    fn set_nonblocking(&mut self, _nb: bool) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_tap_send_recv() {
        let tap = MockTap::new("mock0");
        assert_eq!(tap.iface_name().unwrap(), "mock0");

        tap.inject(vec![1, 2, 3, 4]);
        let mut buf = [0u8; 10];
        assert_eq!(tap.recv(&mut buf).unwrap(), Some(4));
        assert_eq!(&buf[..4], &[1, 2, 3, 4]);

        assert_eq!(tap.send(&[5, 6, 7]).unwrap(), 3);
        assert_eq!(tap.sent(), Some(vec![5, 6, 7]));
    }
}
