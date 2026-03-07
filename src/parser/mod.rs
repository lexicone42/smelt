use chumsky::prelude::*;

use crate::ast::*;

/// Parse a complete .smelt file from source text.
pub fn parse(source: &str) -> Result<SmeltFile, Vec<Simple<char>>> {
    file_parser().parse(source)
}

/// Skip whitespace and `#` line comments.
fn ws() -> impl Parser<char, (), Error = Simple<char>> + Clone {
    let line_comment = just('#')
        .then(none_of('\n').repeated())
        .then(just('\n').or_not())
        .ignored();

    filter(|c: &char| c.is_whitespace())
        .ignored()
        .or(line_comment)
        .repeated()
        .ignored()
}

/// Top-level file parser: zero or more declarations.
fn file_parser() -> impl Parser<char, SmeltFile, Error = Simple<char>> {
    ws().ignore_then(
        declaration()
            .repeated()
            .then_ignore(end())
            .map(|declarations| SmeltFile { declarations }),
    )
}

/// A declaration is either a resource or a layer.
fn declaration() -> impl Parser<char, Declaration, Error = Simple<char>> {
    resource_decl()
        .map(Declaration::Resource)
        .or(layer_decl().map(Declaration::Layer))
        .padded_by(ws())
}

/// Parse a resource declaration:
/// `resource [kind] "name" : type.path { body }`
///
/// When `kind` is omitted, it is derived from the type path's last segment (lowercased).
/// e.g. `resource "main" : aws.ec2.Vpc { ... }` → kind = "vpc"
fn resource_decl() -> impl Parser<char, ResourceDecl, Error = Simple<char>> {
    text::keyword("resource")
        .padded()
        .ignore_then(ident().then_ignore(text::whitespace()).or_not())
        .then(string_literal())
        .padded()
        .then_ignore(just(':').padded())
        .then(type_path())
        .padded()
        .then(resource_body().delimited_by(just('{').padded(), just('}').padded()))
        .map(|(((kind, name), type_path), body)| {
            let kind = kind.unwrap_or_else(|| {
                type_path
                    .segments
                    .last()
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default()
            });
            ResourceDecl {
                kind,
                name,
                type_path,
                annotations: body.annotations,
                dependencies: body.dependencies,
                sections: body.sections,
                fields: body.fields,
            }
        })
}

/// Parse a layer declaration:
/// `layer "name" over "base" { overrides }`
fn layer_decl() -> impl Parser<char, LayerDecl, Error = Simple<char>> {
    text::keyword("layer")
        .padded()
        .ignore_then(string_literal())
        .padded()
        .then_ignore(text::keyword("over").padded())
        .then(string_literal())
        .padded()
        .then(layer_body().delimited_by(just('{').padded(), just('}').padded()))
        .map(|((name, base), body)| LayerDecl {
            name,
            base,
            annotations: body.annotations,
            overrides: body.overrides,
        })
}

/// Intermediate struct for collecting resource body elements.
struct ResourceBody {
    annotations: Vec<Annotation>,
    dependencies: Vec<Dependency>,
    sections: Vec<Section>,
    fields: Vec<Field>,
}

/// Intermediate struct for collecting layer body elements.
struct LayerBody {
    annotations: Vec<Annotation>,
    overrides: Vec<Override>,
}

/// A resource body element — annotation, dependency, section, or field.
enum ResourceBodyItem {
    Annotation(Annotation),
    Dependency(Dependency),
    Section(Section),
    Field(Field),
}

/// Parse the body of a resource declaration.
fn resource_body() -> impl Parser<char, ResourceBody, Error = Simple<char>> {
    resource_body_item()
        .padded_by(ws())
        .repeated()
        .map(|items| {
            let mut annotations = Vec::new();
            let mut dependencies = Vec::new();
            let mut sections = Vec::new();
            let mut fields = Vec::new();

            for item in items {
                match item {
                    ResourceBodyItem::Annotation(a) => annotations.push(a),
                    ResourceBodyItem::Dependency(d) => dependencies.push(d),
                    ResourceBodyItem::Section(s) => sections.push(s),
                    ResourceBodyItem::Field(f) => fields.push(f),
                }
            }

            ResourceBody {
                annotations,
                dependencies,
                sections,
                fields,
            }
        })
}

/// Parse a single resource body item.
fn resource_body_item() -> impl Parser<char, ResourceBodyItem, Error = Simple<char>> {
    annotation()
        .map(ResourceBodyItem::Annotation)
        .or(dependency().map(ResourceBodyItem::Dependency))
        .or(section().map(ResourceBodyItem::Section))
        .or(field().map(ResourceBodyItem::Field))
}

