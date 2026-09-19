//! Process-wide rustls provider selection, and the trust anchors every TLS
//! client here verifies against.
//!
//! # Trust anchors
//!
//! Server certificates are checked against the *host's* trust store, never a
//! copy of Mozilla's roots compiled into the binary. A compiled-in bundle
//! ages with the release: a root the machine has since distrusted stays
//! trusted until the operator upgrades, and — the case that actually bites —
//! an enterprise or private CA the operator installed system-wide is not
//! trusted at all, so a node behind a TLS-inspecting proxy or talking to a
//! self-hosted relay cannot connect without rebuilding. Deferring to the host
//! means `update-ca-certificates`, Keychain, and the Windows certificate store
//! are what decide, which is what an operator already expects to be in charge.
//!
//! [`rustls_platform_verifier`] is the one implementation: the platform APIs
//! on macOS and Windows, and `/etc/ssl` (honouring `SSL_CERT_FILE` and
//! `SSL_CERT_DIR`) elsewhere. Three clients reach it three ways, all landing
//! on the same verifier:
//!
//! - reqwest — DoH, the monitor's tiles, the S3 gateway — uses it by default
//!   as of 0.13, so nothing is configured at those call sites.
//! - iroh's relay and pkarr clients take [`CaTlsConfig::system`], set in
//!   [`endpoint`](crate::endpoint).
//! - The cloud-attach WebSocket takes a [`rustls::ClientConfig`] from
//!   [`client_config`] below.
//!
//! [`CaTlsConfig::system`]: iroh::tls::CaTlsConfig::system
//!
//! # Provider
//!
//! The workspace standardizes on [`aws_lc_rs`]: iroh is built with
//! `tls-aws-lc-rs` and hickory with `dnssec-aws-lc-rs`. reqwest therefore
//! takes `rustls-no-provider` rather than its `rustls` feature — the
//! provider is named in one place, and a second one arriving through a
//! dependency's features would leave rustls unable to guess a process
//! default, panicking on the first TLS handshake.
//!
//! With no provider installed, reqwest panics as soon as a `Client` is
//! *built*, so the provider cannot wait for rustls's fallback (pick the one
//! provider the crate features name, at the first handshake). Every binary
//! installs it as the first thing its `main` does, via
//! [`install_crypto_provider`]. Test binaries have no `main` of ours to
//! install from, so `sim` builds — the feature every test-carrying crate
//! dev-depends on — install it before `main` instead, with [`ctor`].

use std::sync::{Arc, OnceLock};

use rustls::{crypto::aws_lc_rs, ClientConfig};
use rustls_platform_verifier::ConfigVerifierExt;

use crate::error::NetError;

/// A rustls client config that verifies against the host's trust store.
///
/// For the TLS clients that do not build their own — today the cloud-attach
/// WebSocket, which otherwise gets tungstenite's compiled-in roots.
///
/// Built once and shared. Reading the host store is not free (a keychain
/// query, or every file under `/etc/ssl/certs`), and attach dials in a
/// reconnect loop: paying that per attempt would put a syscall storm behind
/// every flap. The store is therefore read at the first TLS connection of the
/// process and not again, so a root added or revoked afterwards takes a
/// restart to see — the same bargain reqwest makes by holding a `Client`.
pub fn client_config() -> Result<Arc<ClientConfig>, NetError> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            ClientConfig::with_platform_verifier()
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(NetError::Tls)
}

/// Install `aws-lc-rs` as the process-wide rustls [`CryptoProvider`].
///
/// [`CryptoProvider`]: rustls::crypto::CryptoProvider
///
/// Idempotent and race-safe: a second call loses the race to install and the
/// error is dropped, because the outcome is the same provider either way.
pub fn install_crypto_provider() {
    let _ = aws_lc_rs::default_provider().install_default();
}

/// The test-binary stand-in for the `main` every shipped binary opens with.
#[cfg(feature = "sim")]
#[ctor::ctor]
fn pre_main_install() {
    install_crypto_provider();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The config builds, and the store behind it is read once: the second
    /// caller gets the first caller's `Arc`, not a second walk of `/etc/ssl`.
    #[test]
    fn client_config_is_built_once() {
        let first = client_config().expect("host trust store");
        let second = client_config().expect("host trust store");
        assert!(Arc::ptr_eq(&first, &second));
    }
}
