//! Authenticated endpoint observations keep group bootstrap hints current.
use super::*;
use futures_util::StreamExt;
use zork_config::membership::MeshRoutes;

impl EnrollmentService {
    pub(crate) async fn update_routes(
        &self,
        state: &AppState,
        origin: &str,
        routes: MeshRoutes,
    ) -> Result<Value> {
        routes.validate()?;
        let service = state.mesh.get().context("mesh_not_ready")?;
        let group = zork_config::load_config(&self.root)?
            .mesh
            .group
            .context("mesh_membership_missing")?;
        let member = group
            .members
            .iter()
            .find(|member| member.origin == origin)
            .context("mesh_member_required")?;
        if member.routes.as_ref() == Some(&routes) {
            return Ok(json!({"group":group}));
        }
        if group.authority != service.origin() {
            ensure!(origin == service.origin(), "can_only_update_own_routes");
            let result = service
                .membership_call(&group.authority, "update_routes", json!({"routes":routes}))
                .await?;
            apply_group(
                state,
                &group.authority,
                serde_json::from_value(result["group"].clone())?,
            )
            .await?;
            return Ok(result);
        }
        let _guard = self.transaction.lock().await;
        let group = zork_config::update_config(&self.root, |config| {
            let mut group = config
                .mesh
                .group
                .clone()
                .context("mesh_membership_missing")?;
            ensure!(group.authority == self.origin, "mesh_authority_changed");
            let member = group
                .members
                .iter_mut()
                .find(|member| member.origin == origin)
                .context("mesh_member_required")?;
            if member.routes.as_ref() != Some(&routes) {
                member.routes = Some(routes);
                group.revision += 1;
                group.apply(service.origin(), &mut config.mesh)?;
            }
            Ok(group)
        })?;
        service.refresh(state).await?;
        self.broadcast(state, &group).await;
        Ok(json!({"group":group}))
    }
}

pub fn start(service: Arc<EnrollmentService>, state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Ok(mut addresses) = service.node.addresses() else {
            return;
        };
        let mut membership = state.db.realtime.listen(crate::realtime::MESH);
        let mut retry = zork_mesh::retry::DiscoveryBackoff::default();
        loop {
            membership.checkpoint();
            let result: Result<()> = async {
                let config = zork_config::load_config(&service.root)?;
                if config
                    .mesh
                    .group
                    .as_ref()
                    .is_some_and(|group| group.contains(&service.origin))
                {
                    service
                        .update_routes(&state, &service.origin, service.node.routes()?)
                        .await?;
                }
                Ok(())
            }
            .await;
            let failed = result.is_err();
            if let Err(error) = result {
                tracing::debug!(%error, "Mesh route publication will retry");
            } else {
                retry.reset();
            }
            let delay = if failed {
                retry.next_delay()
            } else {
                std::time::Duration::ZERO
            };
            tokio::select! {
                _ = tokio::time::sleep(delay), if failed => {},
                address = addresses.next() => if address.is_none() { return; },
                changed = membership.changed() => if changed.is_err() { return; },
            }
        }
    })
}
