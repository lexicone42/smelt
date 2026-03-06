use crate::ast::*;

/// Format a SmeltFile into its canonical form.
///
/// Canonical form guarantees:
/// - Consistent 2-space indentation
/// - Annotations before dependencies before sections before fields
/// - Fields within sections sorted alphabetically
/// - Single blank line between top-level declarations
/// - No trailing whitespace
/// - Single trailing newline
pub fn format(file: &SmeltFile) -> String {
    let mut out = String::new();

    for (i, decl) in file.declarations.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_declaration(&mut out, decl, 0);
    }

    out
}

fn format_declaration(out: &mut String, decl: &Declaration, indent: usize) {
    match decl {
        Declaration::Resource(r) => format_resource(out, r, indent),
        Declaration::Layer(l) => format_layer(out, l, indent),
    }
}

fn format_resource(out: &mut String, resource: &ResourceDecl, indent: usize) {
    let pad = "  ".repeat(indent);
    out.push_str(&format!(
        "{pad}resource {} \"{}\" : {} {{\n",
        resource.kind, resource.name, resource.type_path
    ));

    let inner = indent + 1;

    // Canonical order: annotations, dependencies, sections (sorted), fields (sorted)
    format_annotations(out, &resource.annotations, inner);
    format_dependencies(out, &resource.dependencies, inner);

    // Blank line between annotations/deps and sections if both exist
    if (!resource.annotations.is_empty() || !resource.dependencies.is_empty())
        && (!resource.sections.is_empty() || !resource.fields.is_empty())
    {
        out.push('\n');
    }

    let mut sorted_sections = resource.sections.clone();
    sorted_sections.sort_by(|a, b| a.name.cmp(&b.name));
    for (i, section) in sorted_sections.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_section(out, section, inner);
    }

    if !resource.sections.is_empty() && !resource.fields.is_empty() {
        out.push('\n');
    }

    let mut sorted_fields = resource.fields.clone();
    sorted_fields.sort_by(|a, b| a.name.cmp(&b.name));
    for field in &sorted_fields {
        format_field(out, field, inner);
    }

    out.push_str(&format!("{pad}}}\n"));
}

fn format_layer(out: &mut String, layer: &LayerDecl, indent: usize) {
    let pad = "  ".repeat(indent);
    out.push_str(&format!(
        "{pad}layer \"{}\" over \"{}\" {{\n",
        layer.name, layer.base
    ));

    let inner = indent + 1;
    format_annotations(out, &layer.annotations, inner);

    if !layer.annotations.is_empty() && !layer.overrides.is_empty() {
        out.push('\n');
    }

    for (i, ovr) in layer.overrides.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_override(out, ovr, inner);
    }

    out.push_str(&format!("{pad}}}\n"));
}

fn format_override(out: &mut String, ovr: &Override, indent: usize) {
    let pad = "  ".repeat(indent);
    out.push_str(&format!("{pad}override {} {{\n", ovr.pattern));

    let inner = indent + 1;

    let mut sorted_sections = ovr.sections.clone();
    sorted_sections.sort_by(|a, b| a.name.cmp(&b.name));
    for (i, section) in sorted_sections.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_section(out, section, inner);
    }

    if !ovr.sections.is_empty() && !ovr.fields.is_empty() {
        out.push('\n');
    }

    let mut sorted_fields = ovr.fields.clone();
    sorted_fields.sort_by(|a, b| a.name.cmp(&b.name));
    for field in &sorted_fields {
        format_field(out, field, inner);
    }

    out.push_str(&format!("{pad}}}\n"));
}

fn format_annotations(out: &mut String, annotations: &[Annotation], indent: usize) {
    // Canonical annotation order: intent, owner, constraint, lifecycle
    let order = [
        AnnotationKind::Intent,
        AnnotationKind::Owner,
        AnnotationKind::Constraint,
        AnnotationKind::Lifecycle,
    ];

    for kind in &order {
        for ann in annotations {
            if ann.kind == *kind {
                let pad = "  ".repeat(indent);
                out.push_str(&format!(
                    "{pad}@{} \"{}\"\n",
                    ann.kind,
                    escape_string(&ann.value)
                ));
            }
        }
    }
}

fn format_dependencies(out: &mut String, deps: &[Dependency], indent: usize) {
    // Sort dependencies by source path for canonical order
    let mut sorted = deps.to_vec();
    sorted.sort_by(|a, b| a.source.to_string().cmp(&b.source.to_string()));

    if !sorted.is_empty() {
        out.push('\n');
    }

    for dep in &sorted {
        let pad = "  ".repeat(indent);
        out.push_str(&format!("{pad}needs {} -> {}\n", dep.source, dep.binding));
    }
}

fn format_section(out: &mut String, section: &Section, indent: usize) {
    let pad = "  ".repeat(indent);
    out.push_str(&format!("{pad}{} {{\n", section.name));

    let mut sorted_fields = section.fields.clone();
    sorted_fields.sort_by(|a, b| a.name.cmp(&b.name));

    for field in &sorted_fields {
        format_field(out, field, indent + 1);
    }

    out.push_str(&format!("{pad}}}\n"));
}

