use hbb_common::{
    bytes::Bytes,
    protobuf::Message as _,
    sodiumoxide::crypto::secretbox,
    Stream,
    tokio::sync::watch,
};
use base::message_proto::{message, video_frame, Message};
use base::message_proto::{key_event, KeyEvent, MouseEvent};

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

// GameStream video packets use a standard RTP header followed by the legacy
// NV packet metadata used by Sunshine and Moonlight. RustDesk keeps its
// authenticated session and uses secretbox only for the media payload.
pub const RTP_HEADER_LEN: usize = 12;
pub const NV_HEADER_LEN: usize = 16;
pub const FRAME_HEADER_LEN: usize = 8;
pub const PACKET_HEADER_LEN: usize = RTP_HEADER_LEN + 4 + NV_HEADER_LEN + FRAME_HEADER_LEN;
pub const MEDIA_MTU: usize = 1200;
pub const MEDIA_PAYLOAD_SIZE: usize =
    MEDIA_MTU - PACKET_HEADER_LEN - secretbox::NONCEBYTES - secretbox::MACBYTES;
// Sunshine leaves the video RTP payload type at zero in its raw GameStream
// packet. The extension bit and NV header identify the packet on this socket.
pub const VIDEO_PAYLOAD_TYPE: u8 = 0;
pub const RTP_EXTENSION: u8 = 0x10;
pub const FRAME_FLAG_PICTURE: u8 = 0x01;
pub const FRAME_FLAG_EOF: u8 = 0x02;
pub const FRAME_FLAG_SOF: u8 = 0x04;
const LEGACY_MAGIC: &[u8; 4] = b"RDMV";
const LEGACY_VERSION: u8 = 1;
const LEGACY_FRAGMENT_SIZE: usize = 1050;
const LEGACY_HEADER_LEN: usize = 4 + 1 + 8 + 2 + 2 + secretbox::NONCEBYTES;
const NV_EXTRA_CODEC_H264: u8 = 0x80;
const NV_EXTRA_CODEC_H265: u8 = 0x81;
const GAMESTREAM_PLAINTEXT: u8 = 0x01;
const INPUT_MAGIC: &[u8; 4] = b"RDIN";
const INPUT_VERSION: u8 = 1;
const INPUT_HEADER_LEN: usize = 4 + 1 + secretbox::NONCEBYTES;

pub fn packetize_input(key: &secretbox::Key, data: &[u8]) -> Bytes {
    let nonce = secretbox::gen_nonce();
    let encrypted = secretbox::seal(data, &nonce, key);
    let mut packet = Vec::with_capacity(INPUT_HEADER_LEN + encrypted.len());
    packet.extend_from_slice(INPUT_MAGIC);
    packet.push(INPUT_VERSION);
    packet.extend_from_slice(&nonce.0);
    packet.extend_from_slice(&encrypted);
    Bytes::from(packet)
}

pub fn is_input_packet(packet: &[u8]) -> bool {
    packet.len() >= INPUT_HEADER_LEN + secretbox::MACBYTES
        && &packet[..4] == INPUT_MAGIC
        && packet[4] == INPUT_VERSION
}

pub fn open_input(key: &secretbox::Key, packet: &[u8]) -> Option<Bytes> {
    if !is_input_packet(packet) {
        return None;
    }
    let nonce = secretbox::Nonce::from_slice(&packet[5..INPUT_HEADER_LEN])?;
    secretbox::open(&packet[INPUT_HEADER_LEN..], &nonce, key)
        .ok()
        .map(Bytes::from)
}

const INPUT_KEY_DOWN: u32 = 0x00000003;
const INPUT_KEY_UP: u32 = 0x00000004;
const INPUT_MOUSE_ABS: u32 = 0x00000005;
const INPUT_MOUSE_REL: u32 = 0x00000007;
const INPUT_MOUSE_BUTTON_DOWN: u32 = 0x00000008;
const INPUT_MOUSE_BUTTON_UP: u32 = 0x00000009;
const INPUT_SCROLL: u32 = 0x0000000A;

