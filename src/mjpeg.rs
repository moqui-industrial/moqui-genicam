//! Pull one JPEG for each HTTP body poll. No ExecutionContext escapes its request thread.
use crate::{client::decode_bridge_bytes, worker::WorkerPool};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Mjpeg {
    pub pool: Arc<WorkerPool>,
    pub python: PathBuf,
    pub script: PathBuf,
    pub camera: (String, String),
    pub payload: serde_json::Value,
    pub timeout: Duration,
    pub delay: Duration,
    pub remaining: Option<usize>,
    pub first: Option<Vec<u8>>,
    pub last: Option<Instant>,
    pub ended: bool,
}

impl Mjpeg {
    pub fn frame(&self) -> std::io::Result<Vec<u8>> {
        let response = self
            .pool
            .exchange(
                self.camera.clone(),
                &self.python,
                &self.script,
                &self.payload,
                self.timeout,
            )
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if let Some(error) = response.get("error") {
            return Err(std::io::Error::other(error.to_string()));
        }
        let bytes = decode_bridge_bytes(
            response
                .get("jpeg_bytes")
                .ok_or_else(|| std::io::Error::other("camera returned no JPEG buffer"))?,
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?;
        if bytes.len() > 32 * 1024 * 1024
            || !bytes.starts_with(&[0xff, 0xd8])
            || !bytes.ends_with(&[0xff, 0xd9])
        {
            return Err(std::io::Error::other("invalid or oversized camera JPEG"));
        }
        Ok(bytes)
    }
}

impl Iterator for Mjpeg {
    type Item = std::io::Result<Vec<u8>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }
        if self.remaining == Some(0) {
            self.ended = true;
            return Some(Ok(b"--moqui-frame--\r\n".to_vec()));
        }
        if let Some(last) = self.last {
            std::thread::sleep(self.delay.saturating_sub(last.elapsed()));
        }
        let frame = match self.first.take().map(Ok).unwrap_or_else(|| self.frame()) {
            Ok(frame) => frame,
            Err(error) => {
                self.ended = true;
                return Some(Err(error));
            }
        };
        self.last = Some(Instant::now());
        if let Some(remaining) = &mut self.remaining {
            *remaining -= 1;
        }
        let mut part = format!(
            "--moqui-frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            frame.len()
        )
        .into_bytes();
        part.extend(frame);
        part.extend(b"\r\n");
        Some(Ok(part))
    }
}
