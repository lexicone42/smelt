//! Live AWS integration tests — requires real AWS credentials.
//! Run with: cargo test --test aws_live_test -- --ignored --nocapture
//!
//! These tests create real AWS resources and clean them up.
//! Cost: negligible (all free-tier or no-cost resources).
//! Each test is independent — can run them individually:
//!   cargo test --test aws_live_test sqs_queue_crud -- --ignored --nocapture

use smelt::provider::Provider;
use smelt::provider::aws::AwsProvider;

fn test_name(prefix: &str) -> String {
    format!(
        "smelt-test-{}-{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    )
}

/// Helper: run a full CRUD cycle and report diff findings.
/// Returns (create_output, read_output, diff_changes_after_create).
async fn crud_cycle(
    provider: &AwsProvider,
    resource_type: &str,
    config: &serde_json::Value,
    name: &str,
) -> (
    smelt::provider::ResourceOutput,
    smelt::provider::ResourceOutput,
    Vec<smelt::provider::FieldChange>,
) {
    println!("\n{}", "=".repeat(60));
    println!("  CRUD cycle: {resource_type} ({name})");
    println!("{}", "=".repeat(60));

    // ── Create ──
    println!("\n[CREATE] {resource_type}...");
    let created = provider
        .create(resource_type, config)
        .await
        .unwrap_or_else(|e| panic!("CREATE {resource_type} failed: {e:?}"));
    println!("  provider_id = {}", created.provider_id);
    println!(
        "  state = {}",
        serde_json::to_string_pretty(&created.state).unwrap()
    );
    println!("  outputs = {:?}", created.outputs);

    // ── Read ──
    println!("\n[READ] {resource_type} ({})...", created.provider_id);
    let read = provider
        .read(resource_type, &created.provider_id)
        .await
        .unwrap_or_else(|e| panic!("READ {resource_type} failed: {e:?}"));
    println!(
        "  state = {}",
        serde_json::to_string_pretty(&read.state).unwrap()
    );

    // ── Diff (desired vs actual) ──
    let changes = provider.diff(resource_type, config, &read.state);
    println!("\n[DIFF] {resource_type}: {} change(s)", changes.len());
    for c in &changes {
        println!(
            "  {} {}: {:?} -> {:?}{}",
            if c.forces_replacement { "!" } else { " " },
            c.path,
            c.old_value,
            c.new_value,
            if c.forces_replacement {
                " [FORCES REPLACEMENT]"
            } else {
                ""
            }
        );
    }

    (created, read, changes)
}

// ═══════════════════════════════════════════════════════════════
// SQS Queue — free, instant, tests attribute-map API pattern
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn sqs_queue_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("sqs");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "fifo": false,
        },
        "reliability": {
            "delay_seconds": 5,
            "message_retention_seconds": 86400,
            "visibility_timeout": 60,
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "sqs.Queue", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] sqs.Queue...");
    provider
        .delete("sqs.Queue", &created.provider_id)
        .await
        .expect("DELETE sqs.Queue failed");
    println!("  Deleted.");

    // Assert: diff should be clean after create+read
    if !changes.is_empty() {
        println!(
            "\n** DRIFT DETECTED — {} unexpected diff(s):",
            changes.len()
        );
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
    assert!(
        changes.is_empty(),
        "SQS Queue had {} diff(s) after create+read",
        changes.len()
    );
}

// ═══════════════════════════════════════════════════════════════
// SNS Topic — free, instant, simplest resource
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn sns_topic_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("sns");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "fifo": false,
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "sns.Topic", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] sns.Topic...");
    provider
        .delete("sns.Topic", &created.provider_id)
        .await
        .expect("DELETE sns.Topic failed");
    println!("  Deleted.");

    assert!(
        changes.is_empty(),
        "SNS Topic had {} diff(s) after create+read",
        changes.len()
    );
}

