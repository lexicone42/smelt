use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_certificate(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let domain_name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let validation_method = config
            .pointer("/security/validation_method")
            .and_then(|v| v.as_str())
            .unwrap_or("DNS");

        let vm = match validation_method {
            "EMAIL" => aws_sdk_acm::types::ValidationMethod::Email,
            _ => aws_sdk_acm::types::ValidationMethod::Dns,
        };

        let mut req = self
            .acm_client
            .request_certificate()
            .domain_name(domain_name)
            .validation_method(vm);

        // Subject alternative names
        if let Some(sans) = config
            .pointer("/network/subject_alternative_names")
            .and_then(|v| v.as_array())
        {
            let san_strings: Vec<String> = sans
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            req = req.set_subject_alternative_names(Some(san_strings));
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_acm::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build()
                    .unwrap(),
            );
        }

        let result = req
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("RequestCertificate: {e}")))?;

        let arn = result
            .certificate_arn()
            .ok_or_else(|| ProviderError::ApiError("RequestCertificate returned no ARN".into()))?;

        self.read_certificate(arn).await
    }

    pub(super) async fn read_certificate(
        &self,
        arn: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .acm_client
            .describe_certificate()
            .certificate_arn(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeCertificate: {e}")))?;

        let cert = result
            .certificate()
            .ok_or_else(|| ProviderError::NotFound(format!("Certificate {arn}")))?;

        let domain_name = cert.domain_name().unwrap_or("");
        let status = cert.status().map(|s| s.as_str()).unwrap_or("");
        let cert_type = cert.r#type().map(|t| t.as_str()).unwrap_or("");
        let validation_method = cert
            .domain_validation_options()
            .first()
            .and_then(|d| d.validation_method())
            .map(|m| m.as_str())
            .unwrap_or("DNS");
        let not_before = cert.not_before().map(|t| t.to_string()).unwrap_or_default();
        let not_after = cert.not_after().map(|t| t.to_string()).unwrap_or_default();

        let state = serde_json::json!({
            "identity": {
                "name": domain_name,
            },
            "security": {
                "validation_method": validation_method,
                "status": status,
                "type": cert_type,
                "not_before": not_before,
                "not_after": not_after,
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "certificate_arn".into(),
            serde_json::json!(cert.certificate_arn().unwrap_or("")),
        );
        outputs.insert("domain_name".into(), serde_json::json!(domain_name));

        Ok(ResourceOutput {
            provider_id: arn.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn delete_certificate(&self, arn: &str) -> Result<(), ProviderError> {
        self.acm_client
            .delete_certificate()
            .certificate_arn(arn)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteCertificate: {e}")))?;
        Ok(())
    }

    pub(super) fn acm_certificate_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "acm.Certificate".into(),
            description: "ACM TLS/SSL certificate".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Certificate identification".into(),
                        fields: vec![FieldSchema {
                            name: "name".into(),
                            description: "Domain name for the certificate".into(),
                            field_type: FieldType::String,
                            required: true,
                            default: None,
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "security".into(),
                        description: "Certificate validation".into(),
                        fields: vec![FieldSchema {
                            name: "validation_method".into(),
                            description: "Validation method".into(),
                            field_type: FieldType::Enum(vec!["DNS".into(), "EMAIL".into()]),
                            required: false,
                            default: Some(serde_json::json!("DNS")),
                            sensitive: false,
                        }],
                    },
                    SectionSchema {
                        name: "network".into(),
                        description: "Additional domains".into(),
                        fields: vec![FieldSchema {
                            name: "subject_alternative_names".into(),
                            description: "Subject alternative names".into(),
                            field_type: FieldType::Array(Box::new(FieldType::String)),
                            required: false,
                            default: None,
                            sensitive: false,
                        }],
                    },
                ],
            },
        }
    }
}
