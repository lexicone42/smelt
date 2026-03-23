#![no_main]
use libfuzzer_sys::fuzz_target;

/// Fuzz the dependency graph builder with parsed AST.
///
/// This exercises the most complex transformation pipeline:
/// parse → expand components → expand for_each → expand count → build DAG.
/// Graph construction should never panic — it should return Ok or GraphError.
fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };

    let Ok(file) = smelt::parser::parse(source) else {
        return;
    };

    // Build the graph — this runs all expansion passes + cycle detection
    match smelt::graph::DependencyGraph::build(&[file]) {
        Ok(graph) => {
            // Exercise graph queries on successfully built graphs
            let resources = graph.resources();
            for res in &resources {
                let _ = graph.dependents(&res.id);
                let _ = graph.dependencies(&res.id);
                let _ = graph.blast_radius(&res.id);
            }
            let _ = graph.tiered_apply_order();
            let _ = graph.tiered_destroy_order();
            let _ = graph.to_dot();
        }
        Err(_) => {} // Errors are expected (cycles, missing deps, etc.)
    }
});
