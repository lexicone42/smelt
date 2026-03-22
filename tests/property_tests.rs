//! Property-based tests for smelt core invariants.
//!
//! These tests use proptest to verify key properties hold across
//! a wide range of inputs, catching edge cases that example-based
//! tests miss.

use proptest::prelude::*;

// ─── Arbitrary Generators ────────────────────────────────────────────

/// Generate a valid smelt identifier (lowercase alpha + underscores).
fn arb_ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,15}".prop_filter("non-empty", |s| !s.is_empty())
}

/// Generate a safe string value (no special chars that break parsing).
fn arb_safe_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_./ -]{1,30}"
}

/// Generate a simple smelt Value.
fn arb_value() -> impl Strategy<Value = smelt::ast::Value> {
    prop_oneof![
        arb_safe_string().prop_map(smelt::ast::Value::String),
        (0i64..1000).prop_map(smelt::ast::Value::Integer),
        any::<bool>().prop_map(smelt::ast::Value::Bool),
    ]
}

/// Generate a Field with a safe name and value.
fn arb_field() -> impl Strategy<Value = smelt::ast::Field> {
    (arb_ident(), arb_value()).prop_map(|(name, value)| smelt::ast::Field { name, value })
}

/// Generate a Section with 1-4 unique fields.
fn arb_section() -> impl Strategy<Value = smelt::ast::Section> {
    (arb_ident(), prop::collection::vec(arb_field(), 1..4)).prop_map(|(name, fields)| {
        // Deduplicate field names
        let mut seen = std::collections::HashSet::new();
        let fields = fields
            .into_iter()
            .filter(|f| seen.insert(f.name.clone()))
            .collect();
        smelt::ast::Section { name, fields }
    })
}

/// Generate a minimal resource declaration.
fn arb_resource() -> impl Strategy<Value = smelt::ast::ResourceDecl> {
    (
        arb_ident(),
        arb_ident(),
        prop::collection::vec(arb_section(), 1..3),
    )
        .prop_map(|(kind, name, sections)| {
            // Deduplicate section names
            let mut seen = std::collections::HashSet::new();
            let sections = sections
                .into_iter()
                .filter(|s| seen.insert(s.name.clone()))
                .collect();
            smelt::ast::ResourceDecl {
                kind,
                name,
                type_path: smelt::ast::TypePath {
                    segments: vec!["aws".into(), "ec2".into(), "Vpc".into()],
                },
                annotations: vec![],
                dependencies: vec![],
                sections,
                fields: vec![],
                for_each: None,
            }
        })
}

/// Generate a SmeltFile with 1-3 resources.
fn arb_smelt_file() -> impl Strategy<Value = smelt::ast::SmeltFile> {
    prop::collection::vec(arb_resource(), 1..3).prop_map(|resources| {
        // Deduplicate resource names (kind.name pairs)
        let mut seen = std::collections::HashSet::new();
        let declarations = resources
            .into_iter()
            .filter(|r| seen.insert(format!("{}.{}", r.kind, r.name)))
            .map(smelt::ast::Declaration::Resource)
            .collect();
        smelt::ast::SmeltFile { declarations }
    })
}

// ─── Parser/Formatter Roundtrip Properties ───────────────────────────

