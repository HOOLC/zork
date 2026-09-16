//! Cloud attach: the daemon's outbound tunnel to the control plane.
//!
//! A node behind NAT has no inbound path, so the only connection that can
//! exist is one it opens itself. This module is that connection: a supervised
//! task beside the scanner and the fetcher which, unless the operator has
//! opted it out, discovers where its base's control plane lives from the same
//! DNSSEC-validated zone that names the membership, dials out, proves itself
//! with the device key it already holds, and answers listing and read requests
//! through the same entry points the S3 gateway reads through.
//!
//! Two facts hold it in place:
//!
//! * **What is served is the control plane's call.** The tunnel is on by
//!   default and answers for every space this node holds; which spaces a
//!   dashboard may browse is decided on the other end, by the org admin's
//!   toggle and the RBAC around it. The one local act is an opt-out —
//!   `synch control-plane disable` — and the supervisor drops an open tunnel on its
//!   next pass, seconds later, not at the next reconnect.
//! * **Nothing here can write.** [`frame`] encodes no write opcode, and the
//!   handlers below call `unified_listing`, `versions`, `resolve_set`,
//!   `providers_for`, `fetch_range` and `Store::read_range` and nothing else.

pub mod attach;
pub mod frame;

use crate::{error::Result, node::Node};

/// Where the opt-out lives in the daemon's config namespace.
///
/// `cloud.*`, exactly as the gateway keeps `s3.*`: one row per setting, in the
/// table that already survives restarts, so an operator's choice is not a
/// process's memory of it. No row at all is the default state, and the
/// default is attached — on is where a node starts, not a state it reaches.
const OPT_OUT_KEY: &str = "cloud.disabled";

/// What the operator has said about cloud attach on this node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloudSettings {
    /// Whether the operator has opted this node out. `false` — the derived
    /// default, and the state of a node nobody has configured — is attached.
    pub disabled: bool,
}

/// What the attach task has achieved for one endpoint of one membership
/// domain.
///
/// Reported by `synch control-plane status` and by nothing else: it is a running
/// process's account of itself, so it lives in memory and dies with the
/// daemon rather than pretending to be durable.
///
/// One row per *endpoint*, not per domain: an apex names every node of its
/// control plane and this daemon holds a tunnel to each, so "attached" is a
/// fact about a node. Collapsed to one row per domain, a fleet with one dead
/// replica would read as either attached or detached, and both readings are
/// wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDomainStatus {
    /// The membership domain this attach is for.
    pub domain: String,
    /// The attach URL discovered from the zone, if the lookup validated.
    pub endpoint: Option<String>,
    /// Whether a tunnel is open right now.
    pub attached: bool,
    /// The last failure, kept until something succeeds.
    pub last_error: Option<String>,
}

/// What one row of the status map is keyed by: the domain, and the endpoint
/// within it — `None` before discovery has produced one, which is the row a
/// failing lookup leaves behind.
pub(crate) type CloudKey = (String, Option<String>);

impl Node {
    /// What the operator has said about cloud attach on this node.
    pub fn cloud_settings(&self) -> Result<CloudSettings> {
        let disabled = self.store().config(OPT_OUT_KEY)?.as_deref() == Some("1");
        Ok(CloudSettings { disabled })
    }

    /// Reopens the tunnel after [`Self::disable_cloud`].
    ///
    /// There is nothing to name and nothing to choose: the tunnel is on by
    /// default and serves whatever the control plane requests, so this is
    /// only ever the undo of an opt-out.
    pub fn enable_cloud(&self) -> Result<CloudSettings> {
        self.store().set_config(OPT_OUT_KEY, "0")?;
        self.cloud_settings()
    }

    /// Opts this node out: no tunnel is opened, and one that is open is
    /// dropped by the supervisor on its next pass.
    pub fn disable_cloud(&self) -> Result<()> {
        self.store().set_config(OPT_OUT_KEY, "1")?;
        Ok(())
    }

    /// What the attach task has achieved, per endpoint of per membership
    /// domain.
    pub fn cloud_status(&self) -> Vec<CloudDomainStatus> {
        let mut out: Vec<CloudDomainStatus> = self
            .cloud_slot()
            .values()
            .cloned()
            .collect::<Vec<CloudDomainStatus>>();
        out.sort_by(|a, b| (&a.domain, &a.endpoint).cmp(&(&b.domain, &b.endpoint)));
        out
    }

