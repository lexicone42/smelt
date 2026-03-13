mod acm;
mod apigateway;
mod autoscaling;
mod cloudfront;
mod cloudwatch;
mod cognito;
mod dynamodb;
mod ec2;
mod ecr;
mod ecs;
mod efs;
mod eks;
mod elasticache;
mod elbv2;
mod eventbridge;
mod iam;
mod kms;
mod lambda;
mod logs;
mod rds;
mod route53;
mod s3;
mod secretsmanager;
mod ses;
mod sfn;
mod sns;
mod sqs;
mod ssm;
mod wafv2;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::provider::*;

/// Dispatch macro for AWS provider operations.
///
/// Eliminates the need to maintain parallel match arms in read/create/update/delete.
/// Each entry maps a type path to its handler methods.
///
/// Usage:
/// ```ignore
/// aws_dispatch!(self, resource_type, provider_id, config, {
///     "ec2.Vpc" => { read: read_vpc, create: create_vpc, update: update_vpc, delete: delete_vpc },
///     ...
///     "ec2.Subnet" => { read: read_subnet, create: create_subnet, replace, delete: delete_subnet },
/// });
/// ```
///
/// The `replace` keyword marks types that return `RequiresReplacement` on update.
macro_rules! aws_dispatch_read {
    ($self:ident, $resource_type:expr, $provider_id:expr, { $($type_path:literal => $read_fn:ident),* $(,)? }) => {
        match $resource_type {
            $( $type_path => $self.$read_fn(&$provider_id).await, )*
            _ => Err(ProviderError::ApiError(format!("unsupported resource type: {}", $resource_type))),
        }
    };
}

macro_rules! aws_dispatch_create {
    ($self:ident, $resource_type:expr, $config:expr, { $($type_path:literal => $create_fn:ident),* $(,)? }) => {
        match $resource_type {
            $( $type_path => $self.$create_fn(&$config).await, )*
            _ => Err(ProviderError::ApiError(format!("unsupported resource type: {}", $resource_type))),
        }
    };
}

macro_rules! aws_dispatch_update {
    ($self:ident, $resource_type:expr, $provider_id:expr, $new_config:expr,
     updatable: { $($up:literal => $update_fn:ident),* $(,)? },
     replace: [ $($rp:literal),* $(,)? ]
    ) => {
        match $resource_type {
            $( $up => $self.$update_fn(&$provider_id, &$new_config).await, )*
            $( $rp )|* => Err(ProviderError::RequiresReplacement("resource changes require replacement".into())),
            _ => Err(ProviderError::ApiError(format!("unsupported resource type: {}", $resource_type))),
        }
    };
}

macro_rules! aws_dispatch_delete {
    ($self:ident, $resource_type:expr, $provider_id:expr, { $($type_path:literal => $delete_fn:ident),* $(,)? }) => {
        match $resource_type {
            $( $type_path => $self.$delete_fn(&$provider_id).await, )*
            _ => Err(ProviderError::ApiError(format!("unsupported resource type: {}", $resource_type))),
        }
    };
}

/// AWS provider implementation backed by the AWS SDK for Rust.
///
/// Covers EC2, IAM, S3, ELBv2, ECS, ECR, RDS, Lambda, Route53,
/// CloudWatch Logs, SQS, SNS, and KMS. Credentials and region are
/// resolved from the standard AWS credential chain.
pub struct AwsProvider {
    pub(crate) ec2_client: aws_sdk_ec2::Client,
    pub(crate) iam_client: aws_sdk_iam::Client,
    pub(crate) s3_client: aws_sdk_s3::Client,
    pub(crate) elbv2_client: aws_sdk_elasticloadbalancingv2::Client,
    pub(crate) ecs_client: aws_sdk_ecs::Client,
    pub(crate) ecr_client: aws_sdk_ecr::Client,
    pub(crate) rds_client: aws_sdk_rds::Client,
    pub(crate) lambda_client: aws_sdk_lambda::Client,
    pub(crate) route53_client: aws_sdk_route53::Client,
    pub(crate) logs_client: aws_sdk_cloudwatchlogs::Client,
    pub(crate) sqs_client: aws_sdk_sqs::Client,
    pub(crate) sns_client: aws_sdk_sns::Client,
    pub(crate) kms_client: aws_sdk_kms::Client,
    pub(crate) dynamodb_client: aws_sdk_dynamodb::Client,
    pub(crate) cloudfront_client: aws_sdk_cloudfront::Client,
    pub(crate) acm_client: aws_sdk_acm::Client,
    pub(crate) secretsmanager_client: aws_sdk_secretsmanager::Client,
    pub(crate) ssm_client: aws_sdk_ssm::Client,
    pub(crate) elasticache_client: aws_sdk_elasticache::Client,
    pub(crate) efs_client: aws_sdk_efs::Client,
    pub(crate) apigateway_client: aws_sdk_apigatewayv2::Client,
    pub(crate) sfn_client: aws_sdk_sfn::Client,
    pub(crate) eventbridge_client: aws_sdk_eventbridge::Client,
    pub(crate) cloudwatch_client: aws_sdk_cloudwatch::Client,
    pub(crate) autoscaling_client: aws_sdk_autoscaling::Client,
    pub(crate) eks_client: aws_sdk_eks::Client,
    pub(crate) wafv2_client: aws_sdk_wafv2::Client,
    pub(crate) cognito_client: aws_sdk_cognitoidentityprovider::Client,
    pub(crate) ses_client: aws_sdk_sesv2::Client,
}