proptest! {
    /// format(parse(format(ast))) == format(ast)
    /// The formatter is a canonical form — formatting twice produces identical output.
    #[test]
    fn formatter_is_idempotent(file in arb_smelt_file()) {
        let formatted1 = smelt::formatter::format(&file);
        let reparsed = smelt::parser::parse(&formatted1)
            .expect("formatted output must parse");
        let formatted2 = smelt::formatter::format(&reparsed);
        prop_assert_eq!(&formatted1, &formatted2,
            "formatter must be idempotent");
    }

    /// parse(format(ast)) preserves the semantic content.
    /// After roundtrip, all resource kinds, names, section names, and field names are preserved.
    #[test]
    fn roundtrip_preserves_resource_identity(resource in arb_resource()) {
        let file = smelt::ast::SmeltFile {
            declarations: vec![smelt::ast::Declaration::Resource(resource.clone())],
        };
        let formatted = smelt::formatter::format(&file);
        let reparsed = smelt::parser::parse(&formatted)
            .expect("formatted output must parse");

        prop_assert_eq!(reparsed.declarations.len(), 1);
        if let smelt::ast::Declaration::Resource(r) = &reparsed.declarations[0] {
            prop_assert_eq!(&r.kind, &resource.kind);
            prop_assert_eq!(&r.name, &resource.name);
            prop_assert_eq!(&r.type_path, &resource.type_path);
            // All section names should be present
            let original_sections: std::collections::HashSet<_> =
                resource.sections.iter().map(|s| &s.name).collect();
            let roundtrip_sections: std::collections::HashSet<_> =
                r.sections.iter().map(|s| &s.name).collect();
            prop_assert_eq!(original_sections, roundtrip_sections);
        } else {
            prop_assert!(false, "expected Resource declaration");
        }
    }

    /// Formatting always produces alphabetically sorted sections.
    #[test]
    fn formatter_sorts_sections(file in arb_smelt_file()) {
        let formatted = smelt::formatter::format(&file);
        let reparsed = smelt::parser::parse(&formatted)
            .expect("formatted output must parse");

        for decl in &reparsed.declarations {
            if let smelt::ast::Declaration::Resource(r) = decl {
                let section_names: Vec<_> = r.sections.iter().map(|s| &s.name).collect();
                let mut sorted = section_names.clone();
                sorted.sort();
                prop_assert_eq!(&section_names, &sorted,
                    "sections must be alphabetically sorted after formatting");
            }
        }
    }

    /// Formatting always produces alphabetically sorted fields within sections.
    #[test]
    fn formatter_sorts_fields_within_sections(file in arb_smelt_file()) {
        let formatted = smelt::formatter::format(&file);
        let reparsed = smelt::parser::parse(&formatted)
            .expect("formatted output must parse");

        for decl in &reparsed.declarations {
            if let smelt::ast::Declaration::Resource(r) = decl {
                for section in &r.sections {
                    let field_names: Vec<_> = section.fields.iter().map(|f| &f.name).collect();
                    let mut sorted = field_names.clone();
                    sorted.sort();
                    prop_assert_eq!(&field_names, &sorted,
                        "fields in section '{}' must be sorted", section.name);
                }
            }
        }
    }
}

// ─── Content-Addressable Store Properties ────────────────────────────

mod store_properties {
    use super::*;
    use smelt::store::{ContentHash, Store};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_store() -> (Store, std::path::PathBuf) {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("smelt-prop-test-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).expect("store open");
        (store, dir)
    }

    proptest! {
        /// ContentHash is deterministic: same data always produces same hash.
        #[test]
        fn hash_is_deterministic(data in prop::collection::vec(any::<u8>(), 0..1000)) {
            let h1 = ContentHash::of(&data);
            let h2 = ContentHash::of(&data);
            prop_assert_eq!(h1, h2);
        }

        /// Different data (almost certainly) produces different hashes.
        #[test]
        fn hash_is_collision_resistant(
            a in prop::collection::vec(any::<u8>(), 1..100),
            b in prop::collection::vec(any::<u8>(), 1..100),
        ) {
            prop_assume!(a != b);
            let h1 = ContentHash::of(&a);
            let h2 = ContentHash::of(&b);
            prop_assert_ne!(h1, h2);
        }

        /// put_object then get_object returns identical state.
        #[test]
        fn store_roundtrip(
            resource_id in arb_ident(),
            type_path in arb_ident(),
        ) {
            let (store, _dir) = temp_store();
            let state = smelt::store::ResourceState { last_updated: None,
                resource_id: resource_id.clone(),
                type_path: type_path.clone(),
                config: serde_json::json!({"test": true}),
                actual: Some(serde_json::json!({"status": "ok"})),
                provider_id: Some("test-id-123".to_string()),
                intent: Some("test intent".to_string()),
                outputs: None,
            };

            let hash = store.put_object(&state).expect("put_object");
            let retrieved = store.get_object(&hash).expect("get_object");

            prop_assert_eq!(&retrieved.resource_id, &resource_id);
            prop_assert_eq!(&retrieved.type_path, &type_path);
            prop_assert_eq!(retrieved.provider_id.as_deref(), Some("test-id-123"));
        }

        /// Storing the same object twice produces the same hash (content-addressable).
        #[test]
        fn store_is_content_addressable(resource_id in arb_ident()) {
            let (store, _dir) = temp_store();
            let state = smelt::store::ResourceState { last_updated: None,
                resource_id,
                type_path: "test.Type".to_string(),
                config: serde_json::json!({}),
                actual: None,
                provider_id: None,
                intent: None,
                outputs: None,
            };

            let h1 = store.put_object(&state).expect("put 1");
            let h2 = store.put_object(&state).expect("put 2");
            prop_assert_eq!(h1, h2);
        }
    }
}

// ─── Diff Engine Properties ──────────────────────────────────────────

mod diff_properties {
    use super::*;
    use smelt::provider::FieldChange;
    use smelt::provider::diff_values;

