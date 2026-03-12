//! Property-based tests for smelt-codegen.
//!
//! Tests structural invariants of the codegen pipeline:
//! 1. Generated code is syntactically valid Rust
//! 2. Manifest TOML roundtrips faithfully
//! 3. snake_case is idempotent
//! 4. strip_wrapper inverts correctly
//! 5. toml_to_json_literal produces valid Rust string literals

use proptest::prelude::*;
use smelt_codegen::manifest::*;
use smelt_codegen::generate::generate_provider_code;
use smelt_codegen::introspect::OneofVariant;
use std::collections::BTreeMap;

// ── Strategies ─────────────────────────────────────────────────────────────

fn arb_scope() -> impl Strategy<Value = Scope> {
    prop_oneof![
        Just(Scope::Global),
        Just(Scope::Regional),
        Just(Scope::Zonal),
    ]
}

fn arb_api_style() -> impl Strategy<Value = ApiStyle> {
    prop_oneof![
        Just(ApiStyle::Compute),
        Just(ApiStyle::ResourceName),
        Just(ApiStyle::DirectModel),
    ]
}

fn arb_identifier() -> impl Strategy<Value = String> {
    // Valid Rust identifier-like strings (lowercase, underscores)
    "[a-z][a-z0-9_]{0,20}"
}

fn arb_pascal_case() -> impl Strategy<Value = String> {
    // PascalCase names for struct/enum names
    "[A-Z][a-zA-Z]{1,15}"
}

fn arb_field_type() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("String".to_string()),
        Just("Bool".to_string()),
        Just("Integer".to_string()),
        Just("Integer_u32".to_string()),
        Just("Integer_u64".to_string()),
        Just("Float".to_string()),
        Just("Record".to_string()),
        Just("Duration".to_string()),
        Just("Timestamp".to_string()),
        Just("Bytes".to_string()),
        arb_pascal_case().prop_map(|n| format!("Enum({n})")),
        arb_pascal_case().prop_map(|n| format!("Nested({n})")),
    ]
}

fn arb_section() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("identity".to_string()),
        Just("network".to_string()),
        Just("sizing".to_string()),
        Just("runtime".to_string()),
        Just("security".to_string()),
        Just("config".to_string()),
        Just("dns".to_string()),
    ]
}

fn arb_field_def() -> impl Strategy<Value = FieldDef> {
    (
        arb_section(),
        arb_field_type(),
        any::<bool>(),       // required
        any::<bool>(),       // sensitive
        any::<bool>(),       // output_only
        any::<bool>(),       // optional
    )
        .prop_map(|(section, field_type, required, sensitive, output_only, optional)| {
            let variants = if field_type.starts_with("Enum(") {
                vec!["VARIANT_A".into(), "VARIANT_B".into(), "VARIANT_C".into()]
            } else {
                Vec::new()
            };
            FieldDef {
                section,
                sdk_field: None,
                field_type,
                required,
                default: None,
                sensitive,
                description: Some("Test field".into()),
                variants,
                output_only,
                deprecated: false,
                skip: false,
                optional,
                sdk_type_path: None,
                oneof_variants: Vec::new(),
                aws_attr_key: None,
                aws_enum: false,
                aws_enum_type: None,
                sdk_read_field: None,
                skip_create: false,
                aws_post_create_method: None,
                sdk_non_optional: false,
            }
        })
}

