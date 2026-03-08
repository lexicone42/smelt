# SDK-to-Smelt Codegen Strategy

## Problem
GCP has hundreds of resource types across 30+ services. Each SDK crate is
auto-generated from Google's API specs and updates frequently. Hand-writing
smelt provider code for each resource type doesn't scale.

## Observation
Every smelt resource type follows an identical pattern:

```
Schema  → field names, types, required/optional, sections
Create  → config JSON → SDK model builder → API call → read back
Read    → API call → extract fields from SDK model → state JSON
Update  → config JSON → SDK model builder → API call → read back
Delete  → API call
Diff    → forces_replacement rules per field
```

The GCP SDK crates are themselves auto-generated from proto/API specs.
We're hand-writing a bridge between two generated artifacts.

## Approach: Declarative Resource Definitions

Instead of writing Rust code for each resource type, define resources
declaratively in a `.toml` or `.smelt` manifest:

```toml
[resource]
type_path = "compute.Network"
description = "Google VPC Network"
sdk_crate = "google-cloud-compute-v1"
sdk_model = "Network"
sdk_client = "Networks"
provider_id_format = "{name}"

[resource.crud]
create = "insert"      # client method name
read = "get"
update = "patch"
delete = "delete"

[resource.fields.name]
section = "identity"
sdk_field = "name"
type = "String"
required = true

[resource.fields.auto_create_subnetworks]
section = "network"
sdk_field = "auto_create_subnetworks"
type = "Bool"
default = true

[resource.fields.routing_mode]
section = "network"
sdk_field = "routing_config.routing_mode"  # nested access
type = "Enum"
variants = ["REGIONAL", "GLOBAL"]
default = "REGIONAL"

[resource.replacement_fields]
# Fields that force resource recreation when changed
fields = ["name"]
```

A build-time codegen tool reads these manifests and generates:
1. The `*_schema()` function
2. The `create_*()`, `read_*()`, `update_*()`, `delete_*()` functions
3. The `forces_replacement` rules
4. The dispatch entries in mod.rs

## Advantages
- **Adding a resource type = writing ~20 lines of TOML**, not ~100 lines of Rust
- **SDK updates**: When field types change, update the manifest, regenerate
- **Consistency**: All generated code follows the same patterns
- **Reviewability**: The manifest is the source of truth, not the generated code

## Bootstrap Strategy
1. Write a `smelt-codegen` tool that reads manifests → outputs Rust
2. Convert our existing 27 resource types to manifests (extract from code)
3. Add new resource types by writing manifests only
4. Run codegen as a build step or pre-commit hook

## SDK Introspection Alternative
Even more automated: the codegen tool could introspect the SDK crate source
directly:
- Parse `model.rs` to discover struct fields and their types
- Parse `client.rs` to discover available CRUD methods
- Generate a *draft* manifest with all fields, letting the developer only
  specify section grouping and replacement rules

This means adding a new resource type becomes:
```
smelt-codegen introspect google-cloud-compute-v1 Network > resources/compute.Network.toml
# Edit: group fields into sections, mark replacement fields
smelt-codegen generate resources/compute.Network.toml > src/provider/gcp/compute_network.rs
```

## Phase Plan
1. **Now**: Define the manifest format, convert 2-3 resources as proof of concept
2. **Next**: Build the codegen tool, convert all 27 existing resources
3. **Then**: Build the introspection tool, use it to add remaining resources
4. **Ongoing**: When SDK updates, re-introspect + regenerate
