//! Process fixture using the same core workflow and applied-batch contract as Android.
use anyhow::{Context, Result};
use std::io::Write;
use zork_client_core::{subscriptions::Key, Client, Command};
#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().context("client root")?);
    let ticket = args.next().context("invitation")?;
    let cancel = args.next().as_deref() == Some("cancel");
    let mut client = Client::open(&root)?;
    client
        .execute(Command::BeginInvitation {
            ticket,
            name: "Notification fixture".into(),
            switch_from: None,
        })
        .await?;
    let mut observer = client.local().observe(Key::Invitation)?;
    let mut signals = observer.signals();
    let result = tokio::time::timeout(std::time::Duration::from_secs(65), async {
        loop {
            if let Some(frame) = observer.prepare()? {
                let value = &frame["snapshot"];
                println!("{value}");
                std::io::stdout().flush()?;
                observer.finish(frame["batch"].as_u64().unwrap(), true);
                if cancel && value["invitation"]["status"] == "awaiting_approval" {
                    client.execute(Command::CancelInvitation).await?;
                }
                if value["done"] == true {
                    return Ok::<_, anyhow::Error>(());
                }
            }
            signals.changed().await?;
        }
    })
    .await?;
    drop(observer);
    client.pause().await?;
    result
}
