use crate::model::{ServiceDefinition, ServiceId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Mutex;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub revision: u64,
    pub definitions: BTreeMap<ServiceId, StoredDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDefinition {
    pub definition_revision: u64,
    pub definition: ServiceDefinition,
}

/// Persistence boundary for durable definitions. A store replacement is a
/// compare-and-swap over the whole small registry, so two writers can never
/// silently lose each other's update.
pub trait RegistryStore: Send + Sync {
    fn load(&self) -> Result<RegistrySnapshot, RegistryStoreError>;

    fn replace(
        &self,
        expected_registry_revision: u64,
        definitions: BTreeMap<ServiceId, StoredDefinition>,
    ) -> Result<RegistrySnapshot, RegistryStoreError>;
}

#[derive(Debug, Default)]
pub struct MemoryRegistry {
    snapshot: Mutex<RegistrySnapshot>,
}

impl MemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(snapshot: RegistrySnapshot) -> Self {
        Self {
            snapshot: Mutex::new(snapshot),
        }
    }
}

impl RegistryStore for MemoryRegistry {
    fn load(&self) -> Result<RegistrySnapshot, RegistryStoreError> {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| RegistryStoreError::Unavailable("registry lock was poisoned".to_owned()))
    }

    fn replace(
        &self,
        expected_registry_revision: u64,
        definitions: BTreeMap<ServiceId, StoredDefinition>,
    ) -> Result<RegistrySnapshot, RegistryStoreError> {
        let mut snapshot = self.snapshot.lock().map_err(|_| {
            RegistryStoreError::Unavailable("registry lock was poisoned".to_owned())
        })?;
        if snapshot.revision != expected_registry_revision {
            return Err(RegistryStoreError::Conflict {
                expected: expected_registry_revision,
                actual: snapshot.revision,
            });
        }
        snapshot.revision = snapshot.revision.checked_add(1).ok_or_else(|| {
            RegistryStoreError::Unavailable("registry revision overflow".to_owned())
        })?;
        snapshot.definitions = definitions;
        Ok(snapshot.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryStoreError {
    Conflict { expected: u64, actual: u64 },
    Unavailable(String),
}

impl fmt::Display for RegistryStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict { expected, actual } => write!(
                f,
                "registry compare-and-swap conflict: expected revision {expected}, actual {actual}"
            ),
            Self::Unavailable(message) => write!(f, "registry unavailable: {message}"),
        }
    }
}

impl std::error::Error for RegistryStoreError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencyGraphError {
    Missing {
        service_id: ServiceId,
        dependency_id: ServiceId,
    },
    Cycle {
        path: Vec<ServiceId>,
    },
}

/// Validates that every dependency exists and rejects cycles. The traversal is
/// deterministic because service IDs and dependency sets are ordered.
pub fn validate_dependency_graph(
    definitions: &BTreeMap<ServiceId, StoredDefinition>,
) -> Result<(), DependencyGraphError> {
    for (service_id, stored) in definitions {
        for dependency_id in &stored.definition.dependencies {
            if !definitions.contains_key(dependency_id) {
                return Err(DependencyGraphError::Missing {
                    service_id: service_id.clone(),
                    dependency_id: dependency_id.clone(),
                });
            }
        }
    }

    let mut complete = BTreeSet::new();
    let mut active = BTreeSet::new();
    let mut stack = Vec::new();
    for service_id in definitions.keys() {
        visit(
            service_id,
            definitions,
            &mut complete,
            &mut active,
            &mut stack,
        )?;
    }
    Ok(())
}

fn visit(
    service_id: &ServiceId,
    definitions: &BTreeMap<ServiceId, StoredDefinition>,
    complete: &mut BTreeSet<ServiceId>,
    active: &mut BTreeSet<ServiceId>,
    stack: &mut Vec<ServiceId>,
) -> Result<(), DependencyGraphError> {
    if complete.contains(service_id) {
        return Ok(());
    }
    if active.contains(service_id) {
        let start = stack
            .iter()
            .position(|candidate| candidate == service_id)
            .unwrap_or(0);
        let mut path = stack[start..].to_vec();
        path.push(service_id.clone());
        return Err(DependencyGraphError::Cycle { path });
    }

    active.insert(service_id.clone());
    stack.push(service_id.clone());
    let stored = &definitions[service_id];
    let dependencies: BTreeSet<_> = stored.definition.dependencies.iter().cloned().collect();
    for dependency_id in dependencies {
        visit(&dependency_id, definitions, complete, active, stack)?;
    }
    stack.pop();
    active.remove(service_id);
    complete.insert(service_id.clone());
    Ok(())
}

pub fn topological_order(
    definitions: &BTreeMap<ServiceId, StoredDefinition>,
) -> Result<Vec<ServiceId>, DependencyGraphError> {
    validate_dependency_graph(definitions)?;
    let mut complete = BTreeSet::new();
    let mut ordered = Vec::with_capacity(definitions.len());
    for service_id in definitions.keys() {
        order_visit(service_id, definitions, &mut complete, &mut ordered);
    }
    Ok(ordered)
}

fn order_visit(
    service_id: &ServiceId,
    definitions: &BTreeMap<ServiceId, StoredDefinition>,
    complete: &mut BTreeSet<ServiceId>,
    ordered: &mut Vec<ServiceId>,
) {
    if complete.contains(service_id) {
        return;
    }
    let dependencies: BTreeSet<_> = definitions[service_id]
        .definition
        .dependencies
        .iter()
        .cloned()
        .collect();
    for dependency_id in dependencies {
        order_visit(&dependency_id, definitions, complete, ordered);
    }
    complete.insert(service_id.clone());
    ordered.push(service_id.clone());
}