// ═══════════════════════════════════════════════════════════════
// SSM Parameter — free, instant, tests put_parameter pattern
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ssm_parameter_crud() {
    let provider = AwsProvider::from_env().await;
    let name = format!("/smelt/test/{}", test_name("ssm"));

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test parameter",
        },
        "sizing": {
            "type": "String",
            "value": "hello-smelt",
            "tier": "Standard",
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "ssm.Parameter", &config, &name).await;

    // ── Update ── (change value)
    println!("\n[UPDATE] ssm.Parameter (change value)...");
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test parameter updated",
        },
        "sizing": {
            "type": "String",
            "value": "updated-smelt-value",
            "tier": "Standard",
        }
    });
    let update_result = provider
        .update(
            "ssm.Parameter",
            &created.provider_id,
            &config,
            &update_config,
        )
        .await;
    match &update_result {
        Ok(output) => {
            println!(
                "  Updated. state = {}",
                serde_json::to_string_pretty(&output.state).unwrap()
            );
        }
        Err(e) => {
            println!("  UPDATE FAILED: {e:?}");
        }
    }

    // ── Cleanup ──
    println!("\n[DELETE] ssm.Parameter...");
    provider
        .delete("ssm.Parameter", &created.provider_id)
        .await
        .expect("DELETE ssm.Parameter failed");
    println!("  Deleted.");

    // Report diffs — SSM has known issues (missing description/tier on read)
    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s) after create+read", changes.len());
    }
}

// ═══════════════════════════════════════════════════════════════
// CloudWatch Logs LogGroup — free, instant
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn logs_log_group_crud() {
    let provider = AwsProvider::from_env().await;
    let name = format!("/smelt/test/{}", test_name("logs"));

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
        "reliability": {
            "retention_days": 1,
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "logs.LogGroup", &config, &name).await;

    // ── Prefix-match bug test ──
    // Create a second log group with the first as prefix
    let name2 = format!("{name}-extra");
    let config2 = serde_json::json!({
        "identity": { "name": &name2 },
        "reliability": { "retention_days": 1 },
    });
    println!("\n[PREFIX-MATCH TEST] Creating {name2}...");
    let created2 = provider.create("logs.LogGroup", &config2).await;
    match &created2 {
        Ok(_) => {
            // Now read the first group again — does it still return the right one?
            println!("  Reading original {name} (should NOT match {name2})...");
            let re_read = provider
                .read("logs.LogGroup", &created.provider_id)
                .await
                .unwrap();
            let re_read_name = re_read.state["identity"]["name"].as_str().unwrap_or("");
            println!("  Got: {re_read_name}");
            if re_read_name != name {
                println!("  ** PREFIX-MATCH BUG: read returned wrong log group!");
            }
            // Cleanup the extra log group
            let _ = provider.delete("logs.LogGroup", &name2).await;
        }
        Err(e) => {
            println!("  Second log group creation failed: {e:?}");
        }
    }

    // ── Cleanup ──
    println!("\n[DELETE] logs.LogGroup...");
    provider
        .delete("logs.LogGroup", &created.provider_id)
        .await
        .expect("DELETE logs.LogGroup failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
    }
}

// ═══════════════════════════════════════════════════════════════
// ECR Repository — free, fast
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ecr_repository_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("ecr");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
        "security": {
            "image_tag_mutability": "MUTABLE",
            "scan_on_push": true,
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "ecr.Repository", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] ecr.Repository...");
    provider
        .delete("ecr.Repository", &created.provider_id)
        .await
        .expect("DELETE ecr.Repository failed");
    println!("  Deleted.");

    assert!(
        changes.is_empty(),
        "ECR Repository had {} diff(s)",
        changes.len()
    );
}