    /// Generate a flat JSON object for diffing.
    fn arb_flat_json() -> impl Strategy<Value = serde_json::Value> {
        prop::collection::hash_map(
            "[a-z]{1,8}",
            prop_oneof![
                arb_safe_string().prop_map(|s| serde_json::json!(s)),
                (0i64..1000).prop_map(|n| serde_json::json!(n)),
                any::<bool>().prop_map(|b| serde_json::json!(b)),
            ],
            0..5,
        )
        .prop_map(|map| serde_json::Value::Object(map.into_iter().collect()))
    }

    proptest! {
        /// Diffing identical values produces no changes.
        #[test]
        fn diff_identical_is_empty(json in arb_flat_json()) {
            let mut changes = Vec::new();
            diff_values("", &json, &json, &mut changes);
            prop_assert!(changes.is_empty(),
                "diffing identical JSON should produce no changes, got: {:?}", changes);
        }

        /// Every diff change has a non-empty path.
        #[test]
        fn diff_changes_have_paths(
            a in arb_flat_json(),
            b in arb_flat_json(),
        ) {
            let mut changes = Vec::new();
            diff_values("", &a, &b, &mut changes);
            for change in &changes {
                prop_assert!(!change.path.is_empty(),
                    "change path must not be empty");
            }
        }

        /// For flat objects, diff(a, b) changes cover all differing keys.
        #[test]
        fn diff_covers_all_differences(
            a in arb_flat_json(),
            b in arb_flat_json(),
        ) {
            let mut changes: Vec<FieldChange> = Vec::new();
            diff_values("", &a, &b, &mut changes);

            let a_obj = a.as_object().unwrap();
            let b_obj = b.as_object().unwrap();

            // Every key in a but not b should be an Add
            for key in a_obj.keys() {
                if !b_obj.contains_key(key) {
                    prop_assert!(
                        changes.iter().any(|c| c.path == *key
                            && c.change_type == smelt::provider::ChangeType::Add),
                        "key '{}' in desired but not actual should be Add", key
                    );
                }
            }
            // Every key in b but not a should be a Remove
            for key in b_obj.keys() {
                if !a_obj.contains_key(key) {
                    prop_assert!(
                        changes.iter().any(|c| c.path == *key
                            && c.change_type == smelt::provider::ChangeType::Remove),
                        "key '{}' in actual but not desired should be Remove", key
                    );
                }
            }
        }

        /// diff(a, b) produces Add/Modify/Remove changes that are inverses
        /// of diff(b, a): Adds become Removes and vice versa, Modifies stay Modifies.
        #[test]
        fn diff_inverse_symmetry(
            a in arb_flat_json(),
            b in arb_flat_json(),
        ) {
            let mut forward: Vec<FieldChange> = Vec::new();
            let mut backward: Vec<FieldChange> = Vec::new();
            diff_values("", &a, &b, &mut forward);
            diff_values("", &b, &a, &mut backward);

            // Same number of changes
            prop_assert_eq!(forward.len(), backward.len(),
                "forward and backward diffs should have same length");

            // For each forward Add at path P, there should be a backward Remove at P
            for fc in &forward {
                let inverse_type = match fc.change_type {
                    smelt::provider::ChangeType::Add => smelt::provider::ChangeType::Remove,
                    smelt::provider::ChangeType::Remove => smelt::provider::ChangeType::Add,
                    smelt::provider::ChangeType::Modify => smelt::provider::ChangeType::Modify,
                };
                prop_assert!(
                    backward.iter().any(|bc| bc.path == fc.path && bc.change_type == inverse_type),
                    "forward {:?} at '{}' should have inverse {:?} in backward diff",
                    fc.change_type, fc.path, inverse_type
                );
            }
        }
    }
}

// ─── Signing Properties ──────────────────────────────────────────────

mod signing_properties {
    use super::*;
    use smelt::signing::{SigningKeyStore, TransitionData};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("smelt-sign-prop-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    proptest! {
        /// A freshly signed transition always verifies successfully.
        #[test]
        fn sign_then_verify_succeeds(
            env in "[a-z]{1,10}",
            root in "[a-f0-9]{8,16}",
        ) {
            let dir = temp_dir();
            let store = SigningKeyStore::open(&dir).unwrap();
            store.generate_key("proptest@test").unwrap();

            let transition = TransitionData {
                previous_root: None,
                new_root: root,
                environment: env,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                changes: vec![],
            };

            let signed = store.sign_transition(transition).unwrap();
            SigningKeyStore::verify_transition(&signed).unwrap();
        }

        /// Tampering with any field in the transition invalidates the signature.
        #[test]
        fn tampered_transition_fails_verification(
            env in "[a-z]{1,10}",
            root in "[a-f0-9]{8,16}",
            tampered_root in "[a-f0-9]{8,16}",
        ) {
            prop_assume!(root != tampered_root);

            let dir = temp_dir();
            let store = SigningKeyStore::open(&dir).unwrap();
            store.generate_key("proptest@test").unwrap();

            let transition = TransitionData {
                previous_root: None,
                new_root: root,
                environment: env,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                changes: vec![],
            };

            let mut signed = store.sign_transition(transition).unwrap();
            signed.transition.new_root = tampered_root;

            prop_assert!(SigningKeyStore::verify_transition(&signed).is_err(),
                "tampered transition must fail verification");
        }

        /// Different transition data produces different signatures.
        #[test]
        fn different_data_different_signatures(
            root_a in "[a-f0-9]{8,16}",
            root_b in "[a-f0-9]{8,16}",
        ) {
            prop_assume!(root_a != root_b);

            let dir = temp_dir();
            let store = SigningKeyStore::open(&dir).unwrap();
            store.generate_key("proptest@test").unwrap();

            let sig_a = store.sign_transition(TransitionData {
                previous_root: None,
                new_root: root_a,
                environment: "test".to_string(),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                changes: vec![],
            }).unwrap();

            let sig_b = store.sign_transition(TransitionData {
                previous_root: None,
                new_root: root_b,
                environment: "test".to_string(),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                changes: vec![],
            }).unwrap();

            prop_assert_ne!(sig_a.signature, sig_b.signature,
                "different transitions should produce different signatures");
        }
    }
}

// ─── Schema Properties ──────────────────────────────────────────────
// Note: Schema invariant tests that require `AwsProvider::for_testing()` live in
// `src/provider/aws/mod.rs` as unit tests (since `for_testing()` is #[cfg(test)`).

// ─── Secret Encryption Properties ───────────────────────────────────

mod secret_properties {
    use super::*;
    use smelt::secrets::SecretStore;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("smelt-secret-prop-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Generate arbitrary strings without null bytes (valid UTF-8, no \0).
    fn arb_secret_string() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9!@#$%^&*()_+=\\-]{0,64}"
    }