impl AwsProvider {
    /// Create provider from environment — loads AWS config from standard chain.
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self::from_sdk_config(&config)
    }

    /// Create provider with a specific region.
    pub async fn from_region(region: &str) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;
        Self::from_sdk_config(&config)
    }

    /// Create provider from a pre-built SDK config.
    pub fn from_sdk_config(config: &aws_config::SdkConfig) -> Self {
        Self {
            ec2_client: aws_sdk_ec2::Client::new(config),
            iam_client: aws_sdk_iam::Client::new(config),
            s3_client: aws_sdk_s3::Client::new(config),
            elbv2_client: aws_sdk_elasticloadbalancingv2::Client::new(config),
            ecs_client: aws_sdk_ecs::Client::new(config),
            ecr_client: aws_sdk_ecr::Client::new(config),
            rds_client: aws_sdk_rds::Client::new(config),
            lambda_client: aws_sdk_lambda::Client::new(config),
            route53_client: aws_sdk_route53::Client::new(config),
            logs_client: aws_sdk_cloudwatchlogs::Client::new(config),
            sqs_client: aws_sdk_sqs::Client::new(config),
            sns_client: aws_sdk_sns::Client::new(config),
            kms_client: aws_sdk_kms::Client::new(config),
            dynamodb_client: aws_sdk_dynamodb::Client::new(config),
            cloudfront_client: aws_sdk_cloudfront::Client::new(config),
            acm_client: aws_sdk_acm::Client::new(config),
            secretsmanager_client: aws_sdk_secretsmanager::Client::new(config),
            ssm_client: aws_sdk_ssm::Client::new(config),
            elasticache_client: aws_sdk_elasticache::Client::new(config),
            efs_client: aws_sdk_efs::Client::new(config),
            apigateway_client: aws_sdk_apigatewayv2::Client::new(config),
            sfn_client: aws_sdk_sfn::Client::new(config),
            eventbridge_client: aws_sdk_eventbridge::Client::new(config),
            cloudwatch_client: aws_sdk_cloudwatch::Client::new(config),
            autoscaling_client: aws_sdk_autoscaling::Client::new(config),
            eks_client: aws_sdk_eks::Client::new(config),
            wafv2_client: aws_sdk_wafv2::Client::new(config),
            cognito_client: aws_sdk_cognitoidentityprovider::Client::new(config),
            ses_client: aws_sdk_sesv2::Client::new(config),
        }
    }
}

/// Extract tag key-value pairs from a smelt resource config JSON.
pub(crate) fn extract_tags(config: &serde_json::Value) -> HashMap<String, String> {
    use super::ConfigExt;
    let mut tags = HashMap::new();
    if let Some(name) = config.optional_str("/identity/name") {
        tags.insert("Name".to_string(), name.to_string());
    }
    if let Some(tag_map) = config.pointer("/identity/tags").and_then(|v| v.as_object()) {
        for (k, v) in tag_map {
            if let Some(val) = v.as_str() {
                tags.insert(k.clone(), val.to_string());
            }
        }
    }
    tags.insert("managed_by".to_string(), "smelt".to_string());
    tags
}

/// Extract the "Name" tag value from EC2 tag lists.
pub(crate) fn extract_name_tag(tags: &[aws_sdk_ec2::types::Tag]) -> String {
    tags.iter()
        .find(|t| t.key().unwrap_or("") == "Name")
        .and_then(|t| t.value())
        .unwrap_or("")
        .to_string()
}

