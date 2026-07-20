//! OSC output: broadcasts BPM, beats, genre and palette changes so
//! external VJ tools (Resolume, TouchDesigner, ...) can follow hue2.
//!
//! Address space:
//!   /hue2/bpm        (f)    current BPM
//!   /hue2/beat       (s f)  band name, strength — fired per detected beat
//!   /hue2/intensity  (f)    loudness envelope 0..1
//!   /hue2/genre      (s)    genre id, e.g. "deep_house"
//!   /hue2/palette    (s s*) palette name followed by hex colors

use std::net::UdpSocket;

use core_types::{Band, Palette};
use rosc::{encoder, OscMessage, OscPacket, OscType};

#[derive(Debug, thiserror::Error)]
pub enum OscError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("osc encode error: {0}")]
    Encode(String),
}

pub type Result<T> = std::result::Result<T, OscError>;

pub struct OscSender {
    socket: UdpSocket,
    target: String,
}

fn band_name(band: Band) -> &'static str {
    match band {
        Band::Low => "low",
        Band::LowMid => "low_mid",
        Band::HighMid => "high_mid",
        Band::High => "high",
    }
}

impl OscSender {
    /// `target` like "127.0.0.1:9000".
    pub fn new(target: &str) -> Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        Ok(Self {
            socket,
            target: target.to_string(),
        })
    }

    fn send(&self, addr: &str, args: Vec<OscType>) -> Result<()> {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.to_string(),
            args,
        });
        let bytes = encoder::encode(&packet).map_err(|e| OscError::Encode(e.to_string()))?;
        self.socket.send_to(&bytes, &self.target)?;
        Ok(())
    }

    pub fn send_bpm(&self, bpm: f32) -> Result<()> {
        self.send("/hue2/bpm", vec![OscType::Float(bpm)])
    }

    pub fn send_beat(&self, band: Band, strength: f32) -> Result<()> {
        self.send(
            "/hue2/beat",
            vec![
                OscType::String(band_name(band).to_string()),
                OscType::Float(strength),
            ],
        )
    }

    pub fn send_intensity(&self, intensity: f32) -> Result<()> {
        self.send("/hue2/intensity", vec![OscType::Float(intensity)])
    }

    pub fn send_genre(&self, genre_id: &str) -> Result<()> {
        self.send("/hue2/genre", vec![OscType::String(genre_id.to_string())])
    }

    pub fn send_palette(&self, palette: &Palette) -> Result<()> {
        let mut args = vec![OscType::String(palette.name.clone())];
        args.extend(
            palette
                .colors
                .iter()
                .map(|c| OscType::String(c.to_hex())),
        );
        self.send("/hue2/palette", args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::Color;

    #[test]
    fn messages_arrive_and_decode() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let target = receiver.local_addr().unwrap().to_string();
        let sender = OscSender::new(&target).unwrap();

        sender.send_bpm(174.0).unwrap();
        sender
            .send_palette(&Palette {
                name: "Hardcore".into(),
                colors: vec![Color::new(255, 23, 68)],
            })
            .unwrap();

        let mut buf = [0u8; 1024];
        let (n, _) = receiver.recv_from(&mut buf).unwrap();
        let (_, packet) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        match packet {
            OscPacket::Message(m) => {
                assert_eq!(m.addr, "/hue2/bpm");
                assert_eq!(m.args, vec![OscType::Float(174.0)]);
            }
            _ => panic!("expected message"),
        }

        let (n, _) = receiver.recv_from(&mut buf).unwrap();
        let (_, packet) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        match packet {
            OscPacket::Message(m) => {
                assert_eq!(m.addr, "/hue2/palette");
                assert_eq!(m.args[0], OscType::String("Hardcore".into()));
                assert_eq!(m.args[1], OscType::String("#ff1744".into()));
            }
            _ => panic!("expected message"),
        }
    }
}
