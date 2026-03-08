use crate::provider::*;

use super::GcpProvider;

impl GcpProvider {
    // ─── storage.Bucket ─────────────────────────────────────────────────

    pub(super) fn storage_bucket_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "storage.Bucket".into(),
            description: "Cloud Storage bucket".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Bucket identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Globally unique bucket name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "labels".into(),
                                description: "Key-value labels".into(),
                                field_type: FieldType::Record(vec![]),
                                required: false,
                                default: Some(serde_json::json!({})),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "reliability".into(),
                        description: "Durability and storage class settings".into(),
                        fields: vec![
                            FieldSchema {
                                name: "storage_class".into(),
                                description: "Storage class for the bucket".into(),
                                field_type: FieldType::Enum(vec![
                                    "STANDARD".into(),
                                    "NEARLINE".into(),
                                    "COLDLINE".into(),
                                    "ARCHIVE".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("STANDARD")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "versioning".into(),
                                description: "Enable object versioning".into(),
                                field_type: FieldType::Bool,
                                required: false,
                                default: Some(serde_json::json!(false)),
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Access control settings".into(),
                        fields: vec![FieldSchema {
                            name: "uniform_bucket_level_access".into(),
                            description: "Enable uniform bucket-level access (disables ACLs)"
                                .into(),
                            field_type: FieldType::Bool,
                            required: false,
                            default: Some(serde_json::json!(true)),
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Location settings".into(),
                        fields: vec![FieldSchema {
                            name: "location".into(),
                            description: "Bucket location (e.g., \"US\", \"us-central1\")".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }

    pub(super) async fn create_bucket(
        &self,
        _config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // The google-cloud-storage crate is focused on object operations, not
        // bucket lifecycle management. Bucket CRUD requires the JSON API or
        // the storage-control client which is not yet available in the Rust SDK.
        Err(ProviderError::ApiError(
            "CreateBucket: not yet implemented via storage SDK".into(),
        ))
    }

    pub(super) async fn read_bucket(&self, _name: &str) -> Result<ResourceOutput, ProviderError> {
        Err(ProviderError::ApiError(
            "GetBucket: not yet implemented via storage SDK".into(),
        ))
    }

    pub(super) async fn update_bucket(
        &self,
        _name: &str,
        _config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        Err(ProviderError::ApiError(
            "UpdateBucket: not yet implemented via storage SDK".into(),
        ))
    }

    pub(super) async fn delete_bucket(&self, _name: &str) -> Result<(), ProviderError> {
        Err(ProviderError::ApiError(
            "DeleteBucket: not yet implemented via storage SDK".into(),
        ))
    }
}