impl Provider for AwsProvider {
    fn name(&self) -> &str {
        "aws"
    }

    fn resource_types(&self) -> Vec<ResourceTypeInfo> {
        vec![
            // EC2
            Self::ec2_vpc_schema(),
            Self::ec2_subnet_schema(),
            Self::ec2_security_group_schema(),
            Self::ec2_internet_gateway_schema(),
            Self::ec2_route_table_schema(),
            Self::ec2_nat_gateway_schema(),
            Self::ec2_elastic_ip_schema(),
            Self::ec2_key_pair_schema(),
            Self::ec2_instance_schema(),
            Self::ec2_vpc_endpoint_schema(),
            // IAM
            Self::iam_role_schema(),
            Self::iam_policy_schema(),
            Self::iam_instance_profile_schema(),
            // S3
            Self::s3_bucket_schema(),
            // ELBv2
            Self::elbv2_load_balancer_schema(),
            Self::elbv2_target_group_schema(),
            Self::elbv2_listener_schema(),
            // ECS
            Self::ecs_cluster_schema(),
            Self::ecs_service_schema(),
            Self::ecs_task_definition_schema(),
            // ECR
            Self::ecr_repository_schema(),
            // RDS
            Self::rds_db_instance_schema(),
            Self::rds_db_subnet_group_schema(),
            // Lambda
            Self::lambda_function_schema(),
            Self::lambda_event_source_mapping_schema(),
            // Route53
            Self::route53_hosted_zone_schema(),
            Self::route53_record_set_schema(),
            // CloudWatch Logs
            Self::logs_log_group_schema(),
            // SQS
            Self::sqs_queue_schema(),
            // SNS
            Self::sns_topic_schema(),
            // KMS
            Self::kms_key_schema(),
            // DynamoDB
            Self::dynamodb_table_schema(),
            // CloudFront
            Self::cloudfront_distribution_schema(),
            // ACM
            Self::acm_certificate_schema(),
            // Secrets Manager
            Self::secretsmanager_secret_schema(),
            // SSM Parameter Store
            Self::ssm_parameter_schema(),
            // ElastiCache
            Self::elasticache_replication_group_schema(),
            Self::elasticache_cache_subnet_group_schema(),
            // EFS
            Self::efs_file_system_schema(),
            Self::efs_mount_target_schema(),
            // API Gateway v2
            Self::apigateway_api_schema(),
            Self::apigateway_stage_schema(),
            // Step Functions
            Self::sfn_state_machine_schema(),
            // EventBridge
            Self::eventbridge_rule_schema(),
            Self::eventbridge_event_bus_schema(),
            // CloudWatch
            Self::cloudwatch_alarm_schema(),
            // Auto Scaling
            Self::autoscaling_group_schema(),
            // EKS
            Self::eks_cluster_schema(),
            Self::eks_node_group_schema(),
            // WAFv2
            Self::wafv2_web_acl_schema(),
            // Cognito
            Self::cognito_user_pool_schema(),
            // SES
            Self::ses_email_identity_schema(),
        ]
    }

