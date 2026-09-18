# Platform TLS verifier source

Source: `rustls-platform-verifier` 0.7.0 from crates.io, checksum
`26d1e2536ce4f35f4846aa13bff16bd0ff40157cdb14cc056c7b14ba41233ba0`.
The original VCS metadata, licenses, manifest and certificate fixtures are retained.

The macOS patch marks synchronous `SecTrust` evaluation as blocking when called
on a Tokio multithread runtime. Tokio can then continue other tasks on replacement
workers. System trust roots, hostname checks, revocation checks, verification
dates, signature verification and error propagation are unchanged. Other callers
retain the upstream synchronous behavior.

Run the patch's scheduler regression and the retained certificate fixtures with
`cargo test --manifest-path vendor/rustls-platform-verifier/Cargo.toml --locked --lib`.
The standalone test lock pins the same Rustls, Tokio and Apple trust dependencies
as the application lock.
