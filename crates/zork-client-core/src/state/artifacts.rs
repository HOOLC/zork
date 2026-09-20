use super::Device;

impl Device {
    /// Immutable artifact bytes and offline fallback share the same core cache
    /// on every client. Decoding images and choosing save locations remain UI work.
    pub async fn artifact_content(&self, id: &str) -> Result<Vec<u8>, crate::api::ApiError> {
        if self.snapshot().revoked {
            return Err(crate::api::ApiError::Api {
                status: 403,
                message: "设备访问权限已撤销".into(),
            });
        }
        let generation = self
            .cache
            .as_ref()
            .map(|(store, node)| store.replica_generation(node))
            .transpose()
            .map_err(artifact_cache_error)?
            .unwrap_or(0);
        if let Some((store, node)) = &self.cache {
            if let Some(bytes) = store
                .attachment_blob_at(node, &format!("upload:{id}"), generation)
                .map_err(artifact_cache_error)?
            {
                return Ok(bytes);
            }
        }
        let result = self.client.artifact_content(id).await;
        if self.snapshot().revoked {
            return Err(crate::api::ApiError::Api {
                status: 403,
                message: "设备访问权限已撤销".into(),
            });
        }
        let Some((store, node)) = &self.cache else {
            return result;
        };
        match result {
            Ok(bytes) => {
                store
                    .put_attachment_blob_at(node, id, &bytes, generation)
                    .map_err(artifact_cache_error)?;
                Ok(bytes)
            }
            Err(error) if error.access_revoked() => {
                let _ = self.revoke_replica_access();
                Err(error)
            }
            Err(error @ crate::api::ApiError::Api { .. }) => Err(error),
            Err(error) => match store.attachment_blob_at(node, id, generation) {
                Ok(Some(bytes)) => Ok(bytes),
                _ => Err(error),
            },
        }
    }
}

fn artifact_cache_error(error: anyhow::Error) -> crate::api::ApiError {
    crate::api::ApiError::Task(std::io::Error::other(error.to_string()))
}