    fn read(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResourceOutput, ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            aws_dispatch_read!(self, resource_type.as_str(), provider_id, {
                // EC2 (hybrid: generated + hand-written)
                "ec2.Vpc" => read_ec2_vpc,
                "ec2.Subnet" => read_ec2_subnet,
                "ec2.SecurityGroup" => read_security_group,
                "ec2.InternetGateway" => read_ec2_internet_gateway,
                "ec2.RouteTable" => read_route_table,
                "ec2.NatGateway" => read_ec2_nat_gateway,
                "ec2.ElasticIp" => read_ec2_elastic_ip,
                "ec2.KeyPair" => read_ec2_key_pair,
                "ec2.Instance" => read_instance,
                "ec2.VpcEndpoint" => read_ec2_vpc_endpoint,
                "iam.Role" => read_role,
                "iam.Policy" => read_policy,
                "iam.InstanceProfile" => read_instance_profile,
                "s3.Bucket" => read_bucket,
                "ecs.Service" => read_ecs_service,
                "ecs.TaskDefinition" => read_task_definition,
                "route53.RecordSet" => read_record_set,
                "cloudfront.Distribution" => read_distribution,
                "apigateway.Stage" => read_stage,
                "autoscaling.Group" => read_asg,
                "eks.Cluster" => read_eks_cluster,
                "eks.NodeGroup" => read_eks_node_group,
                "wafv2.WebACL" => read_web_acl,
                // Generated (codegen)
                "route53.HostedZone" => read_route53_hosted_zone,
                "kms.Key" => read_kms_key,
                "dynamodb.Table" => read_dynamodb_table,
                // Generated (codegen)
                "sqs.Queue" => read_sqs_queue,
                "ssm.Parameter" => read_ssm_parameter,
                "lambda.Function" => read_lambda_function,
                "lambda.EventSourceMapping" => read_lambda_event_source_mapping,
                "logs.LogGroup" => read_logs_log_group,
                "sns.Topic" => read_sns_topic,
                "eventbridge.Rule" => read_eventbridge_rule,
                "eventbridge.EventBus" => read_eventbridge_event_bus,
                "cognito.UserPool" => read_cognito_user_pool,
                "ses.EmailIdentity" => read_ses_email_identity,
                "elbv2.LoadBalancer" => read_elbv2_load_balancer,
                "elbv2.TargetGroup" => read_elbv2_target_group,
                "elbv2.Listener" => read_elbv2_listener,
                // Generated (new resources)
                "rds.DBInstance" => read_rds_db_instance,
                "rds.DBSubnetGroup" => read_rds_db_subnet_group,
                "elasticache.ReplicationGroup" => read_elasticache_replication_group,
                "elasticache.CacheSubnetGroup" => read_elasticache_cache_subnet_group,
                "secretsmanager.Secret" => read_secretsmanager_secret,
                "ecr.Repository" => read_ecr_repository,
                "acm.Certificate" => read_acm_certificate,
                "efs.FileSystem" => read_efs_file_system,
                "efs.MountTarget" => read_efs_mount_target,
                "cloudwatch.Alarm" => read_cloudwatch_alarm,
                "sfn.StateMachine" => read_sfn_state_machine,
                "ecs.Cluster" => read_ecs_cluster,
                "apigateway.Api" => read_apigateway_api,
            })
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
            aws_dispatch_create!(self, resource_type.as_str(), config, {
                // EC2 (hybrid: generated + hand-written)
                "ec2.Vpc" => create_ec2_vpc,
                "ec2.Subnet" => create_ec2_subnet,
                "ec2.SecurityGroup" => create_security_group,
                "ec2.InternetGateway" => create_ec2_internet_gateway,
                "ec2.RouteTable" => create_route_table,
                "ec2.NatGateway" => create_ec2_nat_gateway,
                "ec2.ElasticIp" => create_ec2_elastic_ip,
                "ec2.KeyPair" => create_ec2_key_pair,
                "ec2.Instance" => create_instance,
                "ec2.VpcEndpoint" => create_ec2_vpc_endpoint,
                "iam.Role" => create_role,
                "iam.Policy" => create_policy,
                "iam.InstanceProfile" => create_instance_profile,
                "s3.Bucket" => create_bucket,
                "ecs.Service" => create_ecs_service,
                "ecs.TaskDefinition" => create_task_definition,
                "route53.RecordSet" => create_record_set,
                "cloudfront.Distribution" => create_distribution,
                "apigateway.Stage" => create_stage,
                "autoscaling.Group" => create_asg,
                "eks.Cluster" => create_eks_cluster,
                "eks.NodeGroup" => create_eks_node_group,
                "wafv2.WebACL" => create_web_acl,
                // Generated (codegen)
                "route53.HostedZone" => create_route53_hosted_zone,
                "kms.Key" => create_kms_key,
                "dynamodb.Table" => create_dynamodb_table,
                // Generated (codegen)
                "sqs.Queue" => create_sqs_queue,
                "ssm.Parameter" => create_ssm_parameter,
                "lambda.Function" => create_lambda_function,
                "lambda.EventSourceMapping" => create_lambda_event_source_mapping,
                "logs.LogGroup" => create_logs_log_group,
                "sns.Topic" => create_sns_topic,
                "eventbridge.Rule" => create_eventbridge_rule,
                "eventbridge.EventBus" => create_eventbridge_event_bus,
                "cognito.UserPool" => create_cognito_user_pool,
                "ses.EmailIdentity" => create_ses_email_identity,
                "elbv2.LoadBalancer" => create_elbv2_load_balancer,
                "elbv2.TargetGroup" => create_elbv2_target_group,
                "elbv2.Listener" => create_elbv2_listener,
                "elasticache.ReplicationGroup" => create_elasticache_replication_group,
                "elasticache.CacheSubnetGroup" => create_elasticache_cache_subnet_group,
                // Generated (new resources)
                "rds.DBInstance" => create_rds_db_instance,
                "rds.DBSubnetGroup" => create_rds_db_subnet_group,
                "secretsmanager.Secret" => create_secretsmanager_secret,
                "ecr.Repository" => create_ecr_repository,
                "acm.Certificate" => create_acm_certificate,
                "efs.FileSystem" => create_efs_file_system,
                "efs.MountTarget" => create_efs_mount_target,
                "cloudwatch.Alarm" => create_cloudwatch_alarm,
                "sfn.StateMachine" => create_sfn_state_machine,
                "ecs.Cluster" => create_ecs_cluster,
                "apigateway.Api" => create_apigateway_api,
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
            aws_dispatch_update!(self, resource_type.as_str(), provider_id, new_config,
                updatable: {
                    // EC2 (hybrid: generated + hand-written)
                    "ec2.Vpc" => update_ec2_vpc,
                    "ec2.Instance" => update_instance,
                    "ec2.RouteTable" => update_route_table,
                    "iam.Role" => update_role,
                    "iam.Policy" => update_policy,
                    "s3.Bucket" => update_bucket,
                    "ecs.Service" => update_ecs_service_resource,
                    "route53.RecordSet" => update_record_set,
                    "cloudfront.Distribution" => update_distribution,
                    "apigateway.Stage" => update_stage,
                    "autoscaling.Group" => update_asg,
                    "eks.Cluster" => update_eks_cluster,
                    "eks.NodeGroup" => update_eks_node_group,
                    "wafv2.WebACL" => update_web_acl,
                    // Hand-written (restored)
                    "kms.Key" => update_kms_key,
                    "dynamodb.Table" => update_dynamodb_table,
                    // Generated (codegen)
                    "sqs.Queue" => update_sqs_queue,
                    "ssm.Parameter" => update_ssm_parameter,
                    "lambda.Function" => update_lambda_function,
                    "lambda.EventSourceMapping" => update_lambda_event_source_mapping,
                    "logs.LogGroup" => update_logs_log_group,
                    "sns.Topic" => update_sns_topic,
                    "eventbridge.Rule" => update_eventbridge_rule,
                    "cognito.UserPool" => update_cognito_user_pool,
                    "elbv2.LoadBalancer" => update_elbv2_load_balancer,
                    "elbv2.TargetGroup" => update_elbv2_target_group,
                    "elbv2.Listener" => update_elbv2_listener,
                    "elasticache.ReplicationGroup" => update_elasticache_replication_group,
                    "elasticache.CacheSubnetGroup" => update_elasticache_cache_subnet_group,
                    // Generated (new resources)
                    "secretsmanager.Secret" => update_secretsmanager_secret,
                    "cloudwatch.Alarm" => update_cloudwatch_alarm,
                    "sfn.StateMachine" => update_sfn_state_machine,
                    "apigateway.Api" => update_apigateway_api,
                },
                replace: [
                    "ec2.Subnet",
                    "ec2.InternetGateway",
                    "ec2.NatGateway",
                    "ec2.ElasticIp",
                    "ec2.KeyPair",
                    "ec2.SecurityGroup",
                    "iam.InstanceProfile",
                    "ecs.Cluster",
                    "ecs.TaskDefinition",
                    "ecr.Repository",
                    "rds.DBInstance",
                    "rds.DBSubnetGroup",
                    "route53.HostedZone",
                    "acm.Certificate",
                    "efs.FileSystem",
                    "efs.MountTarget",
                    "ses.EmailIdentity",
                    "ec2.VpcEndpoint",
                    "eventbridge.EventBus",
                ]
            )
        })
    }

