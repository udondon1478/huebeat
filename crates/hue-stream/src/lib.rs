//! Hue Entertainment API streaming: DTLS 1.2 with PSK
//! (TLS_PSK_WITH_AES_128_GCM_SHA256) over UDP port 2100,
//! sending HueStream v2 binary frames at up to 60 Hz.

use std::sync::Arc;

use core_types::LightFrame;
use webrtc_dtls::cipher_suite::CipherSuiteId;
use webrtc_dtls::config::Config;
use webrtc_dtls::conn::DTLSConn;
use webrtc_util::Conn;

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("dtls error: {0}")]
    Dtls(String),
    #[error("invalid client key (expected 32 hex chars)")]
    BadClientKey,
    #[error("entertainment configuration id must be 36 chars, got {0}")]
    BadConfigId(usize),
}

pub type Result<T> = std::result::Result<T, StreamError>;

const HUE_STREAM_PORT: u16 = 2100;
const PROTOCOL_NAME: &[u8; 9] = b"HueStream";
/// Max channels per HueStream v2 frame.
pub const MAX_CHANNELS: usize = 20;

fn decode_hex(s: &str) -> Result<Vec<u8>> {
    if s.len() != 32 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(StreamError::BadClientKey);
    }
    Ok((0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect())
}

/// Build one HueStream v2 packet (RGB color space, 16-bit per component).
pub fn encode_frame(config_id: &str, sequence: u8, frame: &LightFrame) -> Result<Vec<u8>> {
    if config_id.len() != 36 {
        return Err(StreamError::BadConfigId(config_id.len()));
    }
    let mut buf = Vec::with_capacity(52 + 7 * MAX_CHANNELS);
    buf.extend_from_slice(PROTOCOL_NAME);
    buf.extend_from_slice(&[0x02, 0x00]); // version 2.0
    buf.push(sequence);
    buf.extend_from_slice(&[0x00, 0x00]); // reserved
    buf.push(0x00); // color space: RGB
    buf.push(0x00); // reserved
    buf.extend_from_slice(config_id.as_bytes());
    for (channel_id, color) in frame.channels.iter().take(MAX_CHANNELS) {
        buf.push(*channel_id);
        // 8-bit sRGB expanded to 16-bit big-endian.
        for c in [color.r, color.g, color.b] {
            let v = u16::from(c) * 257;
            buf.extend_from_slice(&v.to_be_bytes());
        }
    }
    Ok(buf)
}

/// An open DTLS entertainment stream to a bridge.
///
/// Lifecycle: caller must first PUT `action: start` on the entertainment
/// configuration (see `hue-client`), then `HueStreamer::connect` within a
/// few seconds, then send frames continuously (the bridge drops the
/// session after ~10 s of silence), then `close` and PUT `action: stop`.
pub struct HueStreamer {
    conn: DTLSConn,
    config_id: String,
    sequence: u8,
}

impl HueStreamer {
    /// `application_id` is the value of the `hue-application-id` header
    /// (DTLS PSK identity); `client_key` is the 32-hex-char key from pairing.
    pub async fn connect(
        bridge_ip: &str,
        application_id: &str,
        client_key: &str,
        config_id: &str,
    ) -> Result<Self> {
        if config_id.len() != 36 {
            return Err(StreamError::BadConfigId(config_id.len()));
        }
        // webrtc-dtls uses rustls internally; a process-level crypto
        // provider must be installed once or the handshake panics.
        static CRYPTO: std::sync::Once = std::sync::Once::new();
        CRYPTO.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });

        let psk = decode_hex(client_key)?;
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect((bridge_ip, HUE_STREAM_PORT)).await?;

        let identity = application_id.as_bytes().to_vec();
        let config = Config {
            psk: Some(Arc::new(move |_hint: &[u8]| Ok(psk.clone()))),
            psk_identity_hint: Some(identity),
            cipher_suites: vec![CipherSuiteId::Tls_Psk_With_Aes_128_Gcm_Sha256],
            server_name: bridge_ip.to_string(),
            ..Default::default()
        };
        let conn = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            DTLSConn::new(Arc::new(socket), config, true, None),
        )
        .await
        .map_err(|_| StreamError::Dtls("handshake timed out (no DTLS response)".into()))?
        .map_err(|e| StreamError::Dtls(e.to_string()))?;
        tracing::info!(bridge_ip, config_id, "hue entertainment DTLS session established");
        Ok(Self {
            conn,
            config_id: config_id.to_string(),
            sequence: 0,
        })
    }

    pub async fn send(&mut self, frame: &LightFrame) -> Result<()> {
        let packet = encode_frame(&self.config_id, self.sequence, frame)?;
        self.sequence = self.sequence.wrapping_add(1);
        self.conn
            .send(&packet)
            .await
            .map_err(|e| StreamError::Dtls(e.to_string()))?;
        Ok(())
    }

    pub async fn close(self) {
        let _ = self.conn.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::Color;

    const CFG_ID: &str = "01234567-89ab-cdef-0123-456789abcdef";

    #[test]
    fn frame_layout() {
        let frame = LightFrame {
            channels: vec![(0, Color::new(255, 0, 128))],
        };
        let buf = encode_frame(CFG_ID, 7, &frame).unwrap();
        assert_eq!(&buf[0..9], b"HueStream");
        assert_eq!(&buf[9..11], &[0x02, 0x00]);
        assert_eq!(buf[11], 7); // sequence
        assert_eq!(buf[14], 0x00); // RGB color space
        assert_eq!(&buf[16..52], CFG_ID.as_bytes());
        assert_eq!(buf[52], 0); // channel id
        assert_eq!(&buf[53..55], &0xffffu16.to_be_bytes());
        assert_eq!(&buf[55..57], &0x0000u16.to_be_bytes());
        assert_eq!(&buf[57..59], &(128u16 * 257).to_be_bytes());
        assert_eq!(buf.len(), 52 + 7);
    }

    #[test]
    fn hex_decoding() {
        assert!(decode_hex("00ff").is_err());
        assert_eq!(
            decode_hex("000102030405060708090a0b0c0d0e0f").unwrap(),
            (0..16).collect::<Vec<u8>>()
        );
    }
}