/// Parse the body of a layer declaration.
fn layer_body() -> impl Parser<char, LayerBody, Error = Simple<char>> {
    enum LayerBodyItem {
        Annotation(Annotation),
        Override(Override),
    }

    let item = annotation()
        .map(LayerBodyItem::Annotation)
        .or(override_decl().map(LayerBodyItem::Override));

    item.padded_by(ws()).repeated().map(|items| {
        let mut annotations = Vec::new();
        let mut overrides = Vec::new();
        for item in items {
            match item {
                LayerBodyItem::Annotation(a) => annotations.push(a),
                LayerBodyItem::Override(o) => overrides.push(o),
            }
        }
        LayerBody {
            annotations,
            overrides,
        }
    })
}

/// Parse an override block:
/// `override pattern.* { sections and fields }`
fn override_decl() -> impl Parser<char, Override, Error = Simple<char>> {
    text::keyword("override")
        .padded()
        .ignore_then(override_pattern())
        .padded()
        .then(override_body().delimited_by(just('{').padded(), just('}').padded()))
        .map(|(pattern, body)| Override {
            pattern,
            sections: body.0,
            fields: body.1,
        })
}

/// Parse an override pattern like `compute.*` or `data.databases`
fn override_pattern() -> impl Parser<char, String, Error = Simple<char>> {
    let segment = filter(|c: &char| c.is_alphanumeric() || *c == '_' || *c == '*')
        .repeated()
        .at_least(1)
        .collect::<String>();

    segment
        .separated_by(just('.'))
        .at_least(1)
        .map(|segments: Vec<String>| segments.join("."))
}

/// Parse the body of an override block.
fn override_body() -> impl Parser<char, (Vec<Section>, Vec<Field>), Error = Simple<char>> {
    enum Item {
        Section(Section),
        Field(Field),
    }

    let item = section().map(Item::Section).or(field().map(Item::Field));

    item.padded_by(ws()).repeated().map(|items| {
        let mut sections = Vec::new();
        let mut fields = Vec::new();
        for item in items {
            match item {
                Item::Section(s) => sections.push(s),
                Item::Field(f) => fields.push(f),
            }
        }
        (sections, fields)
    })
}

/// Parse an annotation: `@intent "description"`
fn annotation() -> impl Parser<char, Annotation, Error = Simple<char>> {
    just('@')
        .ignore_then(ident())
        .padded()
        .then(string_literal())
        .try_map(|(kind_str, value), span| {
            let kind = match kind_str.as_str() {
                "intent" => AnnotationKind::Intent,
                "owner" => AnnotationKind::Owner,
                "constraint" => AnnotationKind::Constraint,
                "lifecycle" => AnnotationKind::Lifecycle,
                _ => {
                    return Err(Simple::custom(
                        span,
                        format!("unknown annotation '@{kind_str}', expected one of: @intent, @owner, @constraint, @lifecycle"),
                    ))
                }
            };
            Ok(Annotation { kind, value })
        })
}

/// Parse a dependency: `needs vpc.main -> vpc_id`
fn dependency() -> impl Parser<char, Dependency, Error = Simple<char>> {
    text::keyword("needs")
        .padded()
        .ignore_then(resource_ref())
        .padded()
        .then_ignore(just("->").padded())
        .then(ident())
        .map(|(source, binding)| Dependency { source, binding })
}

/// Parse a semantic section: `identity { name = "foo" }`
fn section() -> impl Parser<char, Section, Error = Simple<char>> {
    ident()
        .padded()
        .then(
            field()
                .padded()
                .repeated()
                .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|(name, fields)| Section { name, fields })
}

/// Parse a field: `name = value`
fn field() -> impl Parser<char, Field, Error = Simple<char>> {
    ident()
        .padded()
        .then_ignore(just('=').padded())
        .then(value())
        .map(|(name, value)| Field { name, value })
}