    /// Records what one endpoint's attach is doing now.
    pub(crate) fn set_cloud_status(
        &self,
        domain: &str,
        endpoint: Option<String>,
        attached: bool,
        last_error: Option<String>,
    ) {
        self.cloud_slot().insert(
            (domain.to_string(), endpoint.clone()),
            CloudDomainStatus {
                domain: domain.to_string(),
                endpoint,
                attached,
                last_error,
            },
        );
    }

    /// Forgets a domain the operator has removed, so `control-plane status` does not
    /// report on something that is no longer configured.
    pub(crate) fn forget_cloud_status(&self, domain: &str) {
        self.cloud_slot().retain(|(held, _), _| held != domain);
    }

    /// Forgets one endpoint the zone stopped naming — a replica taken out of
    /// the record — while the domain's other tunnels carry on.
    pub(crate) fn forget_cloud_endpoint(&self, domain: &str, endpoint: &str) {
        self.cloud_slot()
            .remove(&(domain.to_string(), Some(endpoint.to_string())));
    }

    /// Forgets the endpoint-less row a failed discovery round leaves — the
    /// one that says "no validated record" for the domain as a whole.
    ///
    /// Its own key, because nothing else can clear it: it names no endpoint,
    /// so the per-endpoint sweep never reaches it, and one transient resolver
    /// failure at startup would otherwise sit above the real rows in `synch
    /// control-plane status` for the life of the daemon.
    pub(crate) fn forget_cloud_endpoint_none(&self, domain: &str) {
        self.cloud_slot().remove(&(domain.to_string(), None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::node;

    /// Status is per endpoint, and the two kinds of row are forgotten
    /// separately.
    ///
    /// A failed discovery round writes a row that names no endpoint — "this
    /// domain has no validated record" — and the per-endpoint sweep cannot
    /// reach it, because it is keyed by an endpoint that is not there. One
    /// transient resolver failure at startup would otherwise leave a
    /// permanent bogus line above the real ones in `synch control-plane status`.
    #[tokio::test]
    async fn cloud_status_is_per_endpoint_and_forgettable() {
        let (_d, node) = node().await;
        node.set_cloud_status("cluster.example", None, false, Some("no record".into()));
        node.set_cloud_status(
            "cluster.example",
            Some("https://cp.example".into()),
            true,
            None,
        );
        node.set_cloud_status(
            "cluster.example",
            Some("https://ns1.cp.example".into()),
            false,
            Some("refused".into()),
        );
        node.set_cloud_status("other.example", Some("https://cp.other".into()), true, None);
        assert_eq!(node.cloud_status().len(), 4);

        // Discovery recovers: the endpoint-less row goes, the tunnels stay.
        node.forget_cloud_endpoint_none("cluster.example");
        assert!(node.cloud_status().iter().all(|s| s.endpoint.is_some()));

        // The zone stops naming one node: its row goes and no other does.
        node.forget_cloud_endpoint("cluster.example", "https://ns1.cp.example");
        let left: Vec<(String, Option<String>)> = node
            .cloud_status()
            .into_iter()
            .map(|s| (s.domain, s.endpoint))
            .collect();
        assert_eq!(
            left,
            [
                (
                    "cluster.example".to_string(),
                    Some("https://cp.example".to_string())
                ),
                (
                    "other.example".to_string(),
                    Some("https://cp.other".to_string())
                ),
            ]
        );

        // The operator removes the domain: every one of its rows goes, and
        // the other domain's do not.
        node.forget_cloud_status("cluster.example");
        assert_eq!(
            node.cloud_status()
                .iter()
                .map(|s| s.domain.clone())
                .collect::<Vec<_>>(),
            ["other.example"]
        );
        node.shutdown().await.unwrap();
    }

    /// The disable/enable round-trip lives in `attach` against what it gates;
    /// what belongs here is that no row at all means attached.
    #[tokio::test]
    async fn attach_is_on_by_default() {
        let (_d, node) = node().await;
        assert_eq!(node.cloud_settings().unwrap(), CloudSettings::default());
        node.shutdown().await.unwrap();
    }
}
