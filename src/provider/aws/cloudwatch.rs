use std::collections::HashMap;

use crate::provider::*;

use super::AwsProvider;

impl AwsProvider {
    pub(super) async fn create_alarm(
        &self,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        let alarm_name = config
            .pointer("/identity/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("identity.name is required".into()))?;

        let namespace = config
            .pointer("/sizing/namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.namespace is required".into()))?;

        let metric_name = config
            .pointer("/sizing/metric_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.metric_name is required".into()))?;

        let statistic = config
            .pointer("/sizing/statistic")
            .and_then(|v| v.as_str())
            .unwrap_or("Average");

        let period = config
            .pointer("/sizing/period")
            .and_then(|v| v.as_i64())
            .unwrap_or(300) as i32;

        let evaluation_periods = config
            .pointer("/sizing/evaluation_periods")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let threshold = config
            .pointer("/sizing/threshold")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.threshold is required".into()))?;

        let comparison_operator = config
            .pointer("/sizing/comparison_operator")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("sizing.comparison_operator is required".into())
            })?;

        let mut req = self
            .cloudwatch_client
            .put_metric_alarm()
            .alarm_name(alarm_name)
            .namespace(namespace)
            .metric_name(metric_name)
            .statistic(aws_sdk_cloudwatch::types::Statistic::from(statistic))
            .period(period)
            .evaluation_periods(evaluation_periods)
            .threshold(threshold)
            .comparison_operator(aws_sdk_cloudwatch::types::ComparisonOperator::from(
                comparison_operator,
            ));

        // Optional description
        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.alarm_description(desc);
        }

        // Dimensions
        if let Some(dims) = config
            .pointer("/sizing/dimensions")
            .and_then(|v| v.as_array())
        {
            for dim in dims {
                if let (Some(n), Some(v)) = (
                    dim.get("name").and_then(|v| v.as_str()),
                    dim.get("value").and_then(|v| v.as_str()),
                ) {
                    req = req.dimensions(
                        aws_sdk_cloudwatch::types::Dimension::builder()
                            .name(n)
                            .value(v)
                            .build(),
                    );
                }
            }
        }

        // Tags
        let tags = super::extract_tags(config);
        for (k, v) in &tags {
            req = req.tags(
                aws_sdk_cloudwatch::types::Tag::builder()
                    .key(k)
                    .value(v)
                    .build(),
            );
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("PutMetricAlarm: {e}")))?;

        self.read_alarm(alarm_name).await
    }

    pub(super) async fn read_alarm(
        &self,
        alarm_name: &str,
    ) -> Result<ResourceOutput, ProviderError> {
        let result = self
            .cloudwatch_client
            .describe_alarms()
            .alarm_names(alarm_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DescribeAlarms: {e}")))?;

        let alarm = result
            .metric_alarms()
            .first()
            .ok_or_else(|| ProviderError::NotFound(format!("Alarm {alarm_name}")))?;

        let state = serde_json::json!({
            "identity": {
                "name": alarm.alarm_name().unwrap_or(""),
                "description": alarm.alarm_description().unwrap_or(""),
            },
            "sizing": {
                "namespace": alarm.namespace().unwrap_or(""),
                "metric_name": alarm.metric_name().unwrap_or(""),
                "statistic": alarm.statistic().map(|s| s.as_str()).unwrap_or("Average"),
                "period": alarm.period(),
                "evaluation_periods": alarm.evaluation_periods(),
                "threshold": alarm.threshold(),
                "comparison_operator": alarm.comparison_operator()
                    .map(|c| c.as_str()).unwrap_or(""),
            }
        });

        let mut outputs = HashMap::new();
        outputs.insert(
            "alarm_arn".into(),
            serde_json::json!(alarm.alarm_arn().unwrap_or("")),
        );
        outputs.insert(
            "alarm_name".into(),
            serde_json::json!(alarm.alarm_name().unwrap_or("")),
        );

        Ok(ResourceOutput {
            provider_id: alarm_name.to_string(),
            state,
            outputs,
        })
    }

    pub(super) async fn update_alarm(
        &self,
        alarm_name: &str,
        config: &serde_json::Value,
    ) -> Result<ResourceOutput, ProviderError> {
        // put_metric_alarm is idempotent — same as create
        let namespace = config
            .pointer("/sizing/namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.namespace is required".into()))?;

        let metric_name = config
            .pointer("/sizing/metric_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.metric_name is required".into()))?;

        let statistic = config
            .pointer("/sizing/statistic")
            .and_then(|v| v.as_str())
            .unwrap_or("Average");

        let period = config
            .pointer("/sizing/period")
            .and_then(|v| v.as_i64())
            .unwrap_or(300) as i32;

        let evaluation_periods = config
            .pointer("/sizing/evaluation_periods")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let threshold = config
            .pointer("/sizing/threshold")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| ProviderError::InvalidConfig("sizing.threshold is required".into()))?;

        let comparison_operator = config
            .pointer("/sizing/comparison_operator")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::InvalidConfig("sizing.comparison_operator is required".into())
            })?;

        let mut req = self
            .cloudwatch_client
            .put_metric_alarm()
            .alarm_name(alarm_name)
            .namespace(namespace)
            .metric_name(metric_name)
            .statistic(aws_sdk_cloudwatch::types::Statistic::from(statistic))
            .period(period)
            .evaluation_periods(evaluation_periods)
            .threshold(threshold)
            .comparison_operator(aws_sdk_cloudwatch::types::ComparisonOperator::from(
                comparison_operator,
            ));

        if let Some(desc) = config
            .pointer("/identity/description")
            .and_then(|v| v.as_str())
        {
            req = req.alarm_description(desc);
        }

        if let Some(dims) = config
            .pointer("/sizing/dimensions")
            .and_then(|v| v.as_array())
        {
            for dim in dims {
                if let (Some(n), Some(v)) = (
                    dim.get("name").and_then(|v| v.as_str()),
                    dim.get("value").and_then(|v| v.as_str()),
                ) {
                    req = req.dimensions(
                        aws_sdk_cloudwatch::types::Dimension::builder()
                            .name(n)
                            .value(v)
                            .build(),
                    );
                }
            }
        }

        req.send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("PutMetricAlarm: {e}")))?;

        self.read_alarm(alarm_name).await
    }

    pub(super) async fn delete_alarm(&self, alarm_name: &str) -> Result<(), ProviderError> {
        self.cloudwatch_client
            .delete_alarms()
            .alarm_names(alarm_name)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("DeleteAlarms: {e}")))?;
        Ok(())
    }

    pub(super) fn cloudwatch_alarm_schema() -> ResourceTypeInfo {
        ResourceTypeInfo {
            type_path: "cloudwatch.Alarm".into(),
            description: "CloudWatch metric alarm".into(),
            schema: ResourceSchema {
                sections: vec![
                    SectionSchema {
                        name: "identity".into(),
                        description: "Alarm identification".into(),
                        fields: vec![
                            FieldSchema {
                                name: "name".into(),
                                description: "Alarm name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "description".into(),
                                description: "Alarm description".into(),
                                field_type: FieldType::String,
                                required: false,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                    SectionSchema {
                        name: "sizing".into(),
                        description: "Metric and threshold configuration".into(),
                        fields: vec![
                            FieldSchema {
                                name: "namespace".into(),
                                description: "CloudWatch namespace (e.g., AWS/EC2)".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "metric_name".into(),
                                description: "Metric name".into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "statistic".into(),
                                description: "Statistic to evaluate".into(),
                                field_type: FieldType::Enum(vec![
                                    "Average".into(),
                                    "Sum".into(),
                                    "Minimum".into(),
                                    "Maximum".into(),
                                    "SampleCount".into(),
                                ]),
                                required: false,
                                default: Some(serde_json::json!("Average")),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "period".into(),
                                description: "Evaluation period in seconds".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(300)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "evaluation_periods".into(),
                                description: "Number of periods to evaluate".into(),
                                field_type: FieldType::Integer,
                                required: false,
                                default: Some(serde_json::json!(1)),
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "threshold".into(),
                                description: "Threshold value".into(),
                                field_type: FieldType::Float,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                            FieldSchema {
                                name: "comparison_operator".into(),
                                description: "Comparison operator (e.g., GreaterThanThreshold)"
                                    .into(),
                                field_type: FieldType::String,
                                required: true,
                                default: None,
                                sensitive: false,
                            },
                        ],
                    },
                ],
            },
        }
    }
}