fn put_be_i16(dst: &mut Vec<u8>, value: i32) {
    dst.extend_from_slice(&(value.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_be_bytes());
}

fn input_header(magic: u32, payload_len: usize, out: &mut Vec<u8>) {
    out.extend_from_slice(&((payload_len + 4) as u32).to_be_bytes());
    out.extend_from_slice(&magic.to_le_bytes());
}

pub fn encode_sunshine_input(message: &Message) -> Option<Bytes> {
    let mut packet = Vec::new();
    match message.union.as_ref()? {
        message::Union::MouseEvent(mouse) => {
            let kind = mouse.mask & 7;
            if kind == 3 {
                input_header(INPUT_SCROLL, 6, &mut packet);
                put_be_i16(&mut packet, mouse.y);
                put_be_i16(&mut packet, 0);
                packet.extend_from_slice(&0i16.to_be_bytes());
            } else if kind == 1 || kind == 2 {
                let button = ((mouse.mask >> 3) & 0xFF) as u8;
                input_header(
                    if kind == 1 { INPUT_MOUSE_BUTTON_DOWN } else { INPUT_MOUSE_BUTTON_UP },
                    1,
                    &mut packet,
                );
                packet.push(button);
            } else {
                input_header(INPUT_MOUSE_REL, 4, &mut packet);
                put_be_i16(&mut packet, mouse.x);
                put_be_i16(&mut packet, mouse.y);
            }
        }
        message::Union::KeyEvent(key) => {
            let (code, unicode) = match key.union.as_ref()? {
                key_event::Union::Chr(code) => (*code as u16, false),
                key_event::Union::ControlKey(control) => (*control as u16, false),
                key_event::Union::Unicode(code) => (*code as u16, true),
                _ => return None,
            };
            if unicode {
                return None;
            }
            input_header(if key.down || key.press { INPUT_KEY_DOWN } else { INPUT_KEY_UP }, 6, &mut packet);
            packet.push(0);
            packet.extend_from_slice(&code.to_be_bytes());
            packet.push(0);
            packet.extend_from_slice(&0i16.to_be_bytes());
        }
        _ => return None,
    }
    Some(Bytes::from(packet))
}

pub fn decode_sunshine_input(packet: &[u8]) -> Option<Message> {
    if packet.len() < 8 {
        return None;
    }
    let size = u32::from_be_bytes(packet[..4].try_into().ok()?) as usize;
    let magic = u32::from_le_bytes(packet[4..8].try_into().ok()?);
    if size + 4 != packet.len() {
        return None;
    }
    let payload = &packet[8..];
    let mut message = Message::new();
    match magic {
        INPUT_MOUSE_REL if payload.len() >= 4 => {
            let x = i16::from_be_bytes(payload[..2].try_into().ok()?) as i32;
            let y = i16::from_be_bytes(payload[2..4].try_into().ok()?) as i32;
            message.set_mouse_event(MouseEvent { x, y, ..Default::default() });
        }
        INPUT_MOUSE_BUTTON_DOWN | INPUT_MOUSE_BUTTON_UP if !payload.is_empty() => {
            let button = payload[0] as i32;
            let kind = if magic == INPUT_MOUSE_BUTTON_DOWN { 1 } else { 2 };
            message.set_mouse_event(MouseEvent {
                mask: (button << 3) | kind,
                ..Default::default()
            });
        }
        INPUT_SCROLL if payload.len() >= 2 => {
            let amount = i16::from_be_bytes(payload[..2].try_into().ok()?) as i32;
            message.set_mouse_event(MouseEvent {
                mask: 3,
                y: amount,
                ..Default::default()
            });
        }
        INPUT_KEY_DOWN | INPUT_KEY_UP if payload.len() >= 5 => {
            let code = u16::from_be_bytes(payload[1..3].try_into().ok()?) as u32;
            let mut key = KeyEvent::new();
            key.down = magic == INPUT_KEY_DOWN;
            key.set_chr(code);
            message.set_key_event(key);
        }
        _ => return None,
    }
    Some(message)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Codec {
    H264 = 1,
    H265 = 2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameStreamFrame {
    pub frame_index: u32,
    pub timestamp: u32,
    pub display: i32,
    pub key: bool,
    pub codec: Codec,
    pub data: Bytes,
}

pub fn is_gamestream_packet(packet: &[u8]) -> bool {
    packet.len() >= 64
        && packet[0] & 0xc0 == 0x80
        && packet[0] & RTP_EXTENSION != 0
        && packet[1] == VIDEO_PAYLOAD_TYPE
        && packet[32] == 0x01
}

fn put_be16(dst: &mut [u8], value: u16) {
    dst.copy_from_slice(&value.to_be_bytes());
}

fn put_be32(dst: &mut [u8], value: u32) {
    dst.copy_from_slice(&value.to_be_bytes());
}

fn put_le16(dst: &mut [u8], value: u16) {
    dst.copy_from_slice(&value.to_le_bytes());
}

fn put_le32(dst: &mut [u8], value: u32) {
    dst.copy_from_slice(&value.to_le_bytes());
}

pub fn packetize_gamestream(
    key: &secretbox::Key,
    sequence: &mut u16,
    frame: &GameStreamFrame,
) -> Vec<Vec<u8>> {
    packetize_gamestream_inner(Some(key), sequence, frame)
}

fn packetize_gamestream_inner(
    key: Option<&secretbox::Key>,
    sequence: &mut u16,
    frame: &GameStreamFrame,
) -> Vec<Vec<u8>> {
    let packet_count = frame.data.len().max(1).div_ceil(MEDIA_PAYLOAD_SIZE);
    if packet_count > 1023 {
        return Vec::new();
    }
    let mut packets = Vec::with_capacity(packet_count);
    for index in 0..packet_count {
        let start = index * MEDIA_PAYLOAD_SIZE;
        let end = (start + MEDIA_PAYLOAD_SIZE).min(frame.data.len());
        let plaintext = &frame.data[start..end];
        let nonce = secretbox::gen_nonce();
        let payload = key
            .map(|key| secretbox::seal(plaintext, &nonce, key))
            .unwrap_or_else(|| plaintext.to_vec());
        let mut packet = vec![0; 64 + payload.len()];

        packet[0] = 0x80 | RTP_EXTENSION;
        packet[1] = VIDEO_PAYLOAD_TYPE;
        put_be16(&mut packet[2..4], *sequence);
        put_be32(&mut packet[4..8], frame.timestamp);
        put_be32(&mut packet[8..12], frame.display.max(0) as u32 + 1);

        // Two extension words, matching the 4-byte reserved area followed by
        // the GameStream NV_VIDEO_PACKET fields.
        put_le32(&mut packet[12..16], 0);
        // streamPacketIndex is frame-local in the GameStream depacketizer;
        // RTP sequence remains the global packet ordering field.
        put_le32(&mut packet[16..20], (index as u32) << 8);
        put_le32(&mut packet[20..24], frame.frame_index);
        packet[24] = FRAME_FLAG_PICTURE
            | if index == 0 { FRAME_FLAG_SOF } else { 0 }
            | if index + 1 == packet_count { FRAME_FLAG_EOF } else { 0 };
        packet[25] = 0;
        packet[26] = 0x10;
        packet[27] = 0;
        put_le32(
            &mut packet[28..32],
            (index as u32) << 12 | (packet_count as u32) << 22,
        );

        packet[32] = 0x01;
        put_le16(&mut packet[33..35], 0);
        packet[35] = if frame.key { 2 } else { 1 };
        put_le16(&mut packet[36..38], plaintext.len() as u16);
        packet[38] = match frame.codec {
            Codec::H264 => NV_EXTRA_CODEC_H264,
            Codec::H265 => NV_EXTRA_CODEC_H265,
        };
        packet[39] = if key.is_some() { 0 } else { GAMESTREAM_PLAINTEXT };
        if key.is_some() {
            packet[40..64].copy_from_slice(&nonce.0);
        }
        packet[64..].copy_from_slice(&payload);
        packets.push(packet);
        *sequence = sequence.wrapping_add(1);
    }
    packets
}

pub fn packetize_message(
    key: &secretbox::Key,
    sequence: &mut u16,
    frame_index: &mut u32,
    data: &[u8],
) -> Option<Vec<Vec<u8>>> {
    packetize_message_inner(Some(key), sequence, frame_index, data)
}

pub fn packetize_message_plain(
    sequence: &mut u16,
    frame_index: &mut u32,
    data: &[u8],
) -> Option<Vec<Vec<u8>>> {
    packetize_message_inner(None, sequence, frame_index, data)
}

fn packetize_message_inner(
    key: Option<&secretbox::Key>,
    sequence: &mut u16,
    frame_index: &mut u32,
    data: &[u8],
) -> Option<Vec<Vec<u8>>> {
    let message = Message::parse_from_bytes(data).ok()?;
    let video = match message.union? {
        message::Union::VideoFrame(frame) => frame,
        _ => return None,
    };
    let (codec, frames) = match video.union? {
        video_frame::Union::H264s(frames) => (Codec::H264, frames),
        video_frame::Union::H265s(frames) => (Codec::H265, frames),
        _ => return None,
    };
    let mut packets = Vec::new();
    for frame in frames.frames.iter() {
        let timestamp = (frame.pts.max(0) as u64 * 90).min(u32::MAX as u64) as u32;
        packets.extend(packetize_gamestream_inner(
            key,
            sequence,
            &GameStreamFrame {
                frame_index: *frame_index,
                timestamp,
                display: video.display,
                key: frame.key,
                codec,
                data: frame.data.clone(),
            },
        ));
        *frame_index = (*frame_index).wrapping_add(1);
    }
    Some(packets)
}

pub struct MediaChannel {
    tx: watch::Sender<Bytes>,
    enabled: AtomicBool,
    gamestream: AtomicBool,
    raw: bool,
}

impl MediaChannel {
    pub fn new(raw: bool) -> (Arc<Self>, watch::Receiver<Bytes>) {
        let (tx, rx) = watch::channel(Bytes::new());
        (
            Arc::new(Self {
                tx,
                enabled: AtomicBool::new(false),
                gamestream: AtomicBool::new(false),
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

    pub fn set_gamestream(&self, enabled: bool) {
        self.gamestream.store(enabled, Ordering::Release);
    }

    pub fn is_gamestream(&self) -> bool {
        self.gamestream.load(Ordering::Acquire)
    }

    pub fn send(&self, data: Bytes) {
        if self.is_enabled() {
            self.tx.send_replace(data);
        }
    }
}

#[derive(Default)]
pub struct Reassembler {
    frames: HashMap<u64, (u16, Vec<Option<Vec<u8>>>)>,
}

pub fn packetize_legacy(key: &secretbox::Key, frame_id: u64, data: &[u8]) -> Vec<Vec<u8>> {
    let fragment_count = data.len().max(1).div_ceil(LEGACY_FRAGMENT_SIZE);
    if fragment_count > u16::MAX as usize {
        return Vec::new();
    }
    let fragments = fragment_count as u16;
    (0..fragments)
        .map(|fragment| {
            let start = fragment as usize * LEGACY_FRAGMENT_SIZE;
            let end = (start + LEGACY_FRAGMENT_SIZE).min(data.len());
            let nonce = secretbox::gen_nonce();
            let encrypted = secretbox::seal(&data[start..end], &nonce, key);
            let mut packet = Vec::with_capacity(LEGACY_HEADER_LEN + encrypted.len());
            packet.extend_from_slice(LEGACY_MAGIC);
            packet.push(LEGACY_VERSION);
            packet.extend_from_slice(&frame_id.to_be_bytes());
            packet.extend_from_slice(&fragment.to_be_bytes());
            packet.extend_from_slice(&fragments.to_be_bytes());
            packet.extend_from_slice(&nonce.0);
            packet.extend_from_slice(&encrypted);
            packet
        })
        .collect()
}

pub fn is_legacy_packet(packet: &[u8]) -> bool {
    packet.len() >= LEGACY_HEADER_LEN
        && &packet[..4] == LEGACY_MAGIC
        && packet[4] == LEGACY_VERSION
}

impl Reassembler {
    pub fn push_legacy(&mut self, packet: &[u8], key: &secretbox::Key) -> Option<Bytes> {
        if !is_legacy_packet(packet) {
            return None;
        }
        let frame_id = u64::from_be_bytes(packet[5..13].try_into().ok()?);
        let fragment = u16::from_be_bytes(packet[13..15].try_into().ok()?);
        let fragments = u16::from_be_bytes(packet[15..17].try_into().ok()?);
        if fragments == 0 || fragment >= fragments || fragments > 8192 {
            return None;
        }
        let nonce = secretbox::Nonce::from_slice(&packet[17..LEGACY_HEADER_LEN])?;
        let data = secretbox::open(&packet[LEGACY_HEADER_LEN..], &nonce, key).ok()?;
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

    pub fn push_gamestream(
        &mut self,
        packet: &[u8],
        key: &secretbox::Key,
    ) -> Option<GameStreamFrame> {
        self.push_gamestream_inner(packet, Some(key))
    }

    pub fn push_gamestream_plain(&mut self, packet: &[u8]) -> Option<GameStreamFrame> {
        self.push_gamestream_inner(packet, None)
    }

    fn push_gamestream_inner(
        &mut self,
        packet: &[u8],
        key: Option<&secretbox::Key>,
    ) -> Option<GameStreamFrame> {
        if !is_gamestream_packet(packet)
            || packet.len() < 64
        {
            return None;
        }
        let frame_id = u32::from_le_bytes(packet[20..24].try_into().ok()?);
        let key_frame = packet[35] == 2;
        let codec = match packet[38] {
            NV_EXTRA_CODEC_H264 => Codec::H264,
            NV_EXTRA_CODEC_H265 => Codec::H265,
            _ => return None,
        };
        let packet_count = u32::from_le_bytes(packet[28..32].try_into().ok()?) >> 22;
        if packet_count == 0 || packet_count > 1023 {
            return None;
        }
        let timestamp = u32::from_be_bytes(packet[4..8].try_into().ok()?);
        let display = u32::from_be_bytes(packet[8..12].try_into().ok()?)
            .saturating_sub(1) as i32;
        let fragment = (u32::from_le_bytes(packet[16..20].try_into().ok()?) >> 8) as usize;
        let data = if packet[39] & GAMESTREAM_PLAINTEXT != 0 {
            packet[64..].to_vec()
        } else {
            let key = key?;
            let nonce = secretbox::Nonce::from_slice(&packet[40..64])?;
            secretbox::open(&packet[64..], &nonce, key).ok()?
        };

        if self.frames.len() >= 8 && !self.frames.contains_key(&(frame_id as u64)) {
            if let Some(oldest) = self.frames.keys().min().copied() {
                self.frames.remove(&oldest);
            }
        }
        let entry = self
            .frames
            .entry(frame_id as u64)
            .or_insert_with(|| (packet_count as u16, vec![None; packet_count as usize]));
        if entry.0 != packet_count as u16 || fragment >= packet_count as usize {
            self.frames.remove(&(frame_id as u64));
            return None;
        }
        entry.1[fragment] = Some(data);
        if entry.1.iter().any(Option::is_none) {
            return None;
        }

        let (_, parts) = self.frames.remove(&(frame_id as u64))?;
        let mut full = Vec::new();
        for part in parts.into_iter().flatten() {
            full.extend_from_slice(&part);
        }
        Some(GameStreamFrame {
            frame_index: frame_id,
            timestamp,
            display,
            key: key_frame,
            codec,
            data: Bytes::from(full),
        })
    }
}

pub fn encode_gamestream_frame(frame: &GameStreamFrame) -> Option<Bytes> {
    let mut message = base::message_proto::VideoFrame::new();
    let encoded = base::message_proto::EncodedVideoFrame {
        data: frame.data.clone(),
        key: frame.key,
        pts: (frame.timestamp / 90) as i64,
        ..Default::default()
    };
    let frames = base::message_proto::EncodedVideoFrames {
        frames: vec![encoded].into(),
        ..Default::default()
    };
    match frame.codec {
        Codec::H264 => message.set_h264s(frames),
        Codec::H265 => message.set_h265s(frames),
    }
    let mut outer = Message::new();
    let mut video = message;
    video.display = frame.display;
    outer.set_video_frame(video);
    outer.write_to_bytes().ok().map(Bytes::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamestream_packet_roundtrip_preserves_frame() {
        let _ = hbb_common::sodiumoxide::init();
        let key = secretbox::gen_key();
        let source = GameStreamFrame {
            frame_index: 42,
            timestamp: 90_000,
            display: 2,
            key: true,
            codec: Codec::H264,
            data: Bytes::from(vec![7u8; MEDIA_PAYLOAD_SIZE + 17]),
        };
        let mut sequence = 10;
        let packets = packetize_gamestream(&key, &mut sequence, &source);
        assert!(packets.iter().all(|packet| is_gamestream_packet(packet)));

        let mut reassembler = Reassembler::default();
        let mut result = None;
        for packet in packets.iter().rev() {
            result = reassembler.push_gamestream(packet, &key).or(result);
        }
        assert_eq!(result, Some(source));
    }

    #[test]
    fn legacy_packet_roundtrip_remains_available() {
        let _ = hbb_common::sodiumoxide::init();
        let key = secretbox::gen_key();
        let source = Bytes::from_static(b"legacy media");
        let packets = packetize_legacy(&key, 9, &source);
        let mut reassembler = Reassembler::default();
        let mut result = None;
        for packet in packets {
            result = reassembler.push_legacy(&packet, &key).or(result);
        }
        assert_eq!(result, Some(source));
    }

    #[test]
    fn plaintext_gamestream_packet_roundtrip_preserves_frame() {
        let source = GameStreamFrame {
            frame_index: 7,
            timestamp: 180_000,
            display: 1,
            key: false,
            codec: Codec::H265,
            data: Bytes::from_static(b"relay media"),
        };
        let mut sequence = 0;
        let packets = packetize_gamestream_inner(None, &mut sequence, &source);
        let mut reassembler = Reassembler::default();
        let result = reassembler.push_gamestream_plain(&packets[0]);
        assert_eq!(result, Some(source));
    }

    #[test]
    fn sunshine_input_roundtrip_preserves_relative_mouse() {
        let mut message = Message::new();
        message.set_mouse_event(MouseEvent {
            x: 12,
            y: -7,
            ..Default::default()
        });
        let packet = encode_sunshine_input(&message).unwrap();
        let decoded = decode_sunshine_input(&packet).unwrap();
        let Some(message::Union::MouseEvent(mouse)) = decoded.union else {
            panic!("expected mouse event");
        };
        assert_eq!((mouse.x, mouse.y), (12, -7));
    }
}