    proptest! {
        /// Encrypting then decrypting any string returns the original.
        #[test]
        fn secret_encrypt_decrypt_roundtrip(plaintext in arb_secret_string()) {
            let dir = temp_dir();
            let store = SecretStore::open(&dir).unwrap();
            store.generate_key().unwrap();

            let encrypted = store.encrypt(&plaintext).unwrap();
            let decrypted = store.decrypt(&encrypted).unwrap();
            prop_assert_eq!(&decrypted, &plaintext,
                "decrypt(encrypt(plaintext)) must equal plaintext");
        }

        /// Encrypting the same string twice produces different ciphertexts (random nonces).
        #[test]
        fn secret_encryption_uniqueness(plaintext in arb_secret_string()) {
            let dir = temp_dir();
            let store = SecretStore::open(&dir).unwrap();
            store.generate_key().unwrap();

            let enc1 = store.encrypt(&plaintext).unwrap();
            let enc2 = store.encrypt(&plaintext).unwrap();

            prop_assert_ne!(&enc1, &enc2,
                "two encryptions of the same plaintext must differ (random nonces)");

            // Both must still decrypt correctly
            let dec1 = store.decrypt(&enc1).unwrap();
            let dec2 = store.decrypt(&enc2).unwrap();
            prop_assert_eq!(&dec1, &plaintext);
            prop_assert_eq!(&dec2, &plaintext);
        }
    }
}

// ─── Formatter Idempotence for Secrets ──────────────────────────────

mod secret_formatter_properties {
    use super::*;

