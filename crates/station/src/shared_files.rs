//! The authenticated HTTP view of the same Synch tree used by local clients.
//! Business file ownership and publication do not belong to this view.
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use zork_mesh::node::{MeshNode, ObjectRef, TreeCatalog, TreePage, TreeQuery};

pub struct SharedFiles {
    node: MeshNode,
}
impl SharedFiles {
    pub fn new(node: MeshNode) -> Arc<Self> {
        Arc::new(Self { node })
    }
    pub async fn catalog(&self) -> Result<TreeCatalog> {
        self.node
            .tree_space(zork_config::tree::SHARED_FILES_SPACE)
            .await
    }
    pub async fn directory(&self, query: TreeQuery) -> Result<TreePage> {
        anyhow::ensure!(
            query.space == zork_config::tree::SHARED_FILES_SPACE,
            "not a file-sharing space"
        );
        self.node.tree_directory(query).await
    }
    pub async fn content(&self, object: ObjectRef) -> Result<Vec<u8>> {
        anyhow::ensure!(
            object.space == zork_config::tree::SHARED_FILES_SPACE,
            "not a file-sharing space"
        );
        self.node.tree_read(&object).await
    }
    pub async fn subscribe(
        self: &Arc<Self>,
        query: Option<TreeQuery>,
    ) -> Result<tokio::sync::mpsc::Receiver<Value>> {
        let source = self.node.source_changes().await?;
        let mut changes = source.subscribe();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let this = self.clone();
        tokio::spawn(async move {
            let _source = source;
            let mut previous = None;
            loop {
                changes.checkpoint();
                let frame = async {
                    let catalog = this.catalog().await?;
                    let page = match &query {
                        Some(query) => Some(this.directory(query.clone()).await?),
                        None => None,
                    };
                    Ok::<_, anyhow::Error>(json!({"catalog":catalog,"page":page}))
                }
                .await
                .unwrap_or_else(|error| json!({"error":error.to_string()}));
                if previous.as_ref() != Some(&frame) {
                    tokio::select! {
                        sent=tx.send(json!({"name":"shared_files","data":frame}))=>if sent.is_err(){return;},
                        _=tx.closed()=>return,
                    }
                    previous = Some(frame);
                }
                tokio::select! { _=tx.closed()=>return,changed=changes.changed()=>if changed.is_err(){return;} }
            }
        });
        Ok(rx)
    }
}