/// Parse a value.
fn value() -> impl Parser<char, Value, Error = Simple<char>> {
    recursive(|val| {
        let string_val = string_literal().map(Value::String);

        let number_val = {
            let negative = just('-').or_not();
            let integer_part = text::int(10);
            let decimal_part = just('.').then(text::digits(10));

            negative
                .then(integer_part)
                .then(decimal_part.or_not())
                .try_map(|((neg, int_str), dec), span| {
                    let neg_str = if neg.is_some() { "-" } else { "" };
                    if let Some((_, frac)) = dec {
                        let full = format!("{neg_str}{int_str}.{frac}");
                        full.parse::<f64>()
                            .map(Value::Number)
                            .map_err(|e| Simple::custom(span, format!("invalid number: {e}")))
                    } else {
                        let full = format!("{neg_str}{int_str}");
                        full.parse::<i64>()
                            .map(Value::Integer)
                            .map_err(|e| Simple::custom(span, format!("invalid integer: {e}")))
                    }
                })
        };

        let bool_val = text::keyword("true")
            .to(Value::Bool(true))
            .or(text::keyword("false").to(Value::Bool(false)));

        let ref_val = text::keyword("ref")
            .padded()
            .ignore_then(resource_ref().delimited_by(just('(').padded(), just(')').padded()))
            .map(Value::Ref);

        let array_val = val
            .clone()
            .padded()
            .separated_by(just(',').padded())
            .allow_trailing()
            .delimited_by(just('[').padded(), just(']').padded())
            .map(Value::Array);

        let record_val = ident()
            .padded()
            .then_ignore(just('=').padded())
            .then(val)
            .map(|(name, value)| Field { name, value })
            .padded()
            .separated_by(just(',').padded())
            .allow_trailing()
            .delimited_by(just('{').padded(), just('}').padded())
            .map(Value::Record);

        // Order matters: try ref before bool (both start with alphabetic chars),
        // try number before negative sign is consumed as ident
        ref_val
            .or(bool_val)
            .or(string_val)
            .or(number_val)
            .or(array_val)
            .or(record_val)
    })
}

/// Parse a dot-separated type path: aws.ec2.Vpc
fn type_path() -> impl Parser<char, TypePath, Error = Simple<char>> {
    ident()
        .separated_by(just('.'))
        .at_least(1)
        .map(|segments| TypePath { segments })
}

/// Parse a dot-separated resource reference: network.vpc.main
fn resource_ref() -> impl Parser<char, ResourceRef, Error = Simple<char>> {
    ident()
        .separated_by(just('.'))
        .at_least(1)
        .map(|segments| ResourceRef { segments })
}

/// Parse an identifier: alphanumeric + underscore, starting with alpha/underscore.
fn ident() -> impl Parser<char, String, Error = Simple<char>> {
    filter(|c: &char| c.is_alphabetic() || *c == '_')
        .then(
            filter(|c: &char| c.is_alphanumeric() || *c == '_')
                .repeated()
                .collect::<String>(),
        )
        .map(|(first, rest)| format!("{first}{rest}"))
}

