use hbb_common::{
    bytes::Bytes,
    sodiumoxide::crypto::secretbox,
    Stream,
    tokio::sync::watch,
};

pub fn stream_keys(stream: &Stream) -> Option<(secretbox::Key, secretbox::Key)> {
    match stream {
        Stream::Tcp(s) => s.2.as_ref().map(|encrypt| {
            (encrypt.0.clone(), encrypt.0.clone())
        }),
        Stream::WebRTC(_) => None,
        Stream::WebSocket(_) => None,
    }
}
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub const MAGIC: &[u8; 4] = b"RDMV";
pub const VERSION: u8 = 1;
pub const FRAGMENT_SIZE: usize = 1050;
pub const HEADER_LEN: usize = 4 + 1 + 8 + 2 + 2 + secretbox::NONCEBYTES;

pub struct MediaChannel {
    tx: watch::Sender<Bytes>,
    enabled: AtomicBool,
    raw: bool,
}

impl MediaChannel {
    pub fn new(raw: bool) -> (Arc<Self>, watch::Receiver<Bytes>) {
        let (tx, rx) = watch::channel(Bytes::new());
        (
            Arc::new(Self {
                tx,
                enabled: AtomicBool::new(false),
                raw,
            }),
            rx,
        )
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn is_raw(&self) -> bool {
        self.raw
    }

    pub fn send(&self, data: Bytes) {
        if self.is_enabled() {
            self.tx.send_replace(data);
        }
    }
}

pub fn packetize(key: &secretbox::Key, frame_id: u64, data: &[u8]) -> Vec<Vec<u8>> {
    let fragment_count = (data.len() + FRAGMENT_SIZE - 1) / FRAGMENT_SIZE;
    if fragment_count > u16::MAX as usize {
        return Vec::new();
    }
    let fragments = fragment_count.max(1) as u16;
    (0..fragments)
        .map(|fragment| {
            let start = fragment as usize * FRAGMENT_SIZE;
            let end = (start + FRAGMENT_SIZE).min(data.len());
            let nonce = secretbox::gen_nonce();
            let encrypted = secretbox::seal(&data[start..end], &nonce, key);
            let mut packet = Vec::with_capacity(HEADER_LEN + encrypted.len());
            packet.extend_from_slice(MAGIC);
            packet.push(VERSION);
            packet.extend_from_slice(&frame_id.to_be_bytes());
            packet.extend_from_slice(&fragment.to_be_bytes());
            packet.extend_from_slice(&fragments.to_be_bytes());
            packet.extend_from_slice(&nonce.0);
            packet.extend_from_slice(&encrypted);
            packet
        })
        .collect()
}

#[derive(Default)]
pub struct Reassembler {
    frames: HashMap<u64, (u16, Vec<Option<Vec<u8>>>)>,
}

impl Reassembler {
    pub fn push(&mut self, packet: &[u8], key: &secretbox::Key) -> Option<Bytes> {
        if packet.len() < HEADER_LEN || &packet[..4] != MAGIC || packet[4] != VERSION {
            return None;
        }
        let frame_id = u64::from_be_bytes(packet[5..13].try_into().ok()?);
        let fragment = u16::from_be_bytes(packet[13..15].try_into().ok()?);
        let fragments = u16::from_be_bytes(packet[15..17].try_into().ok()?);
        if fragments == 0 || fragment >= fragments || fragments > 8192 {
            return None;
        }
        let nonce = secretbox::Nonce::from_slice(&packet[17..HEADER_LEN])?;
        let data = secretbox::open(&packet[HEADER_LEN..], &nonce, key).ok()?;

        if self.frames.len() >= 8 && !self.frames.contains_key(&frame_id) {
            if let Some(oldest) = self.frames.keys().min().copied() {
                self.frames.remove(&oldest);
            }
        }
        let entry = self
            .frames
            .entry(frame_id)
            .or_insert_with(|| (fragments, vec![None; fragments as usize]));
        if entry.0 != fragments {
            self.frames.remove(&frame_id);
            return None;
        }
        entry.1[fragment as usize] = Some(data);
        if entry.1.iter().any(Option::is_none) {
            return None;
        }

        let (_, parts) = self.frames.remove(&frame_id)?;
        let mut full = Vec::new();
        for part in parts.into_iter().flatten() {
            full.extend_from_slice(&part);
        }
        Some(Bytes::from(full))
    }
}