    fn delete(
        &self,
        resource_type: &str,
        provider_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        let resource_type = resource_type.to_string();
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            aws_dispatch_delete!(self, resource_type.as_str(), provider_id, {
                // EC2 (hybrid: generated + hand-written)
                "ec2.Vpc" => delete_ec2_vpc,
                "ec2.Subnet" => delete_ec2_subnet,
                "ec2.SecurityGroup" => delete_security_group,
                "ec2.InternetGateway" => delete_ec2_internet_gateway,
                "ec2.RouteTable" => delete_route_table,
                "ec2.NatGateway" => delete_ec2_nat_gateway,
                "ec2.ElasticIp" => delete_ec2_elastic_ip,
                "ec2.KeyPair" => delete_ec2_key_pair,
                "ec2.Instance" => delete_instance,
                "ec2.VpcEndpoint" => delete_ec2_vpc_endpoint,
                "iam.Role" => delete_role,
                "iam.Policy" => delete_policy,
                "iam.InstanceProfile" => delete_instance_profile,
                "s3.Bucket" => delete_bucket,
                "ecs.Service" => delete_ecs_service,
                "ecs.TaskDefinition" => delete_task_definition,
                "route53.RecordSet" => delete_record_set,
                "cloudfront.Distribution" => delete_distribution,
                "apigateway.Stage" => delete_stage,
                "autoscaling.Group" => delete_asg,
                "eks.Cluster" => delete_eks_cluster,
                "eks.NodeGroup" => delete_eks_node_group,
                "wafv2.WebACL" => delete_web_acl,
                // Generated (codegen)
                "route53.HostedZone" => delete_route53_hosted_zone,
                "kms.Key" => delete_kms_key,
                "dynamodb.Table" => delete_dynamodb_table,
                // Generated (codegen)
                "sqs.Queue" => delete_sqs_queue,
                "ssm.Parameter" => delete_ssm_parameter,
                "lambda.Function" => delete_lambda_function,
                "lambda.EventSourceMapping" => delete_lambda_event_source_mapping,
                "logs.LogGroup" => delete_logs_log_group,
                "sns.Topic" => delete_sns_topic,
                "eventbridge.Rule" => delete_eventbridge_rule,
                "eventbridge.EventBus" => delete_eventbridge_event_bus,
                "cognito.UserPool" => delete_cognito_user_pool,
                "ses.EmailIdentity" => delete_ses_email_identity,
                "elbv2.LoadBalancer" => delete_elbv2_load_balancer,
                "elbv2.TargetGroup" => delete_elbv2_target_group,
                "elbv2.Listener" => delete_elbv2_listener,
                "elasticache.ReplicationGroup" => delete_elasticache_replication_group,
                "elasticache.CacheSubnetGroup" => delete_elasticache_cache_subnet_group,
                // Generated (new resources)
                "rds.DBInstance" => delete_rds_db_instance,
                "rds.DBSubnetGroup" => delete_rds_db_subnet_group,
                "secretsmanager.Secret" => delete_secretsmanager_secret,
                "ecr.Repository" => delete_ecr_repository,
                "acm.Certificate" => delete_acm_certificate,
                "efs.FileSystem" => delete_efs_file_system,
                "efs.MountTarget" => delete_efs_mount_target,
                "cloudwatch.Alarm" => delete_cloudwatch_alarm,
                "sfn.StateMachine" => delete_sfn_state_machine,
                "ecs.Cluster" => delete_ecs_cluster,
                "apigateway.Api" => delete_apigateway_api,
            })
        })
    }

    fn diff(
        &self,
        resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();
        super::diff_values("", desired, actual, &mut changes);

        for change in &mut changes {
            change.forces_replacement = match resource_type {
                "ec2.Vpc" => change.path == "network.cidr_block",
                "ec2.Subnet" => matches!(
                    change.path.as_str(),
                    "network.cidr_block" | "network.availability_zone" | "network.vpc_id"
                ),
                "ec2.SecurityGroup" => change.path == "identity.name",
                "ec2.InternetGateway" => false,
                "ec2.RouteTable" => change.path == "network.vpc_id",
                "ec2.NatGateway" | "ec2.ElasticIp" | "ec2.KeyPair" => true,
                "ec2.Instance" => matches!(
                    change.path.as_str(),
                    "sizing.ami_id" | "network.subnet_id" | "network.availability_zone"
                ),
                "iam.Role" => change.path == "identity.name",
                "iam.Policy" => change.path == "identity.name",
                "iam.InstanceProfile" => change.path == "identity.name",
                "s3.Bucket" => change.path == "identity.name",
                "elbv2.LoadBalancer" => change.path == "identity.name",
                "elbv2.TargetGroup" => change.path == "identity.name",
                "elbv2.Listener" => false,
                "ecs.Cluster" => change.path == "identity.name",
                "ecs.Service" => false,
                "ecs.TaskDefinition" => true, // immutable revisions
                "ecr.Repository" => change.path == "identity.name",
                "rds.DBInstance" => matches!(
                    change.path.as_str(),
                    "sizing.engine" | "sizing.engine_version"
                ),
                "rds.DBSubnetGroup" => change.path == "identity.name",
                "lambda.Function" => change.path == "identity.name",
                "route53.HostedZone" => true,
                "route53.RecordSet" => {
                    matches!(change.path.as_str(), "network.name" | "network.record_type")
                }
                "logs.LogGroup" => change.path == "identity.name",
                "sqs.Queue" | "sns.Topic" | "kms.Key" => change.path == "identity.name",
                // Extended services
                "dynamodb.Table" => matches!(
                    change.path.as_str(),
                    "identity.name" | "sizing.partition_key" | "sizing.sort_key"
                ),
                "cloudfront.Distribution" => false, // all fields updatable
                "acm.Certificate" => true,          // domain changes = replacement
                "secretsmanager.Secret" => change.path == "identity.name",
                "ssm.Parameter" => false, // all fields updatable via overwrite
                "elasticache.ReplicationGroup" => {
                    matches!(change.path.as_str(), "identity.name" | "sizing.engine")
                }
                "efs.FileSystem" => change.path == "identity.name",
                "efs.MountTarget" => true, // immutable
                "apigateway.Api" => change.path == "identity.name",
                "apigateway.Stage" => false,
                "sfn.StateMachine" => change.path == "identity.name",
                "eventbridge.Rule" => change.path == "identity.name",
                "cloudwatch.Alarm" => change.path == "identity.name",
                "autoscaling.Group" => change.path == "identity.name",
                "eks.Cluster" => change.path == "identity.name",
                "eks.NodeGroup" => matches!(
                    change.path.as_str(),
                    "identity.name" | "sizing.instance_types"
                ),
                "wafv2.WebACL" => change.path == "identity.name",
                "cognito.UserPool" => change.path == "identity.name",
                "ses.EmailIdentity" => true, // identity changes = replacement
                "ec2.VpcEndpoint" => true,   // VPC endpoints are immutable
                "eventbridge.EventBus" => change.path == "identity.name",
                "elasticache.CacheSubnetGroup" => change.path == "identity.name",
                "lambda.EventSourceMapping" => change.path == "runtime.event_source_arn",
                _ => false,
            };
        }

        changes
    }
}