/// Strip leading newline, trailing whitespace, and common indentation from a triple-quoted string.
fn dedent(s: &str) -> String {
    let s = s.strip_prefix('\n').unwrap_or(s);
    let s = s.trim_end();

    let min_indent = s
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);

    s.lines()
        .map(|line| {
            if line.trim().is_empty() {
                ""
            } else if line.len() >= min_indent {
                &line[min_indent..]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse a double-quoted string literal (single or triple-quoted) with basic escape support.
fn string_literal() -> impl Parser<char, String, Error = Simple<char>> {
    // Triple-quoted raw strings: """content"""
    // Uses negative lookahead: consume one char at a time as long as we're NOT at """
    let triple_quoted = just("\"\"\"")
        .ignore_then(just("\"\"\"").not().repeated().collect::<String>())
        .then_ignore(just("\"\"\""))
        .map(|s| dedent(&s));

    // Single-quoted strings with escape support
    let escape = just('\\').ignore_then(choice((
        just('\\').to('\\'),
        just('"').to('"'),
        just('n').to('\n'),
        just('t').to('\t'),
    )));
    let string_char = none_of("\\\"").or(escape);
    let single_quoted = string_char
        .repeated()
        .collect::<String>()
        .delimited_by(just('"'), just('"'));

    // Try triple-quoted first (both start with ")
    triple_quoted.or(single_quoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_string_literal() {
        let result = string_literal().parse("\"hello world\"");
        assert_eq!(result, Ok("hello world".to_string()));
    }

    #[test]
    fn parse_string_with_escapes() {
        let result = string_literal().parse("\"hello\\nworld\"");
        assert_eq!(result, Ok("hello\nworld".to_string()));
    }

    #[test]
    fn parse_triple_quoted_string() {
        let input = "\"\"\"hello world\"\"\"";
        let result = string_literal().parse(input);
        assert_eq!(result, Ok("hello world".to_string()));
    }

    #[test]
    fn parse_triple_quoted_multiline() {
        let input = "\"\"\"\n            line one\n            line two\n            \"\"\"";
        let result = string_literal().parse(input);
        assert_eq!(result, Ok("line one\nline two".to_string()));
    }

    #[test]
    fn parse_triple_quoted_preserves_relative_indent() {
        let input = "\"\"\"\n            {\n              \"key\": \"value\"\n            }\n            \"\"\"";
        let result = string_literal().parse(input);
        assert_eq!(result, Ok("{\n  \"key\": \"value\"\n}".to_string()));
    }

    #[test]
    fn parse_triple_quoted_in_resource() {
        let input = r#"resource role "lambda" : aws.iam.Role {
            security {
                assume_role_policy = """
                    {
                      "Version": "2012-10-17",
                      "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "lambda.amazonaws.com"},
                        "Action": "sts:AssumeRole"
                      }]
                    }
                    """
            }
        }"#;

        let parsed = parse(input).unwrap();
        let resource = match &parsed.declarations[0] {
            Declaration::Resource(r) => r,
            _ => panic!("expected resource"),
        };
        let policy_field = &resource.sections[0].fields[0];
        assert_eq!(policy_field.name, "assume_role_policy");
        if let Value::String(s) = &policy_field.value {
            assert!(s.contains("\"Version\": \"2012-10-17\""));
            assert!(s.contains("sts:AssumeRole"));
            // Should be dedented — no leading whitespace on first line
            assert!(s.starts_with('{'));
        } else {
            panic!("expected string value");
        }
    }

    #[test]
    fn parse_ident() {
        assert_eq!(ident().parse("foo_bar"), Ok("foo_bar".to_string()));
        assert_eq!(ident().parse("vpc"), Ok("vpc".to_string()));
        assert!(ident().parse("123bad").is_err());
    }

    #[test]
    fn parse_type_path() {
        let result = type_path().parse("aws.ec2.Vpc");
        assert!(result.is_ok());
        let tp = result.unwrap();
        assert_eq!(tp.segments, vec!["aws", "ec2", "Vpc"]);
    }

    #[test]
    fn parse_annotation() {
        let result = annotation().parse("@intent \"Primary VPC\"");
        assert!(result.is_ok());
        let ann = result.unwrap();
        assert_eq!(ann.kind, AnnotationKind::Intent);
        assert_eq!(ann.value, "Primary VPC");
    }

    #[test]
    fn parse_unknown_annotation() {
        let result = annotation().parse("@foobar \"value\"");
        assert!(result.is_err());
    }

    #[test]
    fn parse_dependency() {
        let result = dependency().parse("needs vpc.main -> vpc_id");
        assert!(result.is_ok());
        let dep = result.unwrap();
        assert_eq!(dep.source.segments, vec!["vpc", "main"]);
        assert_eq!(dep.binding, "vpc_id");
    }

    #[test]
    fn parse_simple_field() {
        let result = field().parse("name = \"hello\"");
        assert!(result.is_ok());
        let f = result.unwrap();
        assert_eq!(f.name, "name");
        assert!(matches!(f.value, Value::String(ref s) if s == "hello"));
    }

    #[test]
    fn parse_bool_field() {
        let result = field().parse("enabled = true");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap().value, Value::Bool(true)));
    }

    #[test]
    fn parse_integer_field() {
        let result = field().parse("port = 8080");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap().value, Value::Integer(8080)));
    }

    #[test]
    fn parse_number_field() {
        let result = field().parse("ratio = 3.14");
        assert!(result.is_ok());
        match result.unwrap().value {
            Value::Number(n) => assert!((n - 3.14).abs() < f64::EPSILON),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn parse_ref_field() {
        let result = field().parse("vpc_id = ref(network.vpc.main)");
        assert!(result.is_ok());
        match result.unwrap().value {
            Value::Ref(r) => assert_eq!(r.segments, vec!["network", "vpc", "main"]),
            other => panic!("expected Ref, got {other:?}"),
        }
    }

    #[test]
    fn parse_array_field() {
        let result = field().parse("ports = [80, 443, 8080]");
        assert!(result.is_ok());
        match result.unwrap().value {
            Value::Array(items) => assert_eq!(items.len(), 3),
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn parse_record_field() {
        let result = field().parse("tags = { env = \"prod\", team = \"platform\" }");
        assert!(result.is_ok());
        match result.unwrap().value {
            Value::Record(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "env");
                assert_eq!(fields[1].name, "team");
            }
            other => panic!("expected Record, got {other:?}"),
        }
    }

    #[test]
    fn parse_section() {
        let input = r#"network {
            cidr_block = "10.0.0.0/16"
            dns_support = true
        }"#;
        let result = section().parse(input);
        assert!(result.is_ok());
        let s = result.unwrap();
        assert_eq!(s.name, "network");
        assert_eq!(s.fields.len(), 2);
    }

    #[test]
    fn parse_minimal_resource() {
        let input = r#"resource vpc "main" : aws.ec2.Vpc {
            @intent "Test VPC"
            network {
                cidr_block = "10.0.0.0/16"
            }
        }"#;
        let result = resource_decl().parse(input);
        assert!(result.is_ok(), "parse error: {:?}", result.err());
        let r = result.unwrap();
        assert_eq!(r.kind, "vpc");
        assert_eq!(r.name, "main");
        assert_eq!(r.type_path.to_string(), "aws.ec2.Vpc");
        assert_eq!(r.annotations.len(), 1);
        assert_eq!(r.sections.len(), 1);
    }

    #[test]
    fn parse_resource_without_kind() {
        // kind omitted — derived from type path's last segment (lowercased)
        let input = r#"resource "main" : aws.ec2.Vpc {
            @intent "Test VPC"
            network { cidr_block = "10.0.0.0/16" }
        }"#;
        let result = resource_decl().parse(input);
        assert!(result.is_ok(), "parse error: {:?}", result.err());
        let r = result.unwrap();
        assert_eq!(r.kind, "vpc");
        assert_eq!(r.name, "main");
        assert_eq!(r.type_path.to_string(), "aws.ec2.Vpc");
    }

    #[test]
    fn parse_resource_without_kind_securitygroup() {
        // Multi-word type → lowercased
        let input = r#"resource "web-sg" : aws.ec2.SecurityGroup {
            identity { name = "web-sg" }
        }"#;
        let result = resource_decl().parse(input);
        assert!(result.is_ok(), "parse error: {:?}", result.err());
        let r = result.unwrap();
        assert_eq!(r.kind, "securitygroup");
        assert_eq!(r.name, "web-sg");
    }

    #[test]
    fn parse_resource_with_dependency() {
        let input = r#"resource subnet "pub" : aws.ec2.Subnet {
            @intent "Public subnet"
            needs vpc.main -> vpc_id
            network {
                cidr_block = "10.0.1.0/24"
            }
        }"#;
        let result = resource_decl().parse(input);
        assert!(result.is_ok(), "parse error: {:?}", result.err());
        let r = result.unwrap();
        assert_eq!(r.dependencies.len(), 1);
        assert_eq!(r.dependencies[0].binding, "vpc_id");
    }

    #[test]
    fn parse_full_file() {
        let input = r#"
            resource vpc "main" : aws.ec2.Vpc {
                @intent "Primary VPC"
                network {
                    cidr_block = "10.0.0.0/16"
                }
            }

            resource subnet "pub" : aws.ec2.Subnet {
                @intent "Public subnet"
                needs vpc.main -> vpc_id
                network {
                    cidr_block = "10.0.1.0/24"
                }
            }
        "#;
        let result = parse(input);
        assert!(result.is_ok(), "parse error: {:?}", result.err());
        let file = result.unwrap();
        assert_eq!(file.declarations.len(), 2);
    }

    #[test]
    fn parse_file_with_comments() {
        let input = r#"
            # This is a VPC definition
            resource vpc "main" : aws.ec2.Vpc {
                @intent "Primary VPC"
                # Network configuration
                network {
                    cidr_block = "10.0.0.0/16"
                }
            }
        "#;
        let result = parse(input);
        assert!(result.is_ok(), "parse error: {:?}", result.err());
        let file = result.unwrap();
        assert_eq!(file.declarations.len(), 1);
    }

    #[test]
    fn parse_layer() {
        let input = r#"
            layer "staging" over "base" {
                @intent "Staging environment overrides"
                override compute.* {
                    sizing {
                        instance_type = "t3.small"
                    }
                }
            }
        "#;
        let result = parse(input);
        assert!(result.is_ok(), "parse error: {:?}", result.err());
        let file = result.unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0] {
            Declaration::Layer(l) => {
                assert_eq!(l.name, "staging");
                assert_eq!(l.base, "base");
                assert_eq!(l.overrides.len(), 1);
                assert_eq!(l.overrides[0].pattern, "compute.*");
            }
            _ => panic!("expected Layer"),
        }
    }
}