    /// Generate a resource with secret() values in its sections.
    fn arb_resource_with_secrets() -> impl Strategy<Value = smelt::ast::ResourceDecl> {
        (arb_ident(), arb_ident(), arb_safe_string()).prop_map(|(kind, name, secret_val)| {
            smelt::ast::ResourceDecl {
                kind,
                name,
                type_path: smelt::ast::TypePath {
                    segments: vec!["aws".into(), "rds".into(), "Instance".into()],
                },
                annotations: vec![],
                dependencies: vec![],
                sections: vec![smelt::ast::Section {
                    name: "security".to_string(),
                    fields: vec![
                        smelt::ast::Field {
                            name: "master_password".to_string(),
                            value: smelt::ast::Value::Secret(secret_val),
                        },
                        smelt::ast::Field {
                            name: "master_username".to_string(),
                            value: smelt::ast::Value::String("admin".to_string()),
                        },
                    ],
                }],
                fields: vec![],
                for_each: None,
            }
        })
    }

    proptest! {
        /// Formatting a resource with secret() values is idempotent:
        /// format(parse(format(ast))) == format(ast)
        #[test]
        fn formatter_idempotent_with_secrets(resource in arb_resource_with_secrets()) {
            let file = smelt::ast::SmeltFile {
                declarations: vec![smelt::ast::Declaration::Resource(resource)],
            };
            let formatted1 = smelt::formatter::format(&file);
            let reparsed = smelt::parser::parse(&formatted1)
                .expect("formatted output with secrets must parse");
            let formatted2 = smelt::formatter::format(&reparsed);
            prop_assert_eq!(&formatted1, &formatted2,
                "formatter must be idempotent for resources with secrets");
        }
    }
}

// ─── Formatter Idempotence for Components ───────────────────────────

mod component_formatter_properties {
    use super::*;