#[cfg(test)]
impl AwsProvider {
    pub(crate) fn for_testing() -> Self {
        macro_rules! test_client {
            ($sdk:ident) => {{
                let config = $sdk::Config::builder()
                    .behavior_version($sdk::config::BehaviorVersion::latest())
                    .region($sdk::config::Region::new("us-east-1"))
                    .build();
                $sdk::Client::from_conf(config)
            }};
        }
        Self {
            ec2_client: test_client!(aws_sdk_ec2),
            iam_client: test_client!(aws_sdk_iam),
            s3_client: test_client!(aws_sdk_s3),
            elbv2_client: test_client!(aws_sdk_elasticloadbalancingv2),
            ecs_client: test_client!(aws_sdk_ecs),
            ecr_client: test_client!(aws_sdk_ecr),
            rds_client: test_client!(aws_sdk_rds),
            lambda_client: test_client!(aws_sdk_lambda),
            route53_client: test_client!(aws_sdk_route53),
            logs_client: test_client!(aws_sdk_cloudwatchlogs),
            sqs_client: test_client!(aws_sdk_sqs),
            sns_client: test_client!(aws_sdk_sns),
            kms_client: test_client!(aws_sdk_kms),
            dynamodb_client: test_client!(aws_sdk_dynamodb),
            cloudfront_client: test_client!(aws_sdk_cloudfront),
            acm_client: test_client!(aws_sdk_acm),
            secretsmanager_client: test_client!(aws_sdk_secretsmanager),
            ssm_client: test_client!(aws_sdk_ssm),
            elasticache_client: test_client!(aws_sdk_elasticache),
            efs_client: test_client!(aws_sdk_efs),
            apigateway_client: test_client!(aws_sdk_apigatewayv2),
            sfn_client: test_client!(aws_sdk_sfn),
            eventbridge_client: test_client!(aws_sdk_eventbridge),
            cloudwatch_client: test_client!(aws_sdk_cloudwatch),
            autoscaling_client: test_client!(aws_sdk_autoscaling),
            eks_client: test_client!(aws_sdk_eks),
            wafv2_client: test_client!(aws_sdk_wafv2),
            cognito_client: test_client!(aws_sdk_cognitoidentityprovider),
            ses_client: test_client!(aws_sdk_sesv2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_provider_has_all_resource_types() {
        let provider = AwsProvider::for_testing();
        let types = provider.resource_types();
        assert_eq!(types.len(), 52);

        let paths: Vec<_> = types.iter().map(|t| t.type_path.as_str()).collect();
        assert!(paths.contains(&"ec2.Vpc"));
        assert!(paths.contains(&"ec2.Instance"));
        assert!(paths.contains(&"iam.Role"));
        assert!(paths.contains(&"s3.Bucket"));
        assert!(paths.contains(&"elbv2.LoadBalancer"));
        assert!(paths.contains(&"ecs.Cluster"));
        assert!(paths.contains(&"rds.DBInstance"));
        assert!(paths.contains(&"lambda.Function"));
        assert!(paths.contains(&"route53.HostedZone"));
        assert!(paths.contains(&"logs.LogGroup"));
        assert!(paths.contains(&"sqs.Queue"));
        assert!(paths.contains(&"sns.Topic"));
        assert!(paths.contains(&"kms.Key"));
    }

    #[test]
    fn aws_provider_diff() {
        let provider = AwsProvider::for_testing();

        let desired = serde_json::json!({
            "network": { "cidr_block": "10.0.0.0/16", "dns_support": true }
        });
        let actual = serde_json::json!({
            "network": { "cidr_block": "10.0.0.0/8", "dns_support": true }
        });

        let changes = provider.diff("ec2.Vpc", &desired, &actual);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "network.cidr_block");
        assert_eq!(changes[0].change_type, ChangeType::Modify);
        assert!(changes[0].forces_replacement);
    }

    #[test]
    fn vpc_schema_has_semantic_sections() {
        let schema = AwsProvider::ec2_vpc_schema();
        let section_names: Vec<_> = schema
            .schema
            .sections
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(section_names.contains(&"identity"));
        assert!(section_names.contains(&"network"));
    }

    #[test]
    fn diff_marks_replacement_fields() {
        let provider = AwsProvider::for_testing();

        let desired = serde_json::json!({
            "network": { "cidr_block": "10.0.0.0/16", "availability_zone": "us-east-1b" }
        });
        let actual = serde_json::json!({
            "network": { "cidr_block": "10.0.0.0/16", "availability_zone": "us-east-1a" }
        });

        let changes = provider.diff("ec2.Subnet", &desired, &actual);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].forces_replacement);
    }

