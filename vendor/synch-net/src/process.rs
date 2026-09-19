//! The preamble and exit conventions every synchronicity binary shares.
//!
//! `synch`, `synch-s3` and `synch-monitor` each open the same way — install
//! the process TLS provider, then a `SYNCH_LOG`-driven stderr subscriber —
//! and close the same way: a reader that hung up early is the reader saying
//! "enough", not a failure. Two of the three had hand-copied the opening and
//! the third had only half of it, which left `SYNCH_LOG` dead there and a
//! broken pipe reported as an incomplete run against its documented exit
//! codes.

/// Installs the process TLS provider and the `SYNCH_LOG` tracing subscriber.
///
/// The provider first, before anything builds a TLS client: reqwest, built
/// without a baked-in provider, refuses to construct a `Client` until one is
/// installed. The subscriber writes to stderr and falls back to
/// `default_filter` when `SYNCH_LOG` is unset or unparsable.
pub fn init(default_filter: &str) {
    crate::tls::install_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("SYNCH_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter)),
        )
        .with_writer(std::io::stderr)
        .init();
}

/// Whether an error chain bottoms out in the reader hanging up.
///
/// Rust starts processes with SIGPIPE ignored, so a reader that quit early
/// (`synch ls | head`) surfaces as a broken-pipe write error. That is the
/// reader saying "enough", not a failure to report: the process should exit 0
/// without printing anything.
pub fn reader_hung_up(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut cause = Some(error);
    while let Some(e) = cause {
        if e.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
        {
            return true;
        }
        cause = e.source();
    }
    false
}