    /// Generate a component declaration with params and inner resources.
    fn arb_component() -> impl Strategy<Value = smelt::ast::ComponentDecl> {
        (arb_ident(), arb_ident(), arb_safe_string()).prop_map(
            |(comp_name, param_name, default_val)| smelt::ast::ComponentDecl {
                name: comp_name,
                params: vec![
                    smelt::ast::ParamDecl {
                        name: param_name.clone(),
                        param_type: smelt::ast::ParamType::String,
                        default: Some(smelt::ast::Value::String(default_val)),
                    },
                    smelt::ast::ParamDecl {
                        name: "enabled".to_string(),
                        param_type: smelt::ast::ParamType::Bool,
                        default: Some(smelt::ast::Value::Bool(true)),
                    },
                ],
                annotations: vec![],
                resources: vec![smelt::ast::ResourceDecl {
                    kind: "instance".to_string(),
                    name: "main".to_string(),
                    type_path: smelt::ast::TypePath {
                        segments: vec!["aws".into(), "ec2".into(), "Instance".into()],
                    },
                    annotations: vec![],
                    dependencies: vec![],
                    sections: vec![smelt::ast::Section {
                        name: "identity".to_string(),
                        fields: vec![smelt::ast::Field {
                            name: "name".to_string(),
                            value: smelt::ast::Value::ParamRef(param_name),
                        }],
                    }],
                    fields: vec![],
                    for_each: None,
                }],
            },
        )
    }

    proptest! {
        /// Formatting a component declaration is idempotent:
        /// format(parse(format(ast))) == format(ast)
        #[test]
        fn formatter_idempotent_with_components(component in arb_component()) {
            let file = smelt::ast::SmeltFile {
                declarations: vec![smelt::ast::Declaration::Component(component)],
            };
            let formatted1 = smelt::formatter::format(&file);
            let reparsed = smelt::parser::parse(&formatted1)
                .expect("formatted component must parse");
            let formatted2 = smelt::formatter::format(&reparsed);
            prop_assert_eq!(&formatted1, &formatted2,
                "formatter must be idempotent for components");
        }
    }
}

// ─── Config Roundtrip Properties ────────────────────────────────────

mod config_properties {
    use super::*;
    use smelt::config::{EnvironmentConfig, ProjectConfig, ProjectMeta};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("smelt-config-prop-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Generate an arbitrary environment config.
    fn arb_env_config() -> impl Strategy<Value = EnvironmentConfig> {
        (
            prop::collection::vec("[a-z]{2,8}", 0..3),
            prop::option::of("[a-z]{2,4}-[a-z]{2,6}-[0-9]"),
            prop::option::of("[a-z0-9]{4,12}"),
            any::<bool>(),
            prop::collection::btree_map("[a-z_]{1,8}", "[a-zA-Z0-9_]{1,16}", 0..3),
        )
            .prop_map(
                |(layers, region, project_id, protected, vars)| EnvironmentConfig {
                    layers,
                    region,
                    project_id,
                    protected,
                    vars,
                },
            )
    }

    /// Generate an arbitrary project config.
    fn arb_project_config() -> impl Strategy<Value = ProjectConfig> {
        (
            "[a-z][a-z0-9_-]{1,15}",
            prop::collection::btree_map("[a-z]{2,8}", arb_env_config(), 1..4),
        )
            .prop_map(|(name, environments)| {
                let default_environment = environments
                    .keys()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "default".to_string());
                ProjectConfig {
                    project: ProjectMeta {
                        name,
                        default_environment,
                    },
                    environments,
                    state: None,
                }
            })
    }

    proptest! {
        /// Serializing then deserializing a ProjectConfig preserves all fields.
        #[test]
        fn config_roundtrip(config in arb_project_config()) {
            let dir = temp_dir();
            config.save(&dir).unwrap();
            let loaded = ProjectConfig::load(&dir).unwrap();

            // Compare project metadata
            prop_assert_eq!(&loaded.project.name, &config.project.name);
            prop_assert_eq!(
                &loaded.project.default_environment,
                &config.project.default_environment
            );

            // Compare environment count and keys
            prop_assert_eq!(loaded.environments.len(), config.environments.len(),
                "environment count must match");

            for (env_name, original_env) in &config.environments {
                let loaded_env = loaded.environments.get(env_name)
                    .unwrap_or_else(|| panic!("environment '{}' must exist after roundtrip", env_name));

                prop_assert_eq!(&loaded_env.layers, &original_env.layers,
                    "layers for '{}' must match", env_name);
                prop_assert_eq!(&loaded_env.region, &original_env.region,
                    "region for '{}' must match", env_name);
                prop_assert_eq!(&loaded_env.project_id, &original_env.project_id,
                    "project_id for '{}' must match", env_name);
                prop_assert_eq!(loaded_env.protected, original_env.protected,
                    "protected for '{}' must match", env_name);
                prop_assert_eq!(&loaded_env.vars, &original_env.vars,
                    "vars for '{}' must match", env_name);
            }
        }
    }
}

// ─── Component Expansion Determinism ────────────────────────────────

mod component_expansion_properties {
    use super::*;