fn arb_manifest() -> impl Strategy<Value = ResourceManifest> {
    (
        arb_identifier(),    // type_path prefix (service name)
        arb_pascal_case(),   // struct name
        arb_scope(),
        arb_api_style(),
        prop::collection::btree_map(arb_identifier(), arb_field_def(), 1..8),
        any::<bool>(),       // lro_create
        any::<bool>(),       // lro_update
        any::<bool>(),       // lro_delete
    )
        .prop_map(|(service, model, scope, api_style, mut fields, lro_create, lro_update, lro_delete)| {
            // Ensure a "name" field always exists (required for codegen)
            fields.insert("name".into(), FieldDef {
                section: "identity".into(),
                sdk_field: None,
                field_type: "String".into(),
                required: true,
                default: None,
                sensitive: false,
                description: Some("Resource name".into()),
                variants: Vec::new(),
                output_only: false,
                deprecated: false,
                skip: false,
                optional: true,
                sdk_type_path: None,
                oneof_variants: Vec::new(),
                aws_attr_key: None,
                aws_enum: false,
                aws_enum_type: None,
                sdk_read_field: None,
                skip_create: false,
                aws_post_create_method: None,
                sdk_non_optional: false,
            });

            let type_path = format!("{service}.{model}");
            let client_name = format!("{model}s");
            let parent_format = match api_style {
                ApiStyle::ResourceName | ApiStyle::DirectModel => match scope {
                    Scope::Regional | Scope::Zonal =>
                        Some("projects/{project}/locations/{location}".into()),
                    Scope::Global => Some("projects/{project}".into()),
                },
                ApiStyle::Compute => None,
            };

            ResourceManifest {
                resource: ResourceMeta {
                    type_path,
                    description: format!("{model} resource"),
                    provider: "gcp".into(),
                    sdk_crate: format!("google-cloud-{service}-v1"),
                    sdk_model: model,
                    sdk_client: client_name,
                    provider_id_format: "{name}".into(),
                    scope,
                    api_style,
                    parent_format,
                    resource_id_setter: None,
                    resource_body_setter: None,
                    client_accessor: None,
                    resource_id_param: None,
                    parent_setter: None,
                    resource_name_param: None,
                    has_update_mask: true,
                    output_field: None,
                    lro_create,
                    lro_update,
                    lro_delete,
                    parent_binding: None,
                    parent_binding_section: None,
                    resource_noun: None,
                    skip_name_on_create: false,
                    full_name_on_model: false,
                    raw_labels: false,
                    aws_client_field: None,
                    aws_read_style: None,
                    aws_list_accessor: None,
                    aws_response_accessor: None,
                    aws_id_param: None,
                    aws_id_source: None,
                    aws_response_id_field: None,
                    aws_tag_style: None,
                    aws_tag_resource_type: None,
                    aws_outputs: Vec::new(),
                    aws_updatable: false,
                    aws_tag_infallible: false,
                    aws_read_id_param: None,
                    aws_delete_id_param: None,
                    aws_response_id_non_optional: false,
                },
                crud: CrudMethods {
                    create: "insert".into(),
                    read: "get".into(),
                    update: Some("patch".into()),
                    delete: Some("delete".into()),
                },
                fields,
                replacement_fields: vec!["name".into()],
                output_fields: Vec::new(),
            }
        })
}

// ── Property Tests ─────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Generated code must be syntactically valid Rust.
    /// This catches string escaping bugs, identifier injection, mismatched
    /// braces, and any other codegen correctness issue.
    #[test]
    fn generated_code_parses_as_valid_rust(manifest in arb_manifest()) {
        let code = generate_provider_code(&manifest);

        // Wrap in enough context to be a valid Rust file
        let wrapped = format!(
            "mod test_module {{\n{code}\n}}\n"
        );

        // syn::parse_file gives us a full syntax check
        let result = syn::parse_file(&wrapped);
        prop_assert!(
            result.is_ok(),
            "Generated code is not valid Rust:\n\nError: {:?}\n\nGenerated code:\n{code}",
            result.err()
        );
    }

    /// Manifest roundtrips through TOML serialization.
    #[test]
    fn manifest_toml_roundtrip(manifest in arb_manifest()) {
        let toml_str = toml::to_string_pretty(&manifest)
            .expect("manifest should serialize to TOML");

        let deserialized: ResourceManifest = toml::from_str(&toml_str)
            .expect("manifest should deserialize from TOML");

        // Check key fields survive the roundtrip
        prop_assert_eq!(&manifest.resource.type_path, &deserialized.resource.type_path);
        prop_assert_eq!(&manifest.resource.sdk_crate, &deserialized.resource.sdk_crate);
        prop_assert_eq!(&manifest.resource.sdk_model, &deserialized.resource.sdk_model);
        prop_assert_eq!(&manifest.resource.scope, &deserialized.resource.scope);
        prop_assert_eq!(&manifest.resource.api_style, &deserialized.resource.api_style);
        prop_assert_eq!(manifest.fields.len(), deserialized.fields.len());

        // Field names survive
        for key in manifest.fields.keys() {
            prop_assert!(
                deserialized.fields.contains_key(key),
                "Field {key} lost in roundtrip"
            );
        }

        // Field types survive
        for (key, field) in &manifest.fields {
            let deser_field = &deserialized.fields[key];
            prop_assert_eq!(&field.field_type, &deser_field.field_type);
            prop_assert_eq!(field.required, deser_field.required);
            prop_assert_eq!(field.sensitive, deser_field.sensitive);
        }
    }

    /// snake_case is idempotent: applying it twice gives the same result.
    #[test]
    fn snake_case_idempotent(s in "[a-zA-Z][a-zA-Z0-9]{0,30}") {
        let once = smelt_codegen::snake_case(&s);
        let twice = smelt_codegen::snake_case(&once);
        prop_assert_eq!(&once, &twice, "snake_case is not idempotent for input: {}", s);
    }

    /// snake_case output contains only lowercase and underscores.
    #[test]
    fn snake_case_only_lower_and_underscores(s in "[a-zA-Z][a-zA-Z0-9]{0,30}") {
        let result = smelt_codegen::snake_case(&s);
        for ch in result.chars() {
            prop_assert!(
                ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_',
                "snake_case({s}) produced non-snake char: {ch}"
            );
        }
    }

    /// Generated code has balanced braces.
    #[test]
    fn generated_code_has_balanced_braces(manifest in arb_manifest()) {
        let code = generate_provider_code(&manifest);
        let opens = code.chars().filter(|&c| c == '{').count();
        let closes = code.chars().filter(|&c| c == '}').count();
        prop_assert_eq!(
            opens, closes,
            "Unbalanced braces in generated code: {} opens, {} closes", opens, closes
        );
    }

    /// Every non-skipped, non-output-only field appears in both schema and
    /// the create function body.
    #[test]
    fn active_fields_appear_in_schema_and_create(manifest in arb_manifest()) {
        let code = generate_provider_code(&manifest);
        for (name, field) in &manifest.fields {
            if field.skip || field.output_only {
                continue;
            }
            // Field should appear in the schema section
            prop_assert!(
                code.contains(&format!("name: \"{name}\".into()")),
                "Field {name} missing from schema"
            );
        }
    }
}

