//! Standalone After Effects Project (.aep / .aepx) Decompiler & Inspector
//!
//! Decompiles and inspects any Adobe After Effects project (from CS6 / CC 2017 to CC 2026).
//!
//! Usage:
//!   cargo run -p effectcraft-engine --example aep_decompile -j 2 -- "path/to/project.aep"
//!   cargo run -p effectcraft-engine --example aep_decompile -j 2 -- "path/to/project.aep" --json
//!   cargo run -p effectcraft-engine --example aep_decompile -j 2 -- "path/to/project.aep" --chunks

use std::env;
use std::fs;
use std::path::Path;
use effectcraft_engine::aep::{self, ChunkNode};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: aep_decompile <path-to-aep-or-aepx> [--json] [--chunks]");
        std::process::exit(1);
    }

    let mut path = None;
    let mut as_json = false;
    let mut show_chunks = false;

    for arg in &args {
        if arg == "--json" {
            as_json = true;
        } else if arg == "--chunks" {
            show_chunks = true;
        } else if path.is_none() {
            path = Some(arg.as_str());
        }
    }

    let Some(file_path) = path else {
        eprintln!("Error: Missing file path");
        std::process::exit(1);
    };

    let p = Path::new(file_path);
    if !p.exists() {
        eprintln!("Error: File does not exist: {file_path}");
        std::process::exit(1);
    }

    let bytes = match fs::read(p) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading file: {e}");
            std::process::exit(1);
        }
    };

    println!("[aep_decompile] Inspecting: {}", p.display());
    println!("[aep_decompile] File size: {} bytes ({:.2} MB)", bytes.len(), bytes.len() as f64 / 1_048_576.0);

    let is_binary = aep::is_aep(&bytes);
    let is_xml = aep::is_aepx(&bytes);

    if !is_binary && !is_xml {
        eprintln!("Error: Not a recognized After Effects project (neither RIFX Egg! nor AEPX XML)");
        std::process::exit(1);
    }

    println!("[aep_decompile] Format: {}", if is_binary { "Binary RIFX (Egg!)" } else { "XML (.aepx)" });

    // Decode project
    match aep::from_aep_or_aepx_with_report(&bytes, Some(p)) {
        Ok((project, report)) => {
            if as_json {
                let report_json = serde_json::to_string_pretty(&report).unwrap_or_default();
                println!("{}", report_json);
            } else {
                println!("\n=== PROJECT REPORT ===");
                println!("Compositions: {}", report.comp_count);
                println!("Total Footage Items: {}", report.total_footage_count);
                println!("Missing Footage Items: {}", report.missing_footage_count);

                if !report.missing_footage.is_empty() {
                    println!("\nMissing Assets ({}):", report.missing_footage.len());
                    for m in &report.missing_footage {
                        println!("  - {}", m);
                    }
                }

                if !report.fonts.is_empty() {
                    println!("\nFonts Detected ({}):", report.fonts.len());
                    for f in &report.fonts {
                        println!("  - {}", f);
                    }
                }

                if !report.plugins.is_empty() {
                    println!("\nEffects & Plugins ({}):", report.plugins.len());
                    for pl in &report.plugins {
                        println!("  - {}", pl);
                    }
                }

                if !report.custom_plugins.is_empty() {
                    println!("\nCustom / Third-Party Plugins ({}):", report.custom_plugins.len());
                    for cpl in &report.custom_plugins {
                        println!("  * {}", cpl);
                    }
                }

                println!("\n=== COMPOSITIONS & LAYERS ===");
                for (comp_id, comp) in project.comps() {
                    let comp_name = project
                        .items
                        .get(comp_id)
                        .map(|it| it.name.as_str())
                        .unwrap_or("Untitled Comp");
                    println!(
                        "• Comp '{}' ({}x{} @ {:.2} fps, duration: {:.2}s, {} layers)",
                        comp_name,
                        comp.width,
                        comp.height,
                        comp.frame_rate.as_f64(),
                        comp.duration.seconds(),
                        comp.layers.len()
                    );
                    for (i, layer) in comp.layers.iter().enumerate() {
                        let kind_str = if layer.switches.adjustment {
                            "Adjustment"
                        } else {
                            match &layer.source {
                                effectcraft_project::LayerSource::Comp { .. } => "Precomp",
                                effectcraft_project::LayerSource::Footage { .. } => "Footage",
                                effectcraft_project::LayerSource::Solid { .. } => "Solid",
                                effectcraft_project::LayerSource::Camera => "Camera",
                                effectcraft_project::LayerSource::Text => "Text",
                                effectcraft_project::LayerSource::Shape => "Shape",
                                effectcraft_project::LayerSource::Null => "Null",
                                effectcraft_project::LayerSource::Light { .. } => "Light",
                                effectcraft_project::LayerSource::Model { .. } => "Model",
                                effectcraft_project::LayerSource::Primitive { .. } => "Primitive",
                            }
                        };
                        println!(
                            "   [{:02}] {} ({}) in: {:.2}s, out: {:.2}s, 3D: {}",
                            i + 1,
                            layer.name,
                            kind_str,
                            layer.in_point.seconds(),
                            layer.out_point.seconds(),
                            layer.switches.three_d
                        );
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("[aep_decompile] Decoding error: {e}");
        }
    }

    if show_chunks && is_binary {
        println!("\n=== CHUNK HIERARCHY ===");
        if let Ok(tree) = aep::parse_rifx(&bytes) {
            print_chunk_tree(&tree, 0);
        }
    }
}

fn print_chunk_tree(nodes: &[ChunkNode<'_>], depth: usize) {
    if depth > 10 {
        return;
    }
    let indent = "  ".repeat(depth);
    for node in nodes {
        match node {
            ChunkNode::Chunk { id, data } => {
                let tag = String::from_utf8_lossy(id);
                println!("{indent}- Chunk [{}] ({} bytes)", tag, data.len());
            }
            ChunkNode::List { id, list_type, children } => {
                let tag = String::from_utf8_lossy(id);
                let ltype = String::from_utf8_lossy(list_type);
                println!("{indent}+ {} [{}] ({} children)", tag, ltype, children.len());
                print_chunk_tree(children, depth + 1);
            }
        }
    }
}
