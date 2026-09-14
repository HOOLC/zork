//! Lazy, host-owned tool namespaces. Resolution is pure and has no external effects.
use super::*;

/// A namespace advertises one introduction and materializes exact tool contracts
/// on demand. No API inventory or resolved-name cache is kept by the runtime.
pub trait ToolNamespace: Send + Sync {
    fn introduction(&self) -> ToolIntroduction;
    fn resolve(&self, name: &str) -> Option<Arc<ToolInstance>>;
}

pub(super) struct NamespaceEntry {
    current: bool,
    provider: Arc<dyn ToolNamespace>,
}

impl ToolRegistry {
    pub fn register_namespace(
        &self,
        prefix: &str,
        namespace: Arc<dyn ToolNamespace>,
    ) -> Result<(), ToolDefinitionError> {
        if !prefix.ends_with('.') || prefix.len() < 2 {
            return Err(ToolDefinitionError::InvalidNamespace);
        }
        let introduction = namespace.introduction();
        if introduction.name != format!("{prefix}*") || introduction.description.trim().is_empty() {
            return Err(ToolDefinitionError::InvalidNamespace);
        }
        self.namespaces
            .write()
            .expect("tool namespaces lock poisoned")
            .insert(
                prefix.into(),
                NamespaceEntry {
                    current: true,
                    provider: namespace,
                },
            );
        Ok(())
    }

    pub fn remove_namespace(&self, prefix: &str) -> bool {
        let mut namespaces = self
            .namespaces
            .write()
            .expect("tool namespaces lock poisoned");
        let Some(entry) = namespaces.get_mut(prefix) else {
            return false;
        };
        std::mem::replace(&mut entry.current, false)
    }

    pub(super) fn instance(&self, name: &str) -> Option<Arc<ToolInstance>> {
        // An exact entry, including an explicitly removed entry, wins.
        if let Some(entry) = self
            .entries
            .read()
            .expect("tool registry lock poisoned")
            .get(name)
        {
            return entry.current.clone();
        }
        self.namespace_instance(name, false)
    }

    pub(super) fn namespace_instance(
        &self,
        name: &str,
        historical: bool,
    ) -> Option<Arc<ToolInstance>> {
        let namespace = self
            .namespaces
            .read()
            .expect("tool namespaces lock poisoned")
            .iter()
            .filter(|(prefix, _)| name.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .and_then(|(_, entry)| (entry.current || historical).then(|| entry.provider.clone()));
        let instance = namespace?.resolve(name)?;
        // A faulty provider cannot resolve one name as another tool.
        (instance.contract().name == name).then_some(instance)
    }

    pub(super) fn namespace_introductions(&self) -> Vec<ToolIntroduction> {
        let namespaces = self
            .namespaces
            .read()
            .expect("tool namespaces lock poisoned")
            .values()
            .filter(|entry| entry.current)
            .map(|entry| entry.provider.clone())
            .collect::<Vec<_>>();
        namespaces.iter().map(|n| n.introduction()).collect()
    }
}