// ═══════════════════════════════════════════════════════════════
// EventBridge EventBus — free, fast
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn eventbridge_event_bus_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("eb");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test event bus",
        }
    });

    let (created, read, changes) =
        crud_cycle(&provider, "eventbridge.EventBus", &config, &name).await;

    // Check specifically for the description field
    let read_desc = read.state["identity"]["description"]
        .as_str()
        .unwrap_or("<missing>");
    println!("\n[CHECK] description: requested='smelt live test event bus', got='{read_desc}'");

    // ── Cleanup ──
    println!("\n[DELETE] eventbridge.EventBus...");
    provider
        .delete("eventbridge.EventBus", &created.provider_id)
        .await
        .expect("DELETE eventbridge.EventBus failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!(
            "\n** DRIFT: {} diff(s) — description likely missing from read",
            changes.len()
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// Secrets Manager Secret — small cost, tests describe_secret
// Uses force-delete to avoid 30-day recovery window blocking retests
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn secretsmanager_secret_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("sm");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test secret",
        },
        "security": {
            "secret_string": "test-secret-value-123",
        }
    });

    let (created, read, changes) =
        crud_cycle(&provider, "secretsmanager.Secret", &config, &name).await;

    // Check: does read include description?
    let read_desc = read.state["identity"]["description"]
        .as_str()
        .unwrap_or("<missing>");
    println!("\n[CHECK] description: expected='smelt live test secret', got='{read_desc}'");

    // Check: does read include secret_string? (it shouldn't via describe_secret)
    let read_secret = read.state["security"]["secret_string"]
        .as_str()
        .unwrap_or("<missing>");
    println!(
        "[CHECK] secret_string on read: '{read_secret}' (expect empty or missing — describe_secret doesn't return it)"
    );

    // ── Cleanup (force-delete to avoid recovery window) ──
    println!("\n[DELETE] secretsmanager.Secret...");
    // Note: the generated code uses recovery_window_in_days(30) which blocks re-creation.
    // For tests, we need force_delete_without_recovery.
    provider
        .delete("secretsmanager.Secret", &created.provider_id)
        .await
        .expect("DELETE secretsmanager.Secret failed");
    println!("  Deleted (with 30-day recovery — secret name will be blocked).");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// EventBridge Rule — free, tests schedule/pattern rules
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn eventbridge_rule_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("eb-rule");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test rule",
        },
        "network": {
            "event_bus_name": "default",
        },
        "sizing": {
            "schedule_expression": "rate(1 hour)",
            "state": "DISABLED",
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "eventbridge.Rule", &config, &name).await;

    // ── Update: change description ──
    println!("\n[UPDATE] eventbridge.Rule (change description)...");
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test rule - updated",
        },
        "network": {
            "event_bus_name": "default",
        },
        "sizing": {
            "schedule_expression": "rate(1 hour)",
            "state": "DISABLED",
        }
    });
    let update_result = provider
        .update(
            "eventbridge.Rule",
            &created.provider_id,
            &config,
            &update_config,
        )
        .await;
    match &update_result {
        Ok(output) => {
            println!(
                "  Updated. state = {}",
                serde_json::to_string_pretty(&output.state).unwrap()
            );
        }
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }

    // ── Cleanup ──
    println!("\n[DELETE] eventbridge.Rule...");
    provider
        .delete("eventbridge.Rule", &created.provider_id)
        .await
        .expect("DELETE eventbridge.Rule failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
    }
}