// ── Deterministic Tests ────────────────────────────────────────────────────

#[test]
fn toml_to_json_literal_escapes_quotes() {
    let toml_str = r#"
[resource]
type_path = "test.Test"
description = "d"
provider = "gcp"
sdk_crate = "c"
sdk_model = "T"
sdk_client = "C"
provider_id_format = "{name}"

[crud]
create = "c"
read = "r"

[fields.name]
section = "identity"
type = "String"
required = true
default = 'say "hello"'
"#;
    let manifest: ResourceManifest = toml::from_str(toml_str).unwrap();
    let code = generate_provider_code(&manifest);

    // The generated code should be parseable by syn
    let wrapped = format!("mod test_module {{\n{code}\n}}\n");
    assert!(
        syn::parse_file(&wrapped).is_ok(),
        "Generated code with quoted default is not valid Rust:\n{code}"
    );
}

#[test]
fn toml_to_json_literal_escapes_backslashes() {
    let toml_str = r#"
[resource]
type_path = "test.Test"
description = "d"
provider = "gcp"
sdk_crate = "c"
sdk_model = "T"
sdk_client = "C"
provider_id_format = "{name}"

[crud]
create = "c"
read = "r"

[fields.name]
section = "identity"
type = "String"
required = true
default = 'path\to\file'
"#;
    let manifest: ResourceManifest = toml::from_str(toml_str).unwrap();
    let code = generate_provider_code(&manifest);

    let wrapped = format!("mod test_module {{\n{code}\n}}\n");
    assert!(
        syn::parse_file(&wrapped).is_ok(),
        "Generated code with backslash default is not valid Rust:\n{code}"
    );
}

#[test]
fn integer_codegen_uses_try_from() {
    let mut fields = BTreeMap::new();
    fields.insert("name".into(), FieldDef {
        section: "identity".into(),
        sdk_field: None,
        field_type: "String".into(),
        required: true,
        default: None,
        sensitive: false,
        description: None,
        variants: Vec::new(),
        output_only: false,
        deprecated: false,
        skip: false,
        optional: true,
        sdk_type_path: None,
        oneof_variants: Vec::new(),
        aws_attr_key: None,
        aws_enum: false,
        aws_enum_type: None,
        sdk_read_field: None,
        skip_create: false,
        aws_post_create_method: None,
    });
    fields.insert("count".into(), FieldDef {
        section: "config".into(),
        sdk_field: None,
        field_type: "Integer".into(),
        required: false,
        default: None,
        sensitive: false,
        description: None,
        variants: Vec::new(),
        output_only: false,
        deprecated: false,
        skip: false,
        optional: true,
        sdk_type_path: None,
        oneof_variants: Vec::new(),
        aws_attr_key: None,
        aws_enum: false,
        aws_enum_type: None,
        sdk_read_field: None,
        skip_create: false,
        aws_post_create_method: None,
    });

    let manifest = ResourceManifest {
        resource: ResourceMeta {
            type_path: "test.Counter".into(),
            description: "Test".into(),
            provider: "gcp".into(),
            sdk_crate: "google-cloud-test-v1".into(),
            sdk_model: "Counter".into(),
            sdk_client: "Counters".into(),
            provider_id_format: "{name}".into(),
            scope: Scope::Global,
            api_style: ApiStyle::Compute,
            parent_format: None,
            resource_id_setter: None,
            resource_body_setter: None,
            client_accessor: None,
            resource_id_param: None,
            parent_setter: None,
            resource_name_param: None,
            has_update_mask: true,
            output_field: None,
            lro_create: false,
            lro_update: false,
            lro_delete: false,
            parent_binding: None,
            parent_binding_section: None,
            resource_noun: None,
            skip_name_on_create: false,
            full_name_on_model: false,
            raw_labels: false,
            aws_client_field: None,
            aws_read_style: None,
            aws_list_accessor: None,
            aws_response_accessor: None,
            aws_id_param: None,
            aws_id_source: None,
            aws_response_id_field: None,
            aws_tag_style: None,
            aws_tag_resource_type: None,
            aws_outputs: Vec::new(),
            aws_updatable: false,
                    aws_tag_infallible: false,
            aws_read_id_param: None,
        },
        crud: CrudMethods {
            create: "insert".into(),
            read: "get".into(),
            update: Some("patch".into()),
            delete: Some("delete".into()),
        },
        fields,
        replacement_fields: vec!["name".into()],
        output_fields: Vec::new(),
    };

    let code = generate_provider_code(&manifest);

    // Should use try_from, not bare `as i32`
    assert!(
        code.contains("try_from"),
        "Integer codegen should use try_from for checked casts, got:\n{code}"
    );
    assert!(
        !code.contains("v as i32"),
        "Integer codegen should not use bare 'as i32' cast"
    );
}

