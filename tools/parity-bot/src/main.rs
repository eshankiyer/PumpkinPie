//! Joins a Pumpkin server as an offline player and records what it sees.
//!
//! Entities do not tick in Pumpkin with no player in simulation range, so parity checks that
//! involve mob behaviour, effects or block ticking need a client in the world. This is that
//! client: it joins, stays put, and writes one JSON line per observed event to stdout, which a
//! scenario script can diff against the same scenario run against vanilla.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use azalea::prelude::*;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "parity-bot", about = "Headless observer for Pumpkin parity checks")]
struct Args {
    /// Server address, host:port.
    #[arg(long, default_value = "127.0.0.1:25565")]
    server: String,

    /// Offline username to join with.
    #[arg(long, default_value = "ParityBot")]
    username: String,

    /// Record clientbound packets whose variant name contains this string. Repeatable. Without
    /// it only lifecycle events are recorded, since a joined client sees thousands of packets.
    #[arg(long = "packet")]
    packets: Vec<String>,

    /// Include the full packet body rather than just its variant name.
    #[arg(long)]
    verbose_packets: bool,

    /// Leave the world after this many seconds. Zero stays until the server disconnects us.
    #[arg(long, default_value_t = 0)]
    seconds: u64,
}

#[derive(Clone, Default, Component)]
struct State {
    packets: Arc<Vec<String>>,
    verbose_packets: bool,
}

/// `ClientboundGamePacket` renders as `Variant(..)`, so the name is everything before the paren.
fn packet_name(rendered: &str) -> &str {
    rendered
        .split_once(['(', ' ', '{'])
        .map_or(rendered, |(name, _)| name)
}

fn emit(value: serde_json::Value) {
    println!("{value}");
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.seconds > 0 {
        let seconds = args.seconds;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            emit(serde_json::json!({ "event": "timeout", "seconds": seconds }));
            std::process::exit(0);
        });
    }

    let state = State {
        packets: Arc::new(args.packets),
        verbose_packets: args.verbose_packets,
    };

    let exit = ClientBuilder::new()
        .set_handler(handle)
        .set_state(state)
        .start(Account::offline(&args.username), args.server.as_str())
        .await;

    emit(serde_json::json!({ "event": "exit", "status": format!("{exit:?}") }));
    Ok(())
}

async fn handle(_client: Client, event: Event, state: State) -> Result<()> {
    match event {
        Event::Login => emit(serde_json::json!({ "event": "login" })),
        Event::Spawn => emit(serde_json::json!({ "event": "spawn" })),
        Event::Death(_) => emit(serde_json::json!({ "event": "death" })),
        Event::Chat(message) => emit(serde_json::json!({
            "event": "chat",
            "message": message.message().to_string(),
        })),
        Event::Disconnect(reason) => emit(serde_json::json!({
            "event": "disconnect",
            "reason": reason.map(|r| r.to_string()),
        })),
        Event::Packet(packet) => {
            if state.packets.is_empty() {
                return Ok(());
            }
            let rendered = format!("{packet:?}");
            let name = packet_name(&rendered);
            if !state.packets.iter().any(|wanted| name.contains(wanted)) {
                return Ok(());
            }
            if state.verbose_packets {
                emit(serde_json::json!({ "event": "packet", "name": name, "body": rendered }));
            } else {
                emit(serde_json::json!({ "event": "packet", "name": name }));
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::packet_name;

    #[test]
    fn a_packet_name_stops_at_its_payload() {
        assert_eq!(packet_name("AddEntity(ClientboundAddEntity { .. })"), "AddEntity");
        assert_eq!(packet_name("UpdateMobEffect { id: 1 }"), "UpdateMobEffect");
        assert_eq!(packet_name("KeepAlive"), "KeepAlive");
    }
}
