//! Isolated process fixture for the mobile core's commands and applied snapshots.
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use tokio::io::AsyncBufReadExt;
use zork_client_core::{
    subscriptions::{Key, WireSubscription},
    Client, Command,
};

fn emit(value: Value) -> Result<()> {
    println!("{value}");
    std::io::stdout().flush()?;
    Ok(())
}
fn deliver(source: &str, observer: &mut WireSubscription) -> Result<()> {
    if let Some(frame) = observer.prepare()? {
        emit(json!({"source":source,"event":frame}))?;
        observer.finish(frame["batch"].as_u64().context("batch")?, true);
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let root = std::env::args_os().nth(1).context("client root")?;
    let mut client = Client::open(std::path::Path::new(&root))?;
    let mut invitation = client.local().observe(Key::Invitation)?;
    let mut directory = client.local().observe(Key::Directory)?;
    let mut invitation_changes = invitation.signals();
    let mut directory_changes = directory.signals();
    let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let result = async {
        loop {
            deliver("invitation", &mut invitation)?;
            deliver("directory", &mut directory)?;
            tokio::select! {
                line = input.next_line() => {
                    let Some(line) = line? else { return Ok::<_, anyhow::Error>(()); };
                    let request: Value = serde_json::from_str(&line)?;
                    let command: Command = serde_json::from_value(request["command"].clone())?;
                    match client.execute(command).await {
                        Ok(value) => emit(json!({"id":request["id"],"result":value}))?,
                        Err(error) => emit(json!({"id":request["id"],"error":error.to_string()}))?,
                    }
                }
                result = invitation_changes.changed() => { result?; },
                result = directory_changes.changed() => { result?; },
            }
        }
    }
    .await;
    drop(invitation);
    drop(directory);
    client.pause().await?;
    result
}
