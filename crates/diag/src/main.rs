//! Bridge connectivity diagnostic:
//! cargo run -p diag -- <ip> <app_key> <client_key> <entertainment_config_id>

use core_types::{Color, LightFrame};
use hue_client::{HueClient, PairedBridge};
use hue_stream::HueStreamer;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing_subscriber::filter::LevelFilter::DEBUG)
        .init();
    let args: Vec<String> = std::env::args().collect();
    let [_, ip, app_key, client_key, config_id] = &args[..] else {
        eprintln!("usage: diag <ip> <app_key> <client_key> <entertainment_config_id>");
        std::process::exit(1);
    };

    let bridge = PairedBridge {
        ip: ip.clone(),
        app_key: app_key.clone(),
        client_key: client_key.clone(),
    };

    // --- 1. /auth/v2 headers
    println!("== 1. GET /auth/v2");
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .unwrap();
    match http
        .get(format!("https://{ip}/auth/v2"))
        .header("hue-application-key", app_key)
        .send()
        .await
    {
        Ok(resp) => {
            println!("   status: {}", resp.status());
            for (k, v) in resp.headers() {
                println!("   header: {k}: {v:?}");
            }
        }
        Err(e) => println!("   request failed: {e}"),
    }

    // --- 2. entertainment configurations
    println!("== 2. entertainment configurations");
    let client = HueClient::new(&bridge).unwrap();
    let app_id = match client.application_id().await {
        Ok(id) => {
            println!("   application_id: {id}");
            Some(id)
        }
        Err(e) => {
            println!("   application_id error: {e}");
            None
        }
    };
    match client.entertainment_configs().await {
        Ok(configs) => {
            for c in &configs {
                println!(
                    "   area: {} ({}) channels={:?}",
                    c.name,
                    c.id,
                    c.channels.iter().map(|ch| ch.channel_id).collect::<Vec<_>>()
                );
            }
            if !configs.iter().any(|c| &c.id == config_id) {
                println!("   !! selected config id {config_id} not found");
            }
        }
        Err(e) => println!("   list error: {e}"),
    }

    // --- 3. start streaming + DTLS handshake
    for identity in [app_id.as_deref(), Some(app_key.as_str())].into_iter().flatten() {
        println!("== 3. PUT action=start + DTLS handshake (identity: {identity})");
        if let Err(e) = client.start_streaming(config_id).await {
            println!("   start_streaming error: {e}");
            continue;
        }
        println!("   REST start ok, handshaking (10 s timeout)...");
        let connect = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            HueStreamer::connect(ip, identity, client_key, config_id),
        )
        .await;
        match connect {
            Err(_) => {
                println!("   !! handshake TIMED OUT (no DTLS response)");
                let _ = client.stop_streaming(config_id).await;
            }
            Ok(Err(e)) => {
                println!("   !! handshake error: {e}");
                let _ = client.stop_streaming(config_id).await;
            }
            Ok(Ok(mut streamer)) => {
                println!("   handshake OK — sending rainbow for 5 s");
                let channels: Vec<u8> = client
                    .entertainment_configs()
                    .await
                    .ok()
                    .and_then(|cs| {
                        cs.iter()
                            .find(|c| &c.id == config_id)
                            .map(|c| c.channels.iter().map(|ch| ch.channel_id).collect())
                    })
                    .unwrap_or_else(|| vec![0]);
                let mut ticker =
                    tokio::time::interval(std::time::Duration::from_millis(20));
                for i in 0..250u32 {
                    ticker.tick().await;
                    let hue_deg = (i as f32 * 2.0) % 360.0;
                    let frame = LightFrame {
                        channels: channels.iter().map(|&id| (id, hsv(hue_deg))).collect(),
                    };
                    if let Err(e) = streamer.send(&frame).await {
                        println!("   !! send error at frame {i}: {e}");
                        break;
                    }
                }
                println!("   done sending");
                streamer.close().await;
                let _ = client.stop_streaming(config_id).await;
                return;
            }
        }
    }
}

fn hsv(h: f32) -> Color {
    let x = (1.0 - ((h / 60.0) % 2.0 - 1.0).abs()) * 255.0;
    let x = x as u8;
    match (h as u32) / 60 % 6 {
        0 => Color::new(255, x, 0),
        1 => Color::new(x, 255, 0),
        2 => Color::new(0, 255, x),
        3 => Color::new(0, x, 255),
        4 => Color::new(x, 0, 255),
        _ => Color::new(255, 0, x),
    }
}