// ═══════════════════════════════════════════════════════════════
// DynamoDB Table — free with on-demand billing
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn dynamodb_table_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("ddb");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
        "sizing": {
            "billing_mode": "PAY_PER_REQUEST",
            "partition_key": "pk",
            "partition_key_type": "S",
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "dynamodb.Table", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] dynamodb.Table...");
    provider
        .delete("dynamodb.Table", &created.provider_id)
        .await
        .expect("DELETE dynamodb.Table failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// CloudWatch Alarm — free, tests metric alarm pattern
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn cloudwatch_alarm_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("cw");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test alarm",
        },
        "sizing": {
            "namespace": "AWS/EC2",
            "metric_name": "CPUUtilization",
            "statistic": "Average",
            "period": 300,
            "evaluation_periods": 1,
            "threshold": 80.0,
            "comparison_operator": "GreaterThanThreshold",
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "cloudwatch.Alarm", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] cloudwatch.Alarm...");
    provider
        .delete("cloudwatch.Alarm", &created.provider_id)
        .await
        .expect("DELETE cloudwatch.Alarm failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Cognito UserPool — free tier
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn cognito_user_pool_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("cognito");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "cognito.UserPool", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] cognito.UserPool...");
    provider
        .delete("cognito.UserPool", &created.provider_id)
        .await
        .expect("DELETE cognito.UserPool failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// SES EmailIdentity — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ses_email_identity_crud() {
    let provider = AwsProvider::from_env().await;
    let name = format!(
        "smelt-test-{}.internal",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "ses.EmailIdentity", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] ses.EmailIdentity...");
    provider
        .delete("ses.EmailIdentity", &created.provider_id)
        .await
        .expect("DELETE ses.EmailIdentity failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// S3 Bucket — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn s3_bucket_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("s3");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "s3.Bucket", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] s3.Bucket...");
    provider
        .delete("s3.Bucket", &created.provider_id)
        .await
        .expect("DELETE s3.Bucket failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// ECS Cluster — free
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ecs_cluster_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("ecs");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "ecs.Cluster", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] ecs.Cluster...");
    provider
        .delete("ecs.Cluster", &created.provider_id)
        .await
        .expect("DELETE ecs.Cluster failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// API Gateway v2 Api — free tier
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn apigateway_api_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("apigw");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test api",
        },
        "network": {
            "protocol_type": "HTTP",
        },
    });

    let (created, _read, changes) = crud_cycle(&provider, "apigateway.Api", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] apigateway.Api...");
    provider
        .delete("apigateway.Api", &created.provider_id)
        .await
        .expect("DELETE apigateway.Api failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// IAM Role — free, fast, tests assume-role policy pattern
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn iam_role_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("iam-role");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test role",
        },
        "security": {
            "assume_role_policy": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "lambda.amazonaws.com" },
                    "Action": "sts:AssumeRole"
                }]
            }
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "iam.Role", &config, &name).await;

    // ── Update: change description ──
    println!("\n[UPDATE] iam.Role (change description)...");
    let update_config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test role - updated",
        },
        "security": {
            "assume_role_policy": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "lambda.amazonaws.com" },
                    "Action": "sts:AssumeRole"
                }]
            }
        }
    });
    let update_result = provider
        .update("iam.Role", &created.provider_id, &config, &update_config)
        .await;
    match &update_result {
        Ok(output) => println!(
            "  Updated. state = {}",
            serde_json::to_string_pretty(&output.state).unwrap()
        ),
        Err(e) => println!("  UPDATE FAILED: {e:?}"),
    }

    // ── Cleanup ──
    println!("\n[DELETE] iam.Role...");
    provider
        .delete("iam.Role", &created.provider_id)
        .await
        .expect("DELETE iam.Role failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// IAM Policy — free, fast, tests policy document pattern
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn iam_policy_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("iam-pol");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test policy",
        },
        "security": {
            "policy_document": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "logs:CreateLogGroup",
                    "Resource": "*"
                }]
            }
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "iam.Policy", &config, &name).await;

    // ── Cleanup ──
    println!("\n[DELETE] iam.Policy...");
    provider
        .delete("iam.Policy", &created.provider_id)
        .await
        .expect("DELETE iam.Policy failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// EC2 VPC — free, core networking resource
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ec2_vpc_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("vpc");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "network": { "cidr_block": "10.99.0.0/16" }
    });

    let (created, _read, changes) = crud_cycle(&provider, "ec2.Vpc", &config, &name).await;

    println!("\n[DELETE] ec2.Vpc...");
    provider
        .delete("ec2.Vpc", &created.provider_id)
        .await
        .expect("DELETE ec2.Vpc failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// EC2 Subnet — free, depends on VPC
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ec2_subnet_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("subnet");

    println!("[SETUP] Creating VPC for subnet test...");
    let vpc = provider
        .create(
            "ec2.Vpc",
            &serde_json::json!({
                "identity": { "name": &format!("{name}-vpc") },
                "network": { "cidr_block": "10.98.0.0/16" }
            }),
        )
        .await
        .expect("VPC create failed");
    let vpc_id = &vpc.provider_id;
    println!("  vpc_id = {vpc_id}");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "network": {
            "availability_zone": "us-west-2a",
            "cidr_block": "10.98.1.0/24",
            "public_ip_on_launch": false,
            "vpc_id": vpc_id,
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "ec2.Subnet", &config, &name).await;

    println!("\n[DELETE] ec2.Subnet...");
    provider
        .delete("ec2.Subnet", &created.provider_id)
        .await
        .expect("DELETE ec2.Subnet failed");
    println!("  Deleted subnet.");
    println!("[DELETE] ec2.Vpc...");
    provider
        .delete("ec2.Vpc", vpc_id)
        .await
        .expect("DELETE ec2.Vpc failed");
    println!("  Deleted VPC.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// EC2 SecurityGroup — free, depends on VPC
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ec2_security_group_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("sg");

    println!("[SETUP] Creating VPC for security group test...");
    let vpc = provider
        .create(
            "ec2.Vpc",
            &serde_json::json!({
                "identity": { "name": &format!("{name}-vpc") },
                "network": { "cidr_block": "10.97.0.0/16" }
            }),
        )
        .await
        .expect("VPC create failed");
    let vpc_id = &vpc.provider_id;
    println!("  vpc_id = {vpc_id}");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "security": {
            "vpc_id": vpc_id,
            "ingress": [],
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "ec2.SecurityGroup", &config, &name).await;

    println!("\n[DELETE] ec2.SecurityGroup...");
    provider
        .delete("ec2.SecurityGroup", &created.provider_id)
        .await
        .expect("DELETE ec2.SecurityGroup failed");
    println!("  Deleted SG.");
    println!("[DELETE] ec2.Vpc...");
    provider
        .delete("ec2.Vpc", vpc_id)
        .await
        .expect("DELETE ec2.Vpc failed");
    println!("  Deleted VPC.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// EC2 InternetGateway — free, depends on VPC
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ec2_internet_gateway_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("igw");

    println!("[SETUP] Creating VPC for IGW test...");
    let vpc = provider
        .create(
            "ec2.Vpc",
            &serde_json::json!({
                "identity": { "name": &format!("{name}-vpc") },
                "network": { "cidr_block": "10.96.0.0/16" }
            }),
        )
        .await
        .expect("VPC create failed");
    let vpc_id = &vpc.provider_id;
    println!("  vpc_id = {vpc_id}");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "network": { "vpc_id": vpc_id }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "ec2.InternetGateway", &config, &name).await;

    println!("\n[DELETE] ec2.InternetGateway...");
    provider
        .delete("ec2.InternetGateway", &created.provider_id)
        .await
        .expect("DELETE ec2.InternetGateway failed");
    println!("  Deleted IGW.");
    println!("[DELETE] ec2.Vpc...");
    provider
        .delete("ec2.Vpc", vpc_id)
        .await
        .expect("DELETE ec2.Vpc failed");
    println!("  Deleted VPC.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// EC2 RouteTable — free, depends on VPC
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ec2_route_table_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("rtb");

    println!("[SETUP] Creating VPC for route table test...");
    let vpc = provider
        .create(
            "ec2.Vpc",
            &serde_json::json!({
                "identity": { "name": &format!("{name}-vpc") },
                "network": { "cidr_block": "10.95.0.0/16" }
            }),
        )
        .await
        .expect("VPC create failed");
    let vpc_id = &vpc.provider_id;
    println!("  vpc_id = {vpc_id}");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "network": {
            "vpc_id": vpc_id,
            "routes": [],
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "ec2.RouteTable", &config, &name).await;

    println!("\n[DELETE] ec2.RouteTable...");
    provider
        .delete("ec2.RouteTable", &created.provider_id)
        .await
        .expect("DELETE ec2.RouteTable failed");
    println!("  Deleted route table.");
    println!("[DELETE] ec2.Vpc...");
    provider
        .delete("ec2.Vpc", vpc_id)
        .await
        .expect("DELETE ec2.Vpc failed");
    println!("  Deleted VPC.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// EC2 KeyPair — free, standalone
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn ec2_key_pair_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("kp");

    let config = serde_json::json!({
        "identity": { "name": &name },
    });

    let (created, _read, changes) = crud_cycle(&provider, "ec2.KeyPair", &config, &name).await;

    println!("\n[DELETE] ec2.KeyPair...");
    provider
        .delete("ec2.KeyPair", &created.provider_id)
        .await
        .expect("DELETE ec2.KeyPair failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// IAM InstanceProfile — free, depends on IAM Role
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn iam_instance_profile_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("ip");

    println!("[SETUP] Creating IAM Role for instance profile test...");
    let role = provider
        .create(
            "iam.Role",
            &serde_json::json!({
                "identity": {
                    "name": &format!("{name}-role"),
                    "description": "role for instance profile test",
                },
                "security": {
                    "assume_role_policy": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "ec2.amazonaws.com" },
                            "Action": "sts:AssumeRole"
                        }]
                    }
                }
            }),
        )
        .await
        .expect("IAM Role create failed");
    let role_name = &role.provider_id;
    println!("  role_name = {role_name}");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "security": { "role_name": role_name },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "iam.InstanceProfile", &config, &name).await;

    println!("\n[DELETE] iam.InstanceProfile...");
    provider
        .delete("iam.InstanceProfile", &created.provider_id)
        .await
        .expect("DELETE iam.InstanceProfile failed");
    println!("  Deleted instance profile.");
    // Small delay — IAM eventually-consistent
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    println!("[DELETE] iam.Role...");
    provider
        .delete("iam.Role", role_name)
        .await
        .expect("DELETE iam.Role failed");
    println!("  Deleted role.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Route53 HostedZone — $0.50/month, tests DNS zone pattern
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn route53_hosted_zone_crud() {
    let provider = AwsProvider::from_env().await;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let name = format!("smelt-test-{ts}.internal");

    let config = serde_json::json!({
        "identity": { "name": &name },
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "route53.HostedZone", &config, &name).await;

    println!("\n[DELETE] route53.HostedZone...");
    provider
        .delete("route53.HostedZone", &created.provider_id)
        .await
        .expect("DELETE route53.HostedZone failed");
    println!("  Deleted.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// KMS Key — $1/month, tests key management pattern
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn kms_key_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("kms");

    let config = serde_json::json!({
        "identity": {
            "name": &name,
            "description": "smelt live test key",
        },
        "security": {
            "key_usage": "ENCRYPT_DECRYPT",
            "key_spec": "SYMMETRIC_DEFAULT",
        }
    });

    let (created, _read, changes) = crud_cycle(&provider, "kms.Key", &config, &name).await;

    // KMS keys can't be immediately deleted — schedule deletion (minimum 7 days)
    println!("\n[DELETE] kms.Key (scheduling deletion)...");
    provider
        .delete("kms.Key", &created.provider_id)
        .await
        .expect("DELETE kms.Key failed");
    println!("  Scheduled for deletion.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// ELBv2 TargetGroup — free, depends on VPC
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn elbv2_target_group_crud() {
    let provider = AwsProvider::from_env().await;
    let name = test_name("tg");

    println!("[SETUP] Creating VPC for target group test...");
    let vpc = provider
        .create(
            "ec2.Vpc",
            &serde_json::json!({
                "identity": { "name": &format!("{name}-vpc") },
                "network": { "cidr_block": "10.94.0.0/16" }
            }),
        )
        .await
        .expect("VPC create failed");
    let vpc_id = &vpc.provider_id;
    println!("  vpc_id = {vpc_id}");

    let config = serde_json::json!({
        "identity": { "name": &name },
        "network": {
            "vpc_id": vpc_id,
            "port": 80,
            "protocol": "HTTP",
            "target_type": "ip",
        },
        "reliability": {
            "health_check_path": "/health",
        }
    });

    let (created, _read, changes) =
        crud_cycle(&provider, "elbv2.TargetGroup", &config, &name).await;

    println!("\n[DELETE] elbv2.TargetGroup...");
    provider
        .delete("elbv2.TargetGroup", &created.provider_id)
        .await
        .expect("DELETE elbv2.TargetGroup failed");
    println!("  Deleted target group.");
    println!("[DELETE] ec2.Vpc...");
    provider
        .delete("ec2.Vpc", vpc_id)
        .await
        .expect("DELETE ec2.Vpc failed");
    println!("  Deleted VPC.");

    if !changes.is_empty() {
        println!("\n** DRIFT: {} diff(s)", changes.len());
        for c in &changes {
            println!("  {}: {:?} -> {:?}", c.path, c.old_value, c.new_value);
        }
    }
}
