//! Joins a Pumpkin server as an offline player and records what it sees.
//!
//! Entities do not tick in Pumpkin with no player in simulation range, so parity checks that
//! involve mob behaviour, effects or block ticking need a client in the world. This is that
//! client: it joins, stands still, and writes one JSON line per observed event to stdout, which
//! a scenario script can diff against the same scenario run on vanilla.

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
}

#[derive(Default, Clone, Component)]
struct State;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let exit = ClientBuilder::new()
        .set_handler(handle)
        .start(Account::offline(&args.username), args.server.as_str())
        .await;

    println!("{}", serde_json::json!({ "event": "exit", "status": format!("{exit:?}") }));
    Ok(())
}

async fn handle(_client: Client, event: Event, _state: State) -> Result<()> {
    let line = match event {
        Event::Login => serde_json::json!({ "event": "login" }),
        Event::Chat(message) => serde_json::json!({
            "event": "chat",
            "message": message.message().to_string(),
        }),
        Event::Death(_) => serde_json::json!({ "event": "death" }),
        Event::Disconnect(reason) => serde_json::json!({
            "event": "disconnect",
            "reason": reason.map(|r| r.to_string()),
        }),
        _ => return Ok(()),
    };

    println!("{line}");
    Ok(())
}
