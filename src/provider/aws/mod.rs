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
    let mut tags = HashMap::new();
    if let Some(name) = config.pointer("/identity/name").and_then(|v| v.as_str()) {
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
            match resource_type.as_str() {
                // EC2
                "ec2.Vpc" => self.read_vpc(&provider_id).await,
                "ec2.Subnet" => self.read_subnet(&provider_id).await,
                "ec2.SecurityGroup" => self.read_security_group(&provider_id).await,
                "ec2.InternetGateway" => self.read_internet_gateway(&provider_id).await,
                "ec2.RouteTable" => self.read_route_table(&provider_id).await,
                "ec2.NatGateway" => self.read_nat_gateway(&provider_id).await,
                "ec2.ElasticIp" => self.read_elastic_ip(&provider_id).await,
                "ec2.KeyPair" => self.read_key_pair(&provider_id).await,
                "ec2.Instance" => self.read_instance(&provider_id).await,
                // IAM
                "iam.Role" => self.read_role(&provider_id).await,
                "iam.Policy" => self.read_policy(&provider_id).await,
                "iam.InstanceProfile" => self.read_instance_profile(&provider_id).await,
                // S3
                "s3.Bucket" => self.read_bucket(&provider_id).await,
                // ELBv2
                "elbv2.LoadBalancer" => self.read_load_balancer(&provider_id).await,
                "elbv2.TargetGroup" => self.read_target_group(&provider_id).await,
                "elbv2.Listener" => self.read_listener(&provider_id).await,
                // ECS
                "ecs.Cluster" => self.read_cluster(&provider_id).await,
                "ecs.Service" => self.read_ecs_service(&provider_id).await,
                "ecs.TaskDefinition" => self.read_task_definition(&provider_id).await,
                // ECR
                "ecr.Repository" => self.read_repository(&provider_id).await,
                // RDS
                "rds.DBInstance" => self.read_db_instance(&provider_id).await,
                "rds.DBSubnetGroup" => self.read_db_subnet_group(&provider_id).await,
                // Lambda
                "lambda.Function" => self.read_lambda_function(&provider_id).await,
                // Route53
                "route53.HostedZone" => self.read_hosted_zone(&provider_id).await,
                "route53.RecordSet" => self.read_record_set(&provider_id).await,
                // CloudWatch Logs
                "logs.LogGroup" => self.read_log_group(&provider_id).await,
                // SQS
                "sqs.Queue" => self.read_queue(&provider_id).await,
                // SNS
                "sns.Topic" => self.read_topic(&provider_id).await,
                // KMS
                "kms.Key" => self.read_kms_key(&provider_id).await,
                // DynamoDB
                "dynamodb.Table" => self.read_dynamodb_table(&provider_id).await,
                // CloudFront
                "cloudfront.Distribution" => self.read_distribution(&provider_id).await,
                // ACM
                "acm.Certificate" => self.read_certificate(&provider_id).await,
                // Secrets Manager
                "secretsmanager.Secret" => self.read_secret(&provider_id).await,
                // SSM
                "ssm.Parameter" => self.read_parameter(&provider_id).await,
                // ElastiCache
                "elasticache.ReplicationGroup" => self.read_replication_group(&provider_id).await,
                // EFS
                "efs.FileSystem" => self.read_file_system(&provider_id).await,
                "efs.MountTarget" => self.read_mount_target(&provider_id).await,
                // API Gateway
                "apigateway.Api" => self.read_api(&provider_id).await,
                "apigateway.Stage" => self.read_stage(&provider_id).await,
                // Step Functions
                "sfn.StateMachine" => self.read_state_machine(&provider_id).await,
                // EventBridge
                "eventbridge.Rule" => self.read_eventbridge_rule(&provider_id).await,
                // CloudWatch
                "cloudwatch.Alarm" => self.read_alarm(&provider_id).await,
                // Auto Scaling
                "autoscaling.Group" => self.read_asg(&provider_id).await,
                // EKS
                "eks.Cluster" => self.read_eks_cluster(&provider_id).await,
                "eks.NodeGroup" => self.read_node_group(&provider_id).await,
                // WAFv2
                "wafv2.WebACL" => self.read_web_acl(&provider_id).await,
                // Cognito
                "cognito.UserPool" => self.read_user_pool(&provider_id).await,
                // SES
                "ses.EmailIdentity" => self.read_email_identity(&provider_id).await,
                _ => Err(ProviderError::ApiError(format!(
                    "unsupported resource type: {resource_type}"
                ))),
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
            match resource_type.as_str() {
                // EC2
                "ec2.Vpc" => self.create_vpc(&config).await,
                "ec2.Subnet" => self.create_subnet(&config).await,
                "ec2.SecurityGroup" => self.create_security_group(&config).await,
                "ec2.InternetGateway" => self.create_internet_gateway(&config).await,
                "ec2.RouteTable" => self.create_route_table(&config).await,
                "ec2.NatGateway" => self.create_nat_gateway(&config).await,
                "ec2.ElasticIp" => self.create_elastic_ip(&config).await,
                "ec2.KeyPair" => self.create_key_pair(&config).await,
                "ec2.Instance" => self.create_instance(&config).await,
                // IAM
                "iam.Role" => self.create_role(&config).await,
                "iam.Policy" => self.create_policy(&config).await,
                "iam.InstanceProfile" => self.create_instance_profile(&config).await,
                // S3
                "s3.Bucket" => self.create_bucket(&config).await,
                // ELBv2
                "elbv2.LoadBalancer" => self.create_load_balancer(&config).await,
                "elbv2.TargetGroup" => self.create_target_group(&config).await,
                "elbv2.Listener" => self.create_listener(&config).await,
                // ECS
                "ecs.Cluster" => self.create_cluster(&config).await,
                "ecs.Service" => self.create_ecs_service(&config).await,
                "ecs.TaskDefinition" => self.create_task_definition(&config).await,
                // ECR
                "ecr.Repository" => self.create_repository(&config).await,
                // RDS
                "rds.DBInstance" => self.create_db_instance(&config).await,
                "rds.DBSubnetGroup" => self.create_db_subnet_group(&config).await,
                // Lambda
                "lambda.Function" => self.create_lambda_function(&config).await,
                // Route53
                "route53.HostedZone" => self.create_hosted_zone(&config).await,
                "route53.RecordSet" => self.create_record_set(&config).await,
                // CloudWatch Logs
                "logs.LogGroup" => self.create_log_group(&config).await,
                // SQS
                "sqs.Queue" => self.create_queue(&config).await,
                // SNS
                "sns.Topic" => self.create_topic(&config).await,
                // KMS
                "kms.Key" => self.create_kms_key(&config).await,
                // Extended services
                "dynamodb.Table" => self.create_dynamodb_table(&config).await,
                "cloudfront.Distribution" => self.create_distribution(&config).await,
                "acm.Certificate" => self.create_certificate(&config).await,
                "secretsmanager.Secret" => self.create_secret(&config).await,
                "ssm.Parameter" => self.create_parameter(&config).await,
                "elasticache.ReplicationGroup" => self.create_replication_group(&config).await,
                "efs.FileSystem" => self.create_file_system(&config).await,
                "efs.MountTarget" => self.create_mount_target(&config).await,
                "apigateway.Api" => self.create_api(&config).await,
                "apigateway.Stage" => self.create_stage(&config).await,
                "sfn.StateMachine" => self.create_state_machine(&config).await,
                "eventbridge.Rule" => self.create_eventbridge_rule(&config).await,
                "cloudwatch.Alarm" => self.create_alarm(&config).await,
                "autoscaling.Group" => self.create_asg(&config).await,
                "eks.Cluster" => self.create_eks_cluster(&config).await,
                "eks.NodeGroup" => self.create_node_group(&config).await,
                "wafv2.WebACL" => self.create_web_acl(&config).await,
                "cognito.UserPool" => self.create_user_pool(&config).await,
                "ses.EmailIdentity" => self.create_email_identity(&config).await,
                _ => Err(ProviderError::ApiError(format!(
                    "unsupported resource type: {resource_type}"
                ))),
            }
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
            match resource_type.as_str() {
                // In-place updatable
                "ec2.Vpc" => self.update_vpc(&provider_id, &new_config).await,
                "ec2.Instance" => self.update_instance(&provider_id, &new_config).await,
                "ec2.RouteTable" => self.update_route_table(&provider_id, &new_config).await,
                "iam.Role" => self.update_role(&provider_id, &new_config).await,
                "iam.Policy" => self.update_policy(&provider_id, &new_config).await,
                "s3.Bucket" => self.update_bucket(&provider_id, &new_config).await,
                "elbv2.LoadBalancer" => self.update_load_balancer(&provider_id, &new_config).await,
                "elbv2.TargetGroup" => self.update_target_group(&provider_id, &new_config).await,
                "elbv2.Listener" => {
                    self.update_listener_resource(&provider_id, &new_config)
                        .await
                }
                "ecs.Service" => {
                    self.update_ecs_service_resource(&provider_id, &new_config)
                        .await
                }
                "lambda.Function" => self.update_lambda_function(&provider_id, &new_config).await,
                "route53.RecordSet" => self.update_record_set(&provider_id, &new_config).await,
                "logs.LogGroup" => self.update_log_group(&provider_id, &new_config).await,
                "sqs.Queue" => self.update_queue(&provider_id, &new_config).await,
                "sns.Topic" => self.update_topic(&provider_id, &new_config).await,
                "kms.Key" => self.update_kms_key(&provider_id, &new_config).await,
                // Extended services — in-place updatable
                "dynamodb.Table" => self.update_dynamodb_table(&provider_id, &new_config).await,
                "cloudfront.Distribution" => {
                    self.update_distribution(&provider_id, &new_config).await
                }
                "secretsmanager.Secret" => self.update_secret(&provider_id, &new_config).await,
                "ssm.Parameter" => self.update_parameter(&provider_id, &new_config).await,
                "elasticache.ReplicationGroup" => {
                    self.update_replication_group(&provider_id, &new_config)
                        .await
                }
                "efs.FileSystem" => self.update_file_system(&provider_id, &new_config).await,
                "apigateway.Api" => self.update_api(&provider_id, &new_config).await,
                "apigateway.Stage" => self.update_stage(&provider_id, &new_config).await,
                "sfn.StateMachine" => self.update_state_machine(&provider_id, &new_config).await,
                "eventbridge.Rule" => {
                    self.update_eventbridge_rule(&provider_id, &new_config)
                        .await
                }
                "cloudwatch.Alarm" => self.update_alarm(&provider_id, &new_config).await,
                "autoscaling.Group" => self.update_asg(&provider_id, &new_config).await,
                "eks.Cluster" => self.update_eks_cluster(&provider_id, &new_config).await,
                "eks.NodeGroup" => self.update_node_group(&provider_id, &new_config).await,
                "wafv2.WebACL" => self.update_web_acl(&provider_id, &new_config).await,
                "cognito.UserPool" => self.update_user_pool(&provider_id, &new_config).await,
                // Requires replacement
                "ec2.Subnet"
                | "ec2.InternetGateway"
                | "ec2.NatGateway"
                | "ec2.ElasticIp"
                | "ec2.KeyPair"
                | "ec2.SecurityGroup"
                | "iam.InstanceProfile"
                | "ecs.Cluster"
                | "ecs.TaskDefinition"
                | "ecr.Repository"
                | "rds.DBInstance"
                | "rds.DBSubnetGroup"
                | "route53.HostedZone"
                | "acm.Certificate"
                | "efs.MountTarget"
                | "ses.EmailIdentity" => Err(ProviderError::RequiresReplacement(
                    "resource changes require replacement".into(),
                )),
                _ => Err(ProviderError::ApiError(format!(
                    "unsupported resource type: {resource_type}"
                ))),
            }
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
            match resource_type.as_str() {
                // EC2
                "ec2.Vpc" => self.delete_vpc(&provider_id).await,
                "ec2.Subnet" => self.delete_subnet(&provider_id).await,
                "ec2.SecurityGroup" => self.delete_security_group(&provider_id).await,
                "ec2.InternetGateway" => self.delete_internet_gateway(&provider_id).await,
                "ec2.RouteTable" => self.delete_route_table(&provider_id).await,
                "ec2.NatGateway" => self.delete_nat_gateway(&provider_id).await,
                "ec2.ElasticIp" => self.delete_elastic_ip(&provider_id).await,
                "ec2.KeyPair" => self.delete_key_pair(&provider_id).await,
                "ec2.Instance" => self.delete_instance(&provider_id).await,
                // IAM
                "iam.Role" => self.delete_role(&provider_id).await,
                "iam.Policy" => self.delete_policy(&provider_id).await,
                "iam.InstanceProfile" => self.delete_instance_profile(&provider_id).await,
                // S3
                "s3.Bucket" => self.delete_bucket(&provider_id).await,
                // ELBv2
                "elbv2.LoadBalancer" => self.delete_load_balancer(&provider_id).await,
                "elbv2.TargetGroup" => self.delete_target_group(&provider_id).await,
                "elbv2.Listener" => self.delete_listener(&provider_id).await,
                // ECS
                "ecs.Cluster" => self.delete_cluster(&provider_id).await,
                "ecs.Service" => self.delete_ecs_service(&provider_id).await,
                "ecs.TaskDefinition" => self.delete_task_definition(&provider_id).await,
                // ECR
                "ecr.Repository" => self.delete_repository(&provider_id).await,
                // RDS
                "rds.DBInstance" => self.delete_db_instance(&provider_id).await,
                "rds.DBSubnetGroup" => self.delete_db_subnet_group(&provider_id).await,
                // Lambda
                "lambda.Function" => self.delete_lambda_function(&provider_id).await,
                // Route53
                "route53.HostedZone" => self.delete_hosted_zone(&provider_id).await,
                "route53.RecordSet" => self.delete_record_set(&provider_id).await,
                // CloudWatch Logs
                "logs.LogGroup" => self.delete_log_group(&provider_id).await,
                // SQS
                "sqs.Queue" => self.delete_queue(&provider_id).await,
                // SNS
                "sns.Topic" => self.delete_topic(&provider_id).await,
                // KMS
                "kms.Key" => self.delete_kms_key(&provider_id).await,
                // Extended services
                "dynamodb.Table" => self.delete_dynamodb_table(&provider_id).await,
                "cloudfront.Distribution" => self.delete_distribution(&provider_id).await,
                "acm.Certificate" => self.delete_certificate(&provider_id).await,
                "secretsmanager.Secret" => self.delete_secret(&provider_id).await,
                "ssm.Parameter" => self.delete_parameter(&provider_id).await,
                "elasticache.ReplicationGroup" => self.delete_replication_group(&provider_id).await,
                "efs.FileSystem" => self.delete_file_system(&provider_id).await,
                "efs.MountTarget" => self.delete_mount_target(&provider_id).await,
                "apigateway.Api" => self.delete_api(&provider_id).await,
                "apigateway.Stage" => self.delete_stage(&provider_id).await,
                "sfn.StateMachine" => self.delete_state_machine(&provider_id).await,
                "eventbridge.Rule" => self.delete_eventbridge_rule(&provider_id).await,
                "cloudwatch.Alarm" => self.delete_alarm(&provider_id).await,
                "autoscaling.Group" => self.delete_asg(&provider_id).await,
                "eks.Cluster" => self.delete_eks_cluster(&provider_id).await,
                "eks.NodeGroup" => self.delete_node_group(&provider_id).await,
                "wafv2.WebACL" => self.delete_web_acl(&provider_id).await,
                "cognito.UserPool" => self.delete_user_pool(&provider_id).await,
                "ses.EmailIdentity" => self.delete_email_identity(&provider_id).await,
                _ => Err(ProviderError::ApiError(format!(
                    "unsupported resource type: {resource_type}"
                ))),
            }
        })
    }

    fn diff(
        &self,
        resource_type: &str,
        desired: &serde_json::Value,
        actual: &serde_json::Value,
    ) -> Vec<FieldChange> {
        let mut changes = Vec::new();
        diff_values("", desired, actual, &mut changes);

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

pub fn diff_values(
    path: &str,
    desired: &serde_json::Value,
    actual: &serde_json::Value,
    changes: &mut Vec<FieldChange>,
) {
    if desired == actual {
        return;
    }

    match (desired, actual) {
        (serde_json::Value::Object(d), serde_json::Value::Object(a)) => {
            for (k, dv) in d {
                let field_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match a.get(k) {
                    None => changes.push(FieldChange {
                        path: field_path,
                        change_type: ChangeType::Add,
                        old_value: None,
                        new_value: Some(dv.clone()),
                        forces_replacement: false,
                    }),
                    Some(av) => diff_values(&field_path, dv, av, changes),
                }
            }
            for (k, av) in a {
                if !d.contains_key(k) {
                    let field_path = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    changes.push(FieldChange {
                        path: field_path,
                        change_type: ChangeType::Remove,
                        old_value: Some(av.clone()),
                        new_value: None,
                        forces_replacement: false,
                    });
                }
            }
        }
        _ => {
            let p = if path.is_empty() { "<root>" } else { path };
            changes.push(FieldChange {
                path: p.to_string(),
                change_type: ChangeType::Modify,
                old_value: Some(actual.clone()),
                new_value: Some(desired.clone()),
                forces_replacement: false,
            });
        }
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