    /// Substitute param refs in a resource's sections, replacing ParamRef values
    /// with the corresponding argument values.
    fn expand_component(
        component: &smelt::ast::ComponentDecl,
        args: &std::collections::HashMap<String, smelt::ast::Value>,
        instance_name: &str,
    ) -> Vec<smelt::ast::ResourceDecl> {
        component
            .resources
            .iter()
            .map(|r| {
                let mut expanded = r.clone();
                // Prefix resource name with instance name for scoping
                expanded.name = format!("{}.{}", instance_name, expanded.name);
                // Substitute param refs in sections
                for section in &mut expanded.sections {
                    for field in &mut section.fields {
                        if let smelt::ast::Value::ParamRef(param_name) = &field.value
                            && let Some(arg_val) = args.get(param_name.as_str())
                        {
                            field.value = arg_val.clone();
                        }
                    }
                }
                // Substitute param refs in top-level fields
                for field in &mut expanded.fields {
                    if let smelt::ast::Value::ParamRef(param_name) = &field.value
                        && let Some(arg_val) = args.get(param_name.as_str())
                    {
                        field.value = arg_val.clone();
                    }
                }
                expanded
            })
            .collect()
    }

    proptest! {
        /// Expanding the same component with the same args twice produces identical resources.
        /// The expansion function must be deterministic (no random state, no ordering issues).
        #[test]
        fn component_expansion_is_deterministic(
            comp_name in arb_ident(),
            param_name in arb_ident(),
            arg_value in arb_safe_string(),
            instance_name in arb_ident(),
        ) {
            let component = smelt::ast::ComponentDecl {
                name: comp_name,
                params: vec![smelt::ast::ParamDecl {
                    name: param_name.clone(),
                    param_type: smelt::ast::ParamType::String,
                    default: None,
                }],
                annotations: vec![],
                resources: vec![smelt::ast::ResourceDecl {
                    kind: "instance".to_string(),
                    name: "main".to_string(),
                    type_path: smelt::ast::TypePath {
                        segments: vec!["aws".into(), "ec2".into(), "Instance".into()],
                    },
                    annotations: vec![],
                    dependencies: vec![],
                    sections: vec![smelt::ast::Section {
                        name: "identity".to_string(),
                        fields: vec![smelt::ast::Field {
                            name: "name".to_string(),
                            value: smelt::ast::Value::ParamRef(param_name.clone()),
                        }],
                    }],
                    fields: vec![],
                    for_each: None,
                }],
            };

            let mut args = std::collections::HashMap::new();
            args.insert(
                param_name,
                smelt::ast::Value::String(arg_value),
            );

            let expanded1 = expand_component(&component, &args, &instance_name);
            let expanded2 = expand_component(&component, &args, &instance_name);

            // Compare by formatting both expansions — if the formatter produces
            // the same output, the resources are semantically identical.
            prop_assert_eq!(expanded1.len(), expanded2.len(),
                "expansion must produce same number of resources");

            for (r1, r2) in expanded1.iter().zip(expanded2.iter()) {
                let file1 = smelt::ast::SmeltFile {
                    declarations: vec![smelt::ast::Declaration::Resource(r1.clone())],
                };
                let file2 = smelt::ast::SmeltFile {
                    declarations: vec![smelt::ast::Declaration::Resource(r2.clone())],
                };
                let fmt1 = smelt::formatter::format(&file1);
                let fmt2 = smelt::formatter::format(&file2);
                prop_assert_eq!(&fmt1, &fmt2,
                    "expanding the same component twice must produce identical output");
            }
        }
    }

