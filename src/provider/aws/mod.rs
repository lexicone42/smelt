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
                "ec2.Vpc" => read_vpc,
                "ec2.Subnet" => read_subnet,
                "ec2.SecurityGroup" => read_security_group,
                "ec2.InternetGateway" => read_internet_gateway,
                "ec2.RouteTable" => read_route_table,
                "ec2.NatGateway" => read_nat_gateway,
                "ec2.ElasticIp" => read_elastic_ip,
                "ec2.KeyPair" => read_key_pair,
                "ec2.Instance" => read_instance,
                "iam.Role" => read_role,
                "iam.Policy" => read_policy,
                "iam.InstanceProfile" => read_instance_profile,
                "s3.Bucket" => read_bucket,
                "elbv2.LoadBalancer" => read_load_balancer,
                "elbv2.TargetGroup" => read_target_group,
                "elbv2.Listener" => read_listener,
                "ecs.Cluster" => read_cluster,
                "ecs.Service" => read_ecs_service,
                "ecs.TaskDefinition" => read_task_definition,
                "ecr.Repository" => read_repository,
                "rds.DBInstance" => read_db_instance,
                "rds.DBSubnetGroup" => read_db_subnet_group,
                "lambda.Function" => read_lambda_function,
                "route53.HostedZone" => read_hosted_zone,
                "route53.RecordSet" => read_record_set,
                "logs.LogGroup" => read_log_group,
                "sqs.Queue" => read_queue,
                "sns.Topic" => read_topic,
                "kms.Key" => read_kms_key,
                "dynamodb.Table" => read_dynamodb_table,
                "cloudfront.Distribution" => read_distribution,
                "acm.Certificate" => read_certificate,
                "secretsmanager.Secret" => read_secret,
                "ssm.Parameter" => read_parameter,
                "elasticache.ReplicationGroup" => read_replication_group,
                "efs.FileSystem" => read_file_system,
                "efs.MountTarget" => read_mount_target,
                "apigateway.Api" => read_api,
                "apigateway.Stage" => read_stage,
                "sfn.StateMachine" => read_state_machine,
                "eventbridge.Rule" => read_eventbridge_rule,
                "cloudwatch.Alarm" => read_alarm,
                "autoscaling.Group" => read_asg,
                "eks.Cluster" => read_eks_cluster,
                "eks.NodeGroup" => read_node_group,
                "wafv2.WebACL" => read_web_acl,
                "cognito.UserPool" => read_user_pool,
                "ses.EmailIdentity" => read_email_identity,
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
                "ec2.Vpc" => create_vpc,
                "ec2.Subnet" => create_subnet,
                "ec2.SecurityGroup" => create_security_group,
                "ec2.InternetGateway" => create_internet_gateway,
                "ec2.RouteTable" => create_route_table,
                "ec2.NatGateway" => create_nat_gateway,
                "ec2.ElasticIp" => create_elastic_ip,
                "ec2.KeyPair" => create_key_pair,
                "ec2.Instance" => create_instance,
                "iam.Role" => create_role,
                "iam.Policy" => create_policy,
                "iam.InstanceProfile" => create_instance_profile,
                "s3.Bucket" => create_bucket,
                "elbv2.LoadBalancer" => create_load_balancer,
                "elbv2.TargetGroup" => create_target_group,
                "elbv2.Listener" => create_listener,
                "ecs.Cluster" => create_cluster,
                "ecs.Service" => create_ecs_service,
                "ecs.TaskDefinition" => create_task_definition,
                "ecr.Repository" => create_repository,
                "rds.DBInstance" => create_db_instance,
                "rds.DBSubnetGroup" => create_db_subnet_group,
                "lambda.Function" => create_lambda_function,
                "route53.HostedZone" => create_hosted_zone,
                "route53.RecordSet" => create_record_set,
                "logs.LogGroup" => create_log_group,
                "sqs.Queue" => create_queue,
                "sns.Topic" => create_topic,
                "kms.Key" => create_kms_key,
                "dynamodb.Table" => create_dynamodb_table,
                "cloudfront.Distribution" => create_distribution,
                "acm.Certificate" => create_certificate,
                "secretsmanager.Secret" => create_secret,
                "ssm.Parameter" => create_parameter,
                "elasticache.ReplicationGroup" => create_replication_group,
                "efs.FileSystem" => create_file_system,
                "efs.MountTarget" => create_mount_target,
                "apigateway.Api" => create_api,
                "apigateway.Stage" => create_stage,
                "sfn.StateMachine" => create_state_machine,
                "eventbridge.Rule" => create_eventbridge_rule,
                "cloudwatch.Alarm" => create_alarm,
                "autoscaling.Group" => create_asg,
                "eks.Cluster" => create_eks_cluster,
                "eks.NodeGroup" => create_node_group,
                "wafv2.WebACL" => create_web_acl,
                "cognito.UserPool" => create_user_pool,
                "ses.EmailIdentity" => create_email_identity,
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
                    "ec2.Vpc" => update_vpc,
                    "ec2.Instance" => update_instance,
                    "ec2.RouteTable" => update_route_table,
                    "iam.Role" => update_role,
                    "iam.Policy" => update_policy,
                    "s3.Bucket" => update_bucket,
                    "elbv2.LoadBalancer" => update_load_balancer,
                    "elbv2.TargetGroup" => update_target_group,
                    "elbv2.Listener" => update_listener_resource,
                    "ecs.Service" => update_ecs_service_resource,
                    "lambda.Function" => update_lambda_function,
                    "route53.RecordSet" => update_record_set,
                    "logs.LogGroup" => update_log_group,
                    "sqs.Queue" => update_queue,
                    "sns.Topic" => update_topic,
                    "kms.Key" => update_kms_key,
                    "dynamodb.Table" => update_dynamodb_table,
                    "cloudfront.Distribution" => update_distribution,
                    "secretsmanager.Secret" => update_secret,
                    "ssm.Parameter" => update_parameter,
                    "elasticache.ReplicationGroup" => update_replication_group,
                    "efs.FileSystem" => update_file_system,
                    "apigateway.Api" => update_api,
                    "apigateway.Stage" => update_stage,
                    "sfn.StateMachine" => update_state_machine,
                    "eventbridge.Rule" => update_eventbridge_rule,
                    "cloudwatch.Alarm" => update_alarm,
                    "autoscaling.Group" => update_asg,
                    "eks.Cluster" => update_eks_cluster,
                    "eks.NodeGroup" => update_node_group,
                    "wafv2.WebACL" => update_web_acl,
                    "cognito.UserPool" => update_user_pool,
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
                    "efs.MountTarget",
                    "ses.EmailIdentity",
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
                "ec2.Vpc" => delete_vpc,
                "ec2.Subnet" => delete_subnet,
                "ec2.SecurityGroup" => delete_security_group,
                "ec2.InternetGateway" => delete_internet_gateway,
                "ec2.RouteTable" => delete_route_table,
                "ec2.NatGateway" => delete_nat_gateway,
                "ec2.ElasticIp" => delete_elastic_ip,
                "ec2.KeyPair" => delete_key_pair,
                "ec2.Instance" => delete_instance,
                "iam.Role" => delete_role,
                "iam.Policy" => delete_policy,
                "iam.InstanceProfile" => delete_instance_profile,
                "s3.Bucket" => delete_bucket,
                "elbv2.LoadBalancer" => delete_load_balancer,
                "elbv2.TargetGroup" => delete_target_group,
                "elbv2.Listener" => delete_listener,
                "ecs.Cluster" => delete_cluster,
                "ecs.Service" => delete_ecs_service,
                "ecs.TaskDefinition" => delete_task_definition,
                "ecr.Repository" => delete_repository,
                "rds.DBInstance" => delete_db_instance,
                "rds.DBSubnetGroup" => delete_db_subnet_group,
                "lambda.Function" => delete_lambda_function,
                "route53.HostedZone" => delete_hosted_zone,
                "route53.RecordSet" => delete_record_set,
                "logs.LogGroup" => delete_log_group,
                "sqs.Queue" => delete_queue,
                "sns.Topic" => delete_topic,
                "kms.Key" => delete_kms_key,
                "dynamodb.Table" => delete_dynamodb_table,
                "cloudfront.Distribution" => delete_distribution,
                "acm.Certificate" => delete_certificate,
                "secretsmanager.Secret" => delete_secret,
                "ssm.Parameter" => delete_parameter,
                "elasticache.ReplicationGroup" => delete_replication_group,
                "efs.FileSystem" => delete_file_system,
                "efs.MountTarget" => delete_mount_target,
                "apigateway.Api" => delete_api,
                "apigateway.Stage" => delete_stage,
                "sfn.StateMachine" => delete_state_machine,
                "eventbridge.Rule" => delete_eventbridge_rule,
                "cloudwatch.Alarm" => delete_alarm,
                "autoscaling.Group" => delete_asg,
                "eks.Cluster" => delete_eks_cluster,
                "eks.NodeGroup" => delete_node_group,
                "wafv2.WebACL" => delete_web_acl,
                "cognito.UserPool" => delete_user_pool,
                "ses.EmailIdentity" => delete_email_identity,
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
        assert_eq!(types.len(), 48);

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
