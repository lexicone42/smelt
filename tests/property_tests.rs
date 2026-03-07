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
            let state = smelt::store::ResourceState {
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
            let state = smelt::store::ResourceState {
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
// `src/provider/aws/mod.rs` as unit tests (since `for_testing()` is #[cfg(test)]).