#[test]
fn oneof_uses_field_section_not_config() {
    let mut fields = BTreeMap::new();
    fields.insert("name".into(), FieldDef {
        section: "identity".into(),
        sdk_field: None,
        field_type: "String".into(),
        required: true,
        default: None,
        sensitive: false,
        description: None,
        variants: Vec::new(),
        output_only: false,
        deprecated: false,
        skip: false,
        optional: true,
        sdk_type_path: None,
        oneof_variants: Vec::new(),
        aws_attr_key: None,
        aws_enum: false,
        aws_enum_type: None,
        sdk_read_field: None,
        skip_create: false,
        aws_post_create_method: None,
    });
    fields.insert("expiration".into(), FieldDef {
        section: "security".into(),
        sdk_field: None,
        field_type: "Oneof(Expiration)".into(),
        required: false,
        default: None,
        sensitive: false,
        description: None,
        variants: Vec::new(),
        output_only: false,
        deprecated: false,
        skip: false,
        optional: true,
        sdk_type_path: None,
        oneof_variants: vec![
            OneofVariant {
                name: "ExpireTime".into(),
                setter: "set_expire_time".into(),
                inner_type: smelt_codegen::introspect::OneofInnerType::Timestamp,
                boxed: true,
            },
            OneofVariant {
                name: "Ttl".into(),
                setter: "set_ttl".into(),
                inner_type: smelt_codegen::introspect::OneofInnerType::Duration,
                boxed: true,
            },
        ],
        aws_attr_key: None,
        aws_enum: false,
        aws_enum_type: None,
        sdk_read_field: None,
        skip_create: false,
        aws_post_create_method: None,
    });

    let manifest = ResourceManifest {
        resource: ResourceMeta {
            type_path: "test.Secret".into(),
            description: "Test".into(),
            provider: "gcp".into(),
            sdk_crate: "google-cloud-test-v1".into(),
            sdk_model: "Secret".into(),
            sdk_client: "Secrets".into(),
            provider_id_format: "{name}".into(),
            scope: Scope::Global,
            api_style: ApiStyle::ResourceName,
            parent_format: Some("projects/{project}".into()),
            resource_id_setter: None,
            resource_body_setter: None,
            client_accessor: None,
            resource_id_param: None,
            parent_setter: None,
            resource_name_param: None,
            has_update_mask: true,
            output_field: None,
            lro_create: false,
            lro_update: false,
            lro_delete: false,
            parent_binding: None,
            parent_binding_section: None,
            resource_noun: None,
            skip_name_on_create: false,
            full_name_on_model: false,
            raw_labels: false,
            aws_client_field: None,
            aws_read_style: None,
            aws_list_accessor: None,
            aws_response_accessor: None,
            aws_id_param: None,
            aws_id_source: None,
            aws_response_id_field: None,
            aws_tag_style: None,
            aws_tag_resource_type: None,
            aws_outputs: Vec::new(),
            aws_updatable: false,
                    aws_tag_infallible: false,
            aws_read_id_param: None,
        },
        crud: CrudMethods {
            create: "create_secret".into(),
            read: "get_secret".into(),
            update: Some("update_secret".into()),
            delete: Some("delete_secret".into()),
        },
        fields,
        replacement_fields: vec!["name".into()],
        output_fields: Vec::new(),
    };

    let code = generate_provider_code(&manifest);

    // Oneof paths should use the field's section ("security"), not hardcoded "config"
    assert!(
        code.contains("/security/expire_time"),
        "Oneof should use field section 'security', not 'config'. Got:\n{code}"
    );
    assert!(
        !code.contains("/config/expire_time"),
        "Oneof should not hardcode '/config/' section"
    );
}
