//! Fixtures for this crate's own tests.

use iroh::{
    endpoint::{presets, RelayMode},
    Endpoint, EndpointAddr, TransportAddr,
};
use iroh_base::SecretKey;

/// A loopback-only endpoint speaking one ALPN, with no relay and no discovery.
pub(crate) async fn bare_endpoint(alpn: &'static [u8]) -> Endpoint {
    Endpoint::builder(presets::N0)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .clear_ip_transports()
        .bind_addr(
            "127.0.0.1:0"
                .parse::<std::net::SocketAddr>()
                .expect("valid loopback address"),
        )
        .expect("a loopback bind address")
        .alpns(vec![alpn.to_vec()])
        .bind()
        .await
        .expect("a loopback endpoint binds")
}

/// A scratch store in a temporary directory, kept alive for the store's
/// lifetime.
pub(crate) fn test_store() -> (tempfile::TempDir, std::sync::Arc<synch_store::Store>) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let store = std::sync::Arc::new(synch_store::Store::open(dir.path()).expect("a store"));
    (dir, store)
}

/// The directly bound address of an endpoint, which is all a loopback dial
/// needs.
pub(crate) fn direct_addr(endpoint: &Endpoint) -> EndpointAddr {
    EndpointAddr::from_parts(
        endpoint.id(),
        endpoint.bound_sockets().into_iter().map(TransportAddr::Ip),
    )
}

/// A peer that completes the handshake and then answers nothing — the shape
/// a client deadline exists for: the session stays open, the streams stay
/// open, and no frame ever comes back. Connections are held for as long as
/// the task lives, so nothing on the wire closes and hands the client an
/// error the deadline was not responsible for.
#[allow(missing_debug_implementations)]
pub(crate) struct StalledPeer {
    /// Where to dial it.
    pub addr: EndpointAddr,
    endpoint: Endpoint,
    task: tokio::task::JoinHandle<()>,
}

impl StalledPeer {
    /// Binds one and starts accepting.
    pub(crate) async fn bind(alpn: &'static [u8]) -> StalledPeer {
        let endpoint = bare_endpoint(alpn).await;
        let addr = direct_addr(&endpoint);
        let listening = endpoint.clone();
        let task = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Some(incoming) = listening.accept().await {
                if let Ok(connection) = incoming.await {
                    held.push(connection);
                }
            }
        });
        StalledPeer {
            addr,
            endpoint,
            task,
        }
    }

    /// Stops accepting and closes the endpoint.
    pub(crate) async fn shutdown(self) {
        self.task.abort();
        self.endpoint.close().await;
    }
}

/// A pair of endpoints that trust each other, for exercising a real exchange.
///
/// Membership is unilateral per node (§3.2), so both stores are told about
/// the other's device key before anything is dialled. The client's data
/// directory comes back with them: dropping it would take its database
/// with it.
///
/// `cfg(test)` only: tempfile is a dev-dependency, absent from the `sim`
/// feature build the integration suites link.
#[cfg(test)]
pub(crate) async fn trusting_pair(
    server_store: std::sync::Arc<synch_store::Store>,
    server_options: crate::endpoint::NetOptions,
) -> (
    crate::endpoint::Net,
    crate::endpoint::Net,
    tempfile::TempDir,
) {
    let server_secret = SecretKey::generate();
    let client_secret = SecretKey::generate();
    let client_dir = tempfile::tempdir().expect("a temp dir");
    let client_store =
        std::sync::Arc::new(synch_store::Store::open(client_dir.path()).expect("a client store"));
    trust(&server_store, client_secret.public());
    trust(&client_store, server_secret.public());

    let server = crate::endpoint::Net::bind(server_store, server_secret, server_options)
        .await
        .expect("the server binds");
    let client = crate::endpoint::Net::bind(
        client_store,
        client_secret,
        crate::endpoint::NetOptions::loopback(),
    )
    .await
    .expect("the client binds");
    (server, client, client_dir)
}

/// Binds a device key to an origin of its own in a store, statically.
pub(crate) fn trust(store: &synch_store::Store, key: synch_core::NodeId) {
    store
        .put_binding(&synch_store::Binding {
            origin: synch_core::OriginId::Key(key),
            node_id: key,
            source: synch_store::BindingSource::Static,
            domain: None,
            issuer: None,
            spaces: Vec::new(),
            note: None,
            added_at: 0,
            expires_at: None,
        })
        .expect("a static binding");
}