    // ─── Schema Invariant Tests ──────────────────────────────────────

    #[test]
    fn all_resource_types_have_identity_section() {
        let provider = AwsProvider::for_testing();
        let types = provider.resource_types();

        for rt in &types {
            let has_identity = rt.schema.sections.iter().any(|s| s.name == "identity");
            assert!(
                has_identity,
                "resource type '{}' is missing an 'identity' section",
                rt.type_path
            );
        }
    }

    #[test]
    fn all_resource_types_have_name_field_in_identity() {
        let provider = AwsProvider::for_testing();
        let types = provider.resource_types();

        for rt in &types {
            let identity = rt.schema.sections.iter().find(|s| s.name == "identity");
            if let Some(identity) = identity {
                let has_name = identity.fields.iter().any(|f| f.name == "name");
                assert!(
                    has_name,
                    "resource type '{}' identity section is missing 'name' field",
                    rt.type_path
                );
            }
        }
    }

    #[test]
    fn required_fields_have_no_default() {
        let provider = AwsProvider::for_testing();
        let types = provider.resource_types();

        for rt in &types {
            for section in &rt.schema.sections {
                for field in &section.fields {
                    if field.required {
                        assert!(
                            field.default.is_none(),
                            "resource '{}' field '{}.{}' is required but has a default",
                            rt.type_path,
                            section.name,
                            field.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn enum_fields_have_at_least_two_variants() {
        let provider = AwsProvider::for_testing();
        let types = provider.resource_types();

        for rt in &types {
            for section in &rt.schema.sections {
                for field in &section.fields {
                    if let FieldType::Enum(variants) = &field.field_type {
                        assert!(
                            variants.len() >= 2,
                            "resource '{}' field '{}.{}' enum has fewer than 2 variants: {:?}",
                            rt.type_path,
                            section.name,
                            field.name,
                            variants
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn no_duplicate_field_names_within_sections() {
        let provider = AwsProvider::for_testing();
        let types = provider.resource_types();

        for rt in &types {
            for section in &rt.schema.sections {
                let mut seen = std::collections::HashSet::new();
                for field in &section.fields {
                    assert!(
                        seen.insert(&field.name),
                        "resource '{}' section '{}' has duplicate field '{}'",
                        rt.type_path,
                        section.name,
                        field.name
                    );
                }
            }
        }
    }
}
