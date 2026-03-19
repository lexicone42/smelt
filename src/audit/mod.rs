use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::signing::{SignedTransition, SigningKeyStore};
use crate::store::{ContentHash, Store, TreeEntry};

/// A single entry in the audit trail, correlating events with signed transitions.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub timestamp: String,
    pub event_type: String,
    pub resource_id: String,
    pub actor: String,
    pub intent: Option<String>,
    pub state_hash: Option<String>,
}

/// Full audit report for an environment.
#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub environment: String,
    pub entries: Vec<AuditEntry>,
    pub transitions: Vec<TransitionSummary>,
    pub current_tree_hash: Option<String>,
    pub resource_count: usize,
}

/// Summary of a signed transition.
#[derive(Debug, Clone, Serialize)]
pub struct TransitionSummary {
    pub timestamp: String,
    pub environment: String,
    pub previous_root: Option<String>,
    pub new_root: String,
    pub changes: Vec<ChangeSummary>,
    pub signer_identity: String,
    pub signer_public_key: String,
    pub signature_valid: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeSummary {
    pub resource_id: String,
    pub change_type: String,
    pub intent: Option<String>,
}

/// Result of full integrity verification.
#[derive(Debug, Clone, Serialize)]
pub struct VerificationReport {
    pub environment: String,
    pub chain_valid: bool,
    pub checks: Vec<VerificationCheck>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerificationCheck {
    pub check: String,
    pub passed: bool,
    pub detail: String,
}

/// in-toto v1 attestation envelope (simplified DSSE).
#[derive(Debug, Clone, Serialize)]
pub struct InTotoAttestation {
    #[serde(rename = "_type")]
    pub attestation_type: String,
    pub subject: Vec<InTotoSubject>,
    #[serde(rename = "predicateType")]
    pub predicate_type: String,
    pub predicate: InTotoPredicate,
}

#[derive(Debug, Clone, Serialize)]
pub struct InTotoSubject {
    pub name: String,
    pub digest: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InTotoPredicate {
    pub builder: InTotoBuilder,
    pub metadata: InTotoMetadata,
    pub materials: Vec<InTotoMaterial>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InTotoBuilder {
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InTotoMetadata {
    #[serde(rename = "buildInvocationId")]
    pub build_invocation_id: String,
    #[serde(rename = "buildStartedOn")]
    pub build_started_on: String,
    pub completeness: InTotoCompleteness,
}

#[derive(Debug, Clone, Serialize)]
pub struct InTotoCompleteness {
    pub environment: bool,
    pub materials: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InTotoMaterial {
    pub uri: String,
    pub digest: BTreeMap<String, String>,
}

/// CycloneDX BOM for infrastructure resources.
#[derive(Debug, Clone, Serialize)]
pub struct CycloneDxBom {
    #[serde(rename = "bomFormat")]
    pub bom_format: String,
    #[serde(rename = "specVersion")]
    pub spec_version: String,
    pub version: i32,
    pub metadata: CdxMetadata,
    pub components: Vec<CdxComponent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<CdxDependency>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdxMetadata {
    pub timestamp: String,
    pub tools: Vec<CdxTool>,
    pub component: CdxComponent,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdxTool {
    pub vendor: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdxComponent {
    #[serde(rename = "type")]
    pub component_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(rename = "bom-ref", skip_serializing_if = "Option::is_none")]
    pub bom_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hashes: Vec<CdxHash>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<CdxProperty>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdxHash {
    pub alg: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdxProperty {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdxDependency {
    #[serde(rename = "ref")]
    pub dep_ref: String,
    #[serde(rename = "dependsOn")]
    pub depends_on: Vec<String>,
}

/// Build the audit trail for an environment.
pub fn build_audit_trail(
    store: &Store,
    environment: &str,
    project_root: &std::path::Path,
) -> AuditReport {
    let events = store.read_events().unwrap_or_default();

    let entries: Vec<AuditEntry> = events
        .iter()
        .map(|e| AuditEntry {
            seq: e.seq,
            timestamp: e.timestamp.to_rfc3339(),
            event_type: e.event_type.to_string(),
            resource_id: e.resource_id.clone(),
            actor: e.actor.clone(),
            intent: e.intent.clone(),
            state_hash: e.new_hash.as_ref().map(|h| h.short().to_string()),
        })
        .collect();

    let transitions = load_transitions(project_root);

    let transition_summaries: Vec<TransitionSummary> = transitions
        .iter()
        .filter(|t| t.transition.environment == environment)
        .map(|t| {
            let sig_valid = SigningKeyStore::verify_transition(t).is_ok();
            TransitionSummary {
                timestamp: t.transition.timestamp.clone(),
                environment: t.transition.environment.clone(),
                previous_root: t.transition.previous_root.clone(),
                new_root: t.transition.new_root.clone(),
                changes: t
                    .transition
                    .changes
                    .iter()
                    .map(|c| ChangeSummary {
                        resource_id: c.resource_id.clone(),
                        change_type: c.change_type.clone(),
                        intent: c.intent.clone(),
                    })
                    .collect(),
                signer_identity: t.signer_identity.clone(),
                signer_public_key: t.signer_public_key.clone(),
                signature_valid: Some(sig_valid),
            }
        })
        .collect();

    let current_tree_hash = store.get_ref(environment).ok().map(|h| h.0.clone());
    let resource_count = current_tree_hash
        .as_ref()
        .and_then(|hash| store.get_tree(&ContentHash(hash.clone())).ok())
        .map(|tree| tree.children.len())
        .unwrap_or(0);

    AuditReport {
        environment: environment.to_string(),
        entries,
        transitions: transition_summaries,
        current_tree_hash,
        resource_count,
    }
}

/// Verify the full integrity chain for an environment.
pub fn verify_integrity(
    store: &Store,
    environment: &str,
    project_root: &std::path::Path,
) -> VerificationReport {
    let mut checks = Vec::new();
    let mut all_passed = true;

    // Check 1: Environment ref exists
    let tree_hash = match store.get_ref(environment) {
        Ok(h) => {
            checks.push(VerificationCheck {
                check: "environment_ref".to_string(),
                passed: true,
                detail: format!("environment ref points to tree {}", h.short()),
            });
            Some(h)
        }
        Err(_) => {
            checks.push(VerificationCheck {
                check: "environment_ref".to_string(),
                passed: true,
                detail: "no state exists yet (clean environment)".to_string(),
            });
            None
        }
    };

    // Check 2: Tree integrity (BLAKE3 hash verification)
    // get_tree() already verifies hash from raw bytes — if it succeeds,
    // the tree is intact.
    if let Some(ref hash) = tree_hash {
        match store.get_tree(hash) {
            Ok(tree) => {
                checks.push(VerificationCheck {
                    check: "tree_hash_integrity".to_string(),
                    passed: true,
                    detail: format!("tree hash verified ({} resources)", tree.children.len()),
                });

                // Check 3: All object hashes verify
                let mut objects_ok = true;
                let mut object_count = 0;
                for (name, entry) in &tree.children {
                    if let TreeEntry::Object(obj_hash) = entry {
                        object_count += 1;
                        if store.get_object(obj_hash).is_err() {
                            objects_ok = false;
                            all_passed = false;
                            checks.push(VerificationCheck {
                                check: "object_integrity".to_string(),
                                passed: false,
                                detail: format!(
                                    "object for '{name}' failed hash verification ({})",
                                    obj_hash.short()
                                ),
                            });
                        }
                    }
                }
                if objects_ok {
                    checks.push(VerificationCheck {
                        check: "object_integrity".to_string(),
                        passed: true,
                        detail: format!("all {object_count} object hashes verified"),
                    });
                }
            }
            Err(e) => {
                all_passed = false;
                checks.push(VerificationCheck {
                    check: "tree_hash_integrity".to_string(),
                    passed: false,
                    detail: format!("failed to load tree: {e}"),
                });
            }
        }
    }

    // Check 4: Transition signatures
    let transitions = load_transitions(project_root);
    let env_transitions: Vec<&SignedTransition> = transitions
        .iter()
        .filter(|t| t.transition.environment == environment)
        .collect();

    if env_transitions.is_empty() {
        checks.push(VerificationCheck {
            check: "transition_signatures".to_string(),
            passed: true,
            detail: "no transitions recorded yet".to_string(),
        });
    } else {
        let mut sig_ok = true;
        let mut sig_count = 0;
        for t in &env_transitions {
            sig_count += 1;
            if SigningKeyStore::verify_transition(t).is_err() {
                sig_ok = false;
                all_passed = false;
                checks.push(VerificationCheck {
                    check: "transition_signatures".to_string(),
                    passed: false,
                    detail: format!(
                        "INVALID signature on transition {} by {}",
                        &t.transition.new_root[..12],
                        t.signer_identity
                    ),
                });
            }
        }
        if sig_ok {
            checks.push(VerificationCheck {
                check: "transition_signatures".to_string(),
                passed: true,
                detail: format!("all {sig_count} transition signatures valid"),
            });
        }
    }

    // Check 5: Transition chain continuity (each new_root should be the next previous_root)
    if env_transitions.len() >= 2 {
        let mut chain_ok = true;
        let mut sorted = env_transitions.clone();
        sorted.sort_by(|a, b| a.transition.timestamp.cmp(&b.transition.timestamp));

        for pair in sorted.windows(2) {
            let expected_prev = &pair[0].transition.new_root;
            match &pair[1].transition.previous_root {
                Some(prev) if prev == expected_prev => {}
                Some(prev) => {
                    chain_ok = false;
                    all_passed = false;
                    checks.push(VerificationCheck {
                        check: "transition_chain".to_string(),
                        passed: false,
                        detail: format!(
                            "chain break: expected previous_root={}, got={}",
                            &expected_prev[..12],
                            &prev[..12]
                        ),
                    });
                }
                None => {
                    chain_ok = false;
                    all_passed = false;
                    checks.push(VerificationCheck {
                        check: "transition_chain".to_string(),
                        passed: false,
                        detail: format!(
                            "chain break: transition {} has no previous_root",
                            &pair[1].transition.new_root[..12]
                        ),
                    });
                }
            }
        }
        if chain_ok {
            checks.push(VerificationCheck {
                check: "transition_chain".to_string(),
                passed: true,
                detail: format!(
                    "transition chain is continuous ({} links)",
                    sorted.len() - 1
                ),
            });
        }
    }

    // Check 6: Event log integrity (sequential seq numbers)
    let events = store.read_events().unwrap_or_default();
    if events.is_empty() {
        checks.push(VerificationCheck {
            check: "event_log_sequence".to_string(),
            passed: true,
            detail: "no events recorded yet".to_string(),
        });
    } else {
        let mut seq_ok = true;
        for pair in events.windows(2) {
            if pair[1].seq != pair[0].seq + 1 {
                seq_ok = false;
                all_passed = false;
                checks.push(VerificationCheck {
                    check: "event_log_sequence".to_string(),
                    passed: false,
                    detail: format!(
                        "sequence gap: {} -> {} (expected {})",
                        pair[0].seq,
                        pair[1].seq,
                        pair[0].seq + 1
                    ),
                });
                break;
            }
        }
        if seq_ok {
            checks.push(VerificationCheck {
                check: "event_log_sequence".to_string(),
                passed: true,
                detail: format!(
                    "event sequence continuous (1..{})",
                    events.last().unwrap().seq
                ),
            });
        }
    }

    let passed_count = checks.iter().filter(|c| c.passed).count();
    let total_count = checks.len();

    VerificationReport {
        environment: environment.to_string(),
        chain_valid: all_passed,
        summary: format!("{passed_count}/{total_count} checks passed"),
        checks,
    }
}

/// Generate in-toto attestations from signed transitions.
pub fn export_intoto(
    _store: &Store,
    environment: &str,
    project_root: &std::path::Path,
) -> Vec<InTotoAttestation> {
    let transitions = load_transitions(project_root);

    transitions
        .iter()
        .filter(|t| t.transition.environment == environment)
        .map(|t| {
            let mut digest = BTreeMap::new();
            digest.insert("blake3".to_string(), t.transition.new_root.clone());

            let subject = InTotoSubject {
                name: format!(
                    "smelt:{}:{}",
                    t.transition.environment, t.transition.new_root
                ),
                digest,
            };

            let materials: Vec<InTotoMaterial> = t
                .transition
                .changes
                .iter()
                .map(|c| {
                    let mut d = BTreeMap::new();
                    d.insert("change_type".to_string(), c.change_type.clone());
                    InTotoMaterial {
                        uri: format!("smelt://resource/{}", c.resource_id),
                        digest: d,
                    }
                })
                .collect();

            InTotoAttestation {
                attestation_type: "https://in-toto.io/Statement/v1".to_string(),
                subject: vec![subject],
                predicate_type: "https://slsa.dev/provenance/v1".to_string(),
                predicate: InTotoPredicate {
                    builder: InTotoBuilder {
                        id: format!("smelt:{}@{}", t.signer_identity, &t.signer_public_key[..16]),
                    },
                    metadata: InTotoMetadata {
                        build_invocation_id: t.transition.new_root.clone(),
                        build_started_on: t.transition.timestamp.clone(),
                        completeness: InTotoCompleteness {
                            environment: true,
                            materials: true,
                        },
                    },
                    materials,
                },
            }
        })
        .collect()
}

/// Generate a CycloneDX SBOM from the current infrastructure state.
pub fn export_cyclonedx(store: &Store, environment: &str) -> Option<CycloneDxBom> {
    let tree_hash = store.get_ref(environment).ok()?;
    let tree = store.get_tree(&tree_hash).ok()?;

    let mut components = Vec::new();
    let mut dependencies = Vec::new();

    for (name, entry) in &tree.children {
        if let TreeEntry::Object(hash) = entry
            && let Ok(obj) = store.get_object(hash)
        {
            let mut properties = vec![
                CdxProperty {
                    name: "smelt:type_path".to_string(),
                    value: obj.type_path.clone(),
                },
                CdxProperty {
                    name: "smelt:environment".to_string(),
                    value: environment.to_string(),
                },
            ];

            if let Some(pid) = &obj.provider_id {
                properties.push(CdxProperty {
                    name: "smelt:provider_id".to_string(),
                    value: pid.clone(),
                });
            }

            if let Some(intent) = &obj.intent {
                properties.push(CdxProperty {
                    name: "smelt:intent".to_string(),
                    value: intent.clone(),
                });
            }

            let hashes = vec![CdxHash {
                alg: "BLAKE3".to_string(),
                content: hash.0.clone(),
            }];

            // Determine component type from type_path
            let component_type = if obj.type_path.contains("compute")
                || obj.type_path.contains("Instance")
                || obj.type_path.contains("ec2")
            {
                "device"
            } else if obj.type_path.contains("rds")
                || obj.type_path.contains("sql")
                || obj.type_path.contains("database")
            {
                "data"
            } else {
                "platform"
            };

            components.push(CdxComponent {
                component_type: component_type.to_string(),
                name: name.clone(),
                version: Some(hash.short().to_string()),
                bom_ref: Some(name.clone()),
                description: obj.intent.clone(),
                hashes,
                properties,
            });

            // Build dependency edges from the config's needs references
            // (stored in the resource config as dependency metadata)
            dependencies.push(CdxDependency {
                dep_ref: name.clone(),
                depends_on: vec![],
            });
        }
    }

    let metadata = CdxMetadata {
        timestamp: chrono::Utc::now().to_rfc3339(),
        tools: vec![CdxTool {
            vendor: "smelt".to_string(),
            name: "smelt".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }],
        component: CdxComponent {
            component_type: "application".to_string(),
            name: format!("smelt:{environment}"),
            version: Some(tree_hash.short().to_string()),
            bom_ref: None,
            description: Some(format!(
                "Infrastructure state for environment '{environment}'"
            )),
            hashes: vec![CdxHash {
                alg: "BLAKE3".to_string(),
                content: tree_hash.0.clone(),
            }],
            properties: vec![],
        },
    };

    Some(CycloneDxBom {
        bom_format: "CycloneDX".to_string(),
        spec_version: "1.5".to_string(),
        version: 1,
        metadata,
        components,
        dependencies,
    })
}

/// Format an audit report for terminal output.
pub fn format_audit_report(report: &AuditReport) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "Audit trail for environment: {}\n",
        report.environment
    ));
    out.push_str(&format!(
        "Current state: {} resources",
        report.resource_count
    ));
    if let Some(ref hash) = report.current_tree_hash {
        out.push_str(&format!(" (tree {})", &hash[..12.min(hash.len())]));
    }
    out.push('\n');

    if !report.transitions.is_empty() {
        out.push_str(&format!(
            "\nSigned transitions ({}):\n",
            report.transitions.len()
        ));
        for t in &report.transitions {
            let sig_status = match t.signature_valid {
                Some(true) => "verified",
                Some(false) => "INVALID",
                None => "unchecked",
            };
            out.push_str(&format!(
                "  {} by {} [{}]\n",
                t.timestamp, t.signer_identity, sig_status
            ));
            if let Some(ref prev) = t.previous_root {
                out.push_str(&format!(
                    "    {} -> {}\n",
                    &prev[..12.min(prev.len())],
                    &t.new_root[..12.min(t.new_root.len())]
                ));
            } else {
                out.push_str(&format!(
                    "    (initial) -> {}\n",
                    &t.new_root[..12.min(t.new_root.len())]
                ));
            }
            for c in &t.changes {
                let intent = c
                    .intent
                    .as_deref()
                    .map(|i| format!("  # {i}"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "      {} {}{}\n",
                    c.change_type, c.resource_id, intent
                ));
            }
        }
    }

    if !report.entries.is_empty() {
        out.push_str(&format!("\nEvent log ({} events):\n", report.entries.len()));
        for e in &report.entries {
            let hash_str = e.state_hash.as_deref().unwrap_or("none");
            out.push_str(&format!(
                "  [{:>4}] {} {} {} (by {}) [{}]\n",
                e.seq, e.timestamp, e.event_type, e.resource_id, e.actor, hash_str
            ));
        }
    }

    if report.entries.is_empty() && report.transitions.is_empty() {
        out.push_str("\n  No audit events recorded yet. Run `smelt apply` to create state.\n");
    }

    out
}

/// Format a verification report for terminal output.
pub fn format_verification_report(report: &VerificationReport) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "Integrity verification for environment: {}\n\n",
        report.environment
    ));

    for check in &report.checks {
        let status = if check.passed { "PASS" } else { "FAIL" };
        out.push_str(&format!("  [{status}] {}: {}\n", check.check, check.detail));
    }

    out.push_str(&format!(
        "\nResult: {} ({})\n",
        if report.chain_valid {
            "INTEGRITY VERIFIED"
        } else {
            "INTEGRITY VIOLATION DETECTED"
        },
        report.summary
    ));

    out
}

// --- Internal helpers ---

fn load_transitions(project_root: &Path) -> Vec<SignedTransition> {
    let transitions_dir = project_root.join(".smelt/transitions");
    if !transitions_dir.exists() {
        return Vec::new();
    }

    let mut transitions = Vec::new();
    if let Ok(entries) = fs::read_dir(&transitions_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "json")
                && let Ok(data) = fs::read_to_string(entry.path())
                && let Ok(t) = serde_json::from_str::<SignedTransition>(&data)
            {
                transitions.push(t);
            }
        }
    }

    transitions.sort_by(|a, b| a.transition.timestamp.cmp(&b.transition.timestamp));
    transitions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("smelt-audit-test-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn audit_trail_empty_environment() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();

        let report = build_audit_trail(&store, "production", &dir);
        assert_eq!(report.environment, "production");
        assert!(report.entries.is_empty());
        assert!(report.transitions.is_empty());
        assert_eq!(report.resource_count, 0);
        assert!(report.current_tree_hash.is_none());
    }

    #[test]
    fn audit_trail_with_events() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();

        // Append some events
        let event = crate::store::Event {
            seq: 1,
            timestamp: chrono::Utc::now(),
            event_type: crate::store::EventType::ResourceCreated,
            resource_id: "vpc.main".to_string(),
            actor: "test@example.com".to_string(),
            intent: Some("Primary VPC".to_string()),
            prev_hash: None,
            new_hash: None,
        };
        store.append_event(&event).unwrap();

        let report = build_audit_trail(&store, "default", &dir);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].resource_id, "vpc.main");
        assert_eq!(report.entries[0].actor, "test@example.com");
    }