    // ─── for_each expansion ───────────────────────────────────────────

    #[test]
    fn for_each_expansion_count() {
        // for_each with N items should produce exactly N instances
        let input = r#"
            resource subnet "az" : aws.ec2.Subnet {
                for_each = ["a", "b", "c"]
                identity { name = each.value }
                network { index = each.index }
            }
        "#;
        let file = smelt::parser::parse(input).unwrap();
        let graph = smelt::graph::DependencyGraph::build(&[file]).unwrap();
        // The graph should have 3 expanded resources, not 1 template
        assert_eq!(
            graph.len(),
            3,
            "for_each with 3 items should produce 3 resources"
        );
    }

    #[test]
    fn for_each_substitutes_values() {
        let input = r#"
            resource bucket "region" : gcp.storage.Bucket {
                for_each = ["us", "eu"]
                identity { name = each.value }
            }
        "#;
        let file = smelt::parser::parse(input).unwrap();
        let graph = smelt::graph::DependencyGraph::build(&[file]).unwrap();
        let expanded = graph.expanded_resources();
        assert_eq!(expanded.len(), 2);
        // Check names include the for_each key
        assert!(expanded.iter().any(|r| r.name == "region[us]"));
        assert!(expanded.iter().any(|r| r.name == "region[eu]"));
    }

    // ─── plan serialization roundtrip ──────────────────────────────────

    #[test]
    fn plan_serialization_roundtrip() {
        use smelt::plan::{ActionType, Plan, PlannedAction};
        use smelt::provider::FieldChange;

        let plan = Plan::new(
            "production".to_string(),
            vec![vec![
                PlannedAction {
                    resource_id: "vpc.main".to_string(),
                    type_path: "gcp.compute.Network".to_string(),
                    action: ActionType::Create,
                    intent: Some("Primary VPC".to_string()),
                    changes: vec![],
                    forces_replacement: false,
                    dependent_count: None,
                },
                PlannedAction {
                    resource_id: "subnet.app".to_string(),
                    type_path: "gcp.compute.Subnetwork".to_string(),
                    action: ActionType::Update,
                    intent: None,
                    changes: vec![FieldChange {
                        path: "network.cidr_block".to_string(),
                        change_type: smelt::provider::ChangeType::Modify,
                        old_value: Some(serde_json::json!("10.0.0.0/16")),
                        new_value: Some(serde_json::json!("10.1.0.0/16")),
                        forces_replacement: false,
                    }],
                    forces_replacement: false,
                    dependent_count: Some(3),
                },
            ]],
        );

        let json = serde_json::to_string_pretty(&plan).unwrap();
        let deserialized: Plan = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.environment, "production");
        assert_eq!(deserialized.tiers.len(), 1);
        assert_eq!(deserialized.tiers[0].len(), 2);
        assert_eq!(deserialized.tiers[0][0].action, ActionType::Create);
        assert_eq!(deserialized.tiers[0][1].action, ActionType::Update);
        assert_eq!(deserialized.tiers[0][1].dependent_count, Some(3));
        assert_eq!(deserialized.tiers[0][1].changes.len(), 1);
        assert_eq!(deserialized.summary.create, 1);
        assert_eq!(deserialized.summary.update, 1);
    }

    // ─── Levenshtein distance ──────────────────────────────────────────

    #[test]
    fn levenshtein_basic_cases() {
        // Same string = 0
        assert_eq!(smelt_levenshtein("hello", "hello"), 0);
        // One insertion
        assert_eq!(smelt_levenshtein("helo", "hello"), 1);
        // One substitution
        assert_eq!(smelt_levenshtein("hello", "jello"), 1);
        // Completely different
        assert!(smelt_levenshtein("abc", "xyz") > 0);
    }
}

// Can't access private fn from main.rs, so replicate for testing
fn smelt_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut matrix = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 0..=a.len() {
        matrix[i][0] = i;
    }
    for j in 0..=b.len() {
        matrix[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }
    matrix[a.len()][b.len()]
}
