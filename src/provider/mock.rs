//! Mock provider for testing the apply engine, output passing, and parallel execution.
//!
//! The mock provider simulates cloud resources with deterministic, controllable behavior.
//! It generates provider IDs, tracks state in-memory, and produces configurable outputs.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    ChangeType, FieldChange, Provider, ProviderError, ResourceOutput, ResourceSchema,
    ResourceTypeInfo, SectionSchema,
};

/// A mock provider that simulates cloud resources for testing.
///
/// Resources are tracked in-memory. Provider IDs are deterministic
/// (based on resource type + sequential counter). Behavior can be
/// configured per resource type or per resource name.
pub struct MockProvider {
    counter: AtomicU64,
    /// Resources currently "alive" in the mock cloud: provider_id -> state
    resources: Mutex<HashMap<String, serde_json::Value>>,
    /// Configure specific resources to fail: resource type -> error message
    fail_on_create: Mutex<HashMap<String, String>>,
    /// Configure specific resources to require replacement on update
    force_replacement: Mutex<Vec<String>>,
    /// Simulated latency in milliseconds (for testing parallel execution)
    latency_ms: u64,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            counter: AtomicU64::new(1),
            resources: Mutex::new(HashMap::new()),
            fail_on_create: Mutex::new(HashMap::new()),
            force_replacement: Mutex::new(Vec::new()),
            latency_ms: 0,
        }
    }

    /// Set simulated latency for all operations (milliseconds).
    pub fn with_latency(mut self, ms: u64) -> Self {
        self.latency_ms = ms;
        self
    }

    /// Configure a resource type to fail on create with the given error message.
    pub fn fail_create(&self, resource_type: &str, error: &str) {
        self.fail_on_create
            .lock()
            .unwrap()
            .insert(resource_type.to_string(), error.to_string());
    }

    /// Configure a resource type to require replacement on update.
    pub fn require_replacement(&self, resource_type: &str) {
        self.force_replacement
            .lock()
            .unwrap()
            .push(resource_type.to_string());
    }

    fn next_id(&self, resource_type: &str) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        // Generate deterministic IDs like "mock-vpc-1", "mock-subnet-2"
        let short_type = resource_type
            .split('.')
            .next_back()
            .unwrap_or(resource_type)
            .to_lowercase();
        format!("mock-{short_type}-{n}")
    }

    /// Build outputs from the config — simulates real providers returning
    /// computed values like ARNs, endpoints, IPs.
    fn build_outputs(
        &self,
        resource_type: &str,
        provider_id: &str,
        config: &serde_json::Value,
    ) -> HashMap<String, serde_json::Value> {
        let mut outputs = HashMap::new();

        // Every resource gets an ARN-like output
        outputs.insert(
            "arn".to_string(),
            serde_json::json!(format!("arn:mock::{resource_type}/{provider_id}")),
        );

        // Extract the name from identity section if present
        if let Some(name) = config.pointer("/identity/name").and_then(|v| v.as_str()) {
            outputs.insert("name".to_string(), serde_json::json!(name));
        }

        // Type-specific outputs that mirror real cloud behavior
        let short_type = resource_type.split('.').next_back().unwrap_or("");
        match short_type {
            "Vpc" => {
                if let Some(cidr) = config.pointer("/network/cidr_block") {
                    outputs.insert("cidr_block".to_string(), cidr.clone());
                }
            }
            "Subnet" => {
                outputs.insert("available_ips".to_string(), serde_json::json!(251));
            }
            "Instance" => {
                outputs.insert("private_ip".to_string(), serde_json::json!("10.0.1.42"));
                outputs.insert("public_ip".to_string(), serde_json::json!("203.0.113.42"));
            }
            "SecurityGroup" => {
                outputs.insert("group_id".to_string(), serde_json::json!(provider_id));
            }
            "DBInstance" => {
                outputs.insert(
                    "endpoint".to_string(),
                    serde_json::json!(format!("{provider_id}.mock.rds.amazonaws.com:5432")),
                );
            }
            "Bucket" => {
                if let Some(name) = config.pointer("/identity/name").and_then(|v| v.as_str()) {
                    outputs.insert(
                        "domain_name".to_string(),
                        serde_json::json!(format!("{name}.s3.amazonaws.com")),
                    );
                }
            }
            _ => {}
        }

        outputs
    }

    async fn simulate_latency(&self) {
        if self.latency_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.latency_ms)).await;
        }
    }
}

impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn resource_types(&self) -> Vec<ResourceTypeInfo> {
        // Provide schemas for common resource types used in tests
        vec![
            ResourceTypeInfo {
                type_path: "test.Vpc".to_string(),
                description: "Mock VPC".to_string(),
                schema: ResourceSchema {
                    sections: vec![
                        SectionSchema {
                            name: "identity".to_string(),
                            description: "Identity".to_string(),
                            fields: vec![],
                        },
                        SectionSchema {
                            name: "network".to_string(),
                            description: "Network config".to_string(),
                            fields: vec![],
                        },
                    ],
                },
            },
            ResourceTypeInfo {
                type_path: "test.Subnet".to_string(),
                description: "Mock Subnet".to_string(),
                schema: ResourceSchema {
                    sections: vec![
                        SectionSchema {
                            name: "identity".to_string(),
                            description: "Identity".to_string(),
                            fields: vec![],
                        },
                        SectionSchema {
                            name: "network".to_string(),
                            description: "Network config".to_string(),
                            fields: vec![],
                        },
                    ],
                },
            },
            ResourceTypeInfo {
                type_path: "test.Instance".to_string(),
                description: "Mock Instance".to_string(),
                schema: ResourceSchema {
                    sections: vec![
                        SectionSchema {
                            name: "identity".to_string(),
                            description: "Identity".to_string(),
                            fields: vec![],
                        },
                        SectionSchema {
                            name: "compute".to_string(),
                            description: "Compute config".to_string(),
                            fields: vec![],
                        },
                    ],
                },
            },
            ResourceTypeInfo {
                type_path: "test.SecurityGroup".to_string(),
                description: "Mock Security Group".to_string(),
                schema: ResourceSchema {
                    sections: vec![SectionSchema {
                        name: "identity".to_string(),
                        description: "Identity".to_string(),
                        fields: vec![],
                    }],
                },
            },
        ]
    }

    fn read(
        &self,
        _resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            self.simulate_latency().await;
            let resources = self.resources.lock().unwrap();
            match resources.get(&provider_id) {
                Some(state) => Ok(ResourceOutput {
                    provider_id: provider_id.clone(),
                    state: state.clone(),
                    outputs: HashMap::new(),
                }),
                None => Err(ProviderError::NotFound(provider_id)),
            }
        })
    }

    fn create(
        &self,
        resource_type: &str,
        config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let config = config.clone();
        Box::pin(async move {
            self.simulate_latency().await;

            // Check for configured failures
            if let Some(error) = self.fail_on_create.lock().unwrap().get(&resource_type) {
                return Err(ProviderError::ApiError(error.clone()));
            }

            let provider_id = self.next_id(&resource_type);
            let outputs = self.build_outputs(&resource_type, &provider_id, &config);

            // Store the resource state
            self.resources
                .lock()
                .unwrap()
                .insert(provider_id.clone(), config.clone());

            Ok(ResourceOutput {
                provider_id,
                state: config,
                outputs,
            })
        })
    }

    fn update(
        &self,
        resource_type: &str,
        provider_id: &str,
        _old_config: &serde_json::Value,
        new_config: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        let new_config = new_config.clone();
        Box::pin(async move {
            self.simulate_latency().await;

            // Check for forced replacement
            if self
                .force_replacement
                .lock()
                .unwrap()
                .contains(&resource_type)
            {
                return Err(ProviderError::RequiresReplacement(
                    "mock: forced replacement".to_string(),
                ));
            }

            let outputs = self.build_outputs(&resource_type, &provider_id, &new_config);

            // Update stored state
            self.resources
                .lock()
                .unwrap()
                .insert(provider_id.clone(), new_config.clone());

            Ok(ResourceOutput {
                provider_id,
                state: new_config,
                outputs,
            })
        })
    }

    fn delete(
        &self,
        _resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            self.simulate_latency().await;
            self.resources.lock().unwrap().remove(&provider_id);
            Ok(())
        })
    }

    fn diff(
        &self,
        _resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        // Simple field-level diff
        let mut changes = Vec::new();
        if let (Some(desired_obj), Some(actual_obj)) = (desired.as_object(), actual.as_object()) {
            for (key, desired_val) in desired_obj {
                match actual_obj.get(key) {
                    None => changes.push(FieldChange {
                        path: key.clone(),
                        change_type: ChangeType::Add,
                        old_value: None,
                        new_value: Some(desired_val.clone()),
                        forces_replacement: false,
                    }),
                    Some(actual_val) if actual_val != desired_val => {
                        changes.push(FieldChange {
                            path: key.clone(),
                            change_type: ChangeType::Modify,
                            old_value: Some(actual_val.clone()),
                            new_value: Some(desired_val.clone()),
                            forces_replacement: false,
                        });
                    }
                    _ => {}
                }
            }
            for key in actual_obj.keys() {
                if !desired_obj.contains_key(key) {
                    changes.push(FieldChange {
                        path: key.clone(),
                        change_type: ChangeType::Remove,
                        old_value: actual_obj.get(key).cloned(),
                        new_value: None,
                        forces_replacement: false,
                    });
                }
            }
        }
        changes
    }
}