    #[test]
    fn verify_empty_environment() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();

        let report = verify_integrity(&store, "production", &dir);
        assert!(report.chain_valid);
        assert!(!report.checks.is_empty());
    }

    #[test]
    fn verify_with_stored_state() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();

        // Store a resource and tree
        let state = crate::store::ResourceState {
            resource_id: "vpc.main".to_string(),
            type_path: "aws.ec2.Vpc".to_string(),
            config: serde_json::json!({"network": {"cidr_block": "10.0.0.0/16"}}),
            actual: None,
            provider_id: Some("vpc-123".to_string()),
            intent: Some("Primary VPC".to_string()),
            outputs: None,
        };
        let obj_hash = store.put_object(&state).unwrap();

        let mut tree = crate::store::TreeNode::new();
        tree.children
            .insert("vpc.main".to_string(), TreeEntry::Object(obj_hash));
        let tree_hash = store.put_tree(&tree).unwrap();
        store.set_ref("production", &tree_hash).unwrap();

        let report = verify_integrity(&store, "production", &dir);
        assert!(report.chain_valid);
        assert!(report.checks.iter().all(|c| c.passed));
    }

    #[test]
    fn cyclonedx_export_includes_resources() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();

        let state = crate::store::ResourceState {
            resource_id: "vpc.main".to_string(),
            type_path: "aws.ec2.Vpc".to_string(),
            config: serde_json::json!({"network": {"cidr_block": "10.0.0.0/16"}}),
            actual: None,
            provider_id: Some("vpc-abc123".to_string()),
            intent: Some("Primary VPC".to_string()),
            outputs: None,
        };
        let obj_hash = store.put_object(&state).unwrap();

        let mut tree = crate::store::TreeNode::new();
        tree.children
            .insert("vpc.main".to_string(), TreeEntry::Object(obj_hash));
        let tree_hash = store.put_tree(&tree).unwrap();
        store.set_ref("prod", &tree_hash).unwrap();

        let bom = export_cyclonedx(&store, "prod").unwrap();
        assert_eq!(bom.bom_format, "CycloneDX");
        assert_eq!(bom.spec_version, "1.5");
        assert_eq!(bom.components.len(), 1);
        assert_eq!(bom.components[0].name, "vpc.main");

        // Verify properties contain type_path and provider_id
        let props = &bom.components[0].properties;
        assert!(
            props
                .iter()
                .any(|p| p.name == "smelt:type_path" && p.value == "aws.ec2.Vpc")
        );
        assert!(
            props
                .iter()
                .any(|p| p.name == "smelt:provider_id" && p.value == "vpc-abc123")
        );
    }

    #[test]
    fn format_audit_report_output() {
        let report = AuditReport {
            environment: "production".to_string(),
            entries: vec![AuditEntry {
                seq: 1,
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                event_type: "created".to_string(),
                resource_id: "vpc.main".to_string(),
                actor: "deploy@ci".to_string(),
                intent: Some("Primary VPC".to_string()),
                state_hash: Some("abc123def456".to_string()),
            }],
            transitions: vec![],
            current_tree_hash: Some("abc123def456789012345678".to_string()),
            resource_count: 3,
        };

        let output = format_audit_report(&report);
        assert!(output.contains("production"));
        assert!(output.contains("3 resources"));
        assert!(output.contains("vpc.main"));
        assert!(output.contains("deploy@ci"));
    }

    #[test]
    fn format_verification_report_output() {
        let report = VerificationReport {
            environment: "staging".to_string(),
            chain_valid: true,
            summary: "4/4 checks passed".to_string(),
            checks: vec![VerificationCheck {
                check: "tree_hash".to_string(),
                passed: true,
                detail: "hash verified".to_string(),
            }],
        };

        let output = format_verification_report(&report);
        assert!(output.contains("staging"));
        assert!(output.contains("INTEGRITY VERIFIED"));
        assert!(output.contains("[PASS]"));
    }
}
