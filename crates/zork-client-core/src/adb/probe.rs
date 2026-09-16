use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Verify the ADB wire header, rather than treating any open TCP port as adbd.
pub(super) async fn probe(port: u16, facts: &Facts) -> LocalState {
    let result = tokio::time::timeout(Duration::from_millis(800), async {
        let mut socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        let payload = b"host::\0";
        let command = u32::from_le_bytes(*b"CNXN");
        for word in [
            command,
            0x01000000,
            4096,
            payload.len() as u32,
            payload.iter().map(|v| *v as u32).sum(),
            command ^ u32::MAX,
        ] {
            socket.write_u32_le(word).await?;
        }
        socket.write_all(payload).await?;
        let mut header = [0; 24];
        socket.read_exact(&mut header).await?;
        let word = |i| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
        anyhow::ensure!(
            word(20) == word(0) ^ u32::MAX && word(12) <= 1024 * 1024,
            "invalid ADB header"
        );
        match &header[..4] {
            b"CNXN" | b"AUTH" => Ok(LocalState::Ready),
            b"STLS" => Ok(LocalState::WirelessOnly),
            _ => anyhow::bail!("not an ADB endpoint"),
        }
    })
    .await;
    if let Ok(Ok(state)) = result {
        return state;
    }
    if facts.developer_enabled == Some(false) {
        LocalState::DeveloperDisabled
    } else if facts.usb_enabled == Some(false) {
        LocalState::UsbDisabled
    } else if facts.wireless_enabled == Some(true) {
        LocalState::WirelessOnly
    } else {
        LocalState::ActivationRequired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn distinguishes_adb_from_open_ports_and_system_switches() {
        for (response, expected) in [
            (*b"AUTH", LocalState::Ready),
            (*b"CNXN", LocalState::Ready),
            (*b"STLS", LocalState::WirelessOnly),
            (*b"HTTP", LocalState::ActivationRequired),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 31];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(&request[..4], b"CNXN");
                for word in [
                    u32::from_le_bytes(response),
                    1,
                    0,
                    0,
                    0,
                    u32::from_le_bytes(response) ^ u32::MAX,
                ] {
                    socket.write_u32_le(word).await.unwrap();
                }
            });
            assert_eq!(probe(port, &Facts::default()).await, expected);
            server.await.unwrap();
        }
        let facts = Facts {
            developer_enabled: Some(false),
            ..Default::default()
        };
        assert_eq!(probe(0, &facts).await, LocalState::DeveloperDisabled);
    }
}
