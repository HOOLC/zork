//! DNSSEC membership end to end: a simulated signed zone, the real resolver,
//! and a real node adopting what validates (§3.2).

use iroh_base::SecretKey;
use synch_core::OriginId;
use synch_net::{sim::SimZone, DnssecResolver, RekorPolicy, ResolverOptions};

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validated_records_become_bindings_and_outages_keep_them() {
    // The body drives the world synchronously; the node's runtime workers stay checked (§10).
    let _blocking = synch_core::BlockingScope::enter();
    let peer = common::spawn_node("laptop").await;
    let node = &peer.node;

    let nas = SecretKey::generate().public();
    let zone = SimZone::new(
        "cluster.example",
        vec![format!("v=sync1 id=nas nk={}", nas.to_z32())],
    );
    let anchor = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(anchor.path(), zone.anchor_record()).unwrap();
    let (url, server) = zone.serve().await;

    let resolver = DnssecResolver::with_options(&ResolverOptions {
        doh_url: Some(url),
        trust_anchor: Some(anchor.path().to_path_buf()),
        // DNSSEC-only: the zone-key transparency path has its own suite, and this zone logs nothing.
        rekor: Some(RekorPolicy::Off),
        rekor_key: None,
        rekor_state: None,
        rekor_config: None,
        tuf_url: None,
        no_tuf: true,
        tuf_root: None,
    })
    .unwrap();

    node.set_domain("cluster.example").unwrap();
    let outcomes = node
        .refresh_domains_named(&resolver, Some("cluster.example"))
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    let refresh = outcomes[0].result.as_ref().expect("a validated refresh");
    assert_eq!(refresh.bindings, 1, "{refresh:?}");

    let origin = OriginId::named("nas", "cluster.example").unwrap();
    let now = synch_core::now_ns();
    assert!(node.store().is_bound(&origin, &nas, now).unwrap());

    // The zone goes dark: the refresh reports the failure and the binding
    // stays exactly as cached — fail closed shrinks the member set by expiry, never by outage.
    server.abort();
    let outcomes = node
        .refresh_domains_named(&resolver, Some("cluster.example"))
        .await
        .unwrap();
    assert!(outcomes[0].result.is_err(), "{outcomes:?}");
    assert!(node.store().is_bound(&origin, &nas, now).unwrap());
    let health = node.domain_health().unwrap();
    assert!(health[0].schedule.as_ref().unwrap().last_error.is_some());

    node.shutdown().await.unwrap();
}
