//! Descriptor soak for real endpoint, proxy, enrollment and discovery ownership.
use anyhow::{ensure, Result};
use std::{path::Path, time::Duration};
use zork_config::MeshConfig;
use zork_mesh::{enrollment::Enrollment, managed};

fn descriptors() -> Result<usize> {
    let root = if Path::new("/proc/self/fd").exists() {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    Ok(std::fs::read_dir(root)?.count())
}
async fn cycle(root: &Path, graceful: bool) -> Result<usize> {
    let baseline = descriptors()?;
    let config = MeshConfig {
        enabled: true,
        relay_urls: Some(vec!["http://127.0.0.1:9".into()]),
        discovery_url: Some("http://127.0.0.1:9/pkarr".into()),
        quic_discovery_urls: Some(vec![]),
        ..Default::default()
    };
    let mut runtime = managed::start_client(root, &config).await?;
    let enrollment = Enrollment::bind_for_node(root, &config, &runtime.node()).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let peak = descriptors()?;
    if graceful {
        enrollment.close().await;
        runtime.shutdown().await?;
    } else {
        drop(enrollment);
        drop(runtime);
    }
    // Drop requests asynchronous shutdown. Wait for its bounded drain instead
    // of confusing descriptors still closing with a accumulating leak.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if descriptors()? <= baseline + 4 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("endpoint resources did not drain after ownership ended"))??;
    Ok(peak)
}
#[tokio::main]
async fn main() -> Result<()> {
    let root = tempfile::tempdir()?;
    for index in 0..3 {
        cycle(&root.path().join(format!("warm-{index}")), true).await?;
    }
    let baseline = descriptors()?;
    let mut peak = baseline;
    let mut samples = Vec::new();
    for index in 0..40 {
        peak = peak.max(cycle(&root.path().join(format!("node-{index}")), index % 2 == 0).await?);
        samples.push(descriptors()?);
    }
    let final_count = descriptors()?;
    ensure!(
        samples.iter().all(|count| *count <= baseline + 4),
        "descriptor leak: baseline={baseline}, samples={samples:?}"
    );
    println!(
        "{}",
        serde_json::json!({"rounds":40,"baseline":baseline,"peak":peak,"after_shutdown":final_count,"samples":samples,"graceful_and_drop":true,"online_endpoints":true})
    );
    Ok(())
}