fn format_field(out: &mut String, field: &Field, indent: usize) {
    let pad = "  ".repeat(indent);
    out.push_str(&format!("{pad}{} = ", field.name));
    format_value(out, &field.value, indent);
    out.push('\n');
}

fn format_value(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::String(s) => {
            out.push_str(&format!("\"{}\"", escape_string(s)));
        }
        Value::Number(n) => {
            out.push_str(&format!("{n}"));
        }
        Value::Integer(n) => {
            out.push_str(&format!("{n}"));
        }
        Value::Bool(b) => {
            out.push_str(if *b { "true" } else { "false" });
        }
        Value::Ref(r) => {
            out.push_str(&format!("ref({r})"));
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
            } else if is_simple_array(items) {
                // Single-line for arrays of simple values
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    format_value(out, item, indent);
                }
                out.push(']');
            } else {
                // Multi-line for arrays containing records or nested arrays
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    let inner_pad = "  ".repeat(indent + 1);
                    out.push_str(&inner_pad);
                    format_value(out, item, indent + 1);
                    if i < items.len() - 1 {
                        out.push(',');
                    }
                    out.push('\n');
                }
                let pad = "  ".repeat(indent);
                out.push_str(&pad);
                out.push(']');
            }
        }
        Value::Record(fields) => {
            if fields.is_empty() {
                out.push_str("{}");
            } else {
                let mut sorted = fields.clone();
                sorted.sort_by(|a, b| a.name.cmp(&b.name));

                if is_simple_record(&sorted) {
                    // Single-line for simple records
                    out.push_str("{ ");
                    for (i, f) in sorted.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(&format!("{} = ", f.name));
                        format_value(out, &f.value, indent);
                    }
                    out.push_str(" }");
                } else {
                    // Multi-line for complex records
                    out.push_str("{\n");
                    for f in &sorted {
                        format_field(out, f, indent + 1);
                    }
                    let pad = "  ".repeat(indent);
                    out.push_str(&format!("{pad}}}"));
                }
            }
        }
    }
}

/// Returns true if all items are simple scalars (string, number, integer, bool).
fn is_simple_array(items: &[Value]) -> bool {
    items.iter().all(|v| {
        matches!(
            v,
            Value::String(_)
                | Value::Number(_)
                | Value::Integer(_)
                | Value::Bool(_)
                | Value::Ref(_)
        )
    })
}

/// Returns true if all fields have simple scalar values.
fn is_simple_record(fields: &[Field]) -> bool {
    fields.len() <= 4
        && fields.iter().all(|f| {
            matches!(
                f.value,
                Value::String(_)
                    | Value::Number(_)
                    | Value::Integer(_)
                    | Value::Bool(_)
                    | Value::Ref(_)
            )
        })
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn format_is_idempotent() {
        let input = r#"resource vpc "main" : aws.ec2.Vpc {
            @intent "Primary VPC"
            @owner "platform-team"
            network {
                cidr_block = "10.0.0.0/16"
                dns_support = true
            }
            identity {
                name = "prod-vpc"
            }
        }"#;

        let parsed = parser::parse(input).expect("should parse");
        let formatted1 = format(&parsed);
        let reparsed = parser::parse(&formatted1).expect("formatted output should parse");
        let formatted2 = format(&reparsed);

        assert_eq!(formatted1, formatted2, "format should be idempotent");
    }

    #[test]
    fn format_sorts_sections_alphabetically() {
        let input = r#"resource vpc "main" : aws.ec2.Vpc {
            @intent "Test"
            network {
                cidr = "10.0.0.0/16"
            }
            identity {
                name = "test"
            }
        }"#;

        let parsed = parser::parse(input).expect("should parse");
        let formatted = format(&parsed);

        let identity_pos = formatted.find("identity").unwrap();
        let network_pos = formatted.find("network").unwrap();
        assert!(
            identity_pos < network_pos,
            "identity should come before network (alphabetical)"
        );
    }

    #[test]
    fn format_sorts_fields_alphabetically() {
        let input = r#"resource vpc "main" : aws.ec2.Vpc {
            @intent "Test"
            network {
                dns_support = true
                cidr_block = "10.0.0.0/16"
            }
        }"#;

        let parsed = parser::parse(input).expect("should parse");
        let formatted = format(&parsed);

        let cidr_pos = formatted.find("cidr_block").unwrap();
        let dns_pos = formatted.find("dns_support").unwrap();
        assert!(
            cidr_pos < dns_pos,
            "cidr_block should come before dns_support (alphabetical)"
        );
    }

    #[test]
    fn format_canonical_annotation_order() {
        // Annotations should always be: intent, owner, constraint, lifecycle
        let input = r#"resource vpc "main" : aws.ec2.Vpc {
            @owner "team"
            @intent "Test"
            @lifecycle "prevent_destroy"
        }"#;

        let parsed = parser::parse(input).expect("should parse");
        let formatted = format(&parsed);

        let intent_pos = formatted.find("@intent").unwrap();
        let owner_pos = formatted.find("@owner").unwrap();
        let lifecycle_pos = formatted.find("@lifecycle").unwrap();
        assert!(intent_pos < owner_pos);
        assert!(owner_pos < lifecycle_pos);
    }
}
