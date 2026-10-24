//! Pin the shared parser and reject interpreter features that would embed backend assets.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

const REV: &str = "9f75d3dc19b4b514c4f3d4847ed1ec1b0e86b4ef";
const PACKAGES: &[(&str, &str)] = &[("craft-pdf", "0.1.0"), ("hayro-syntax", "0.7.2"), ("hayro-jbig2", "0.3.0")];

pub fn run(root: &Path) -> Result<(), String> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args(["metadata", "--locked", "--format-version", "1"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&output.stderr)));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    validate(&metadata)?;
    println!("PDF backend: pinned parser only; no Hayro interpreter, rasterizer or bundled CMaps");
    Ok(())
}

fn validate(metadata: &Value) -> Result<(), String> {
    let packages = metadata.get("packages").and_then(Value::as_array).ok_or("cargo metadata has no packages")?;
    let nodes = metadata.pointer("/resolve/nodes").and_then(Value::as_array).ok_or("cargo metadata has no resolved nodes")?;
    let source = format!("git+https://github.com/storytold/craft-libs?rev={REV}#{REV}");
    for (name, version) in PACKAGES {
        let matches: Vec<_> = packages.iter().filter(|p| p["name"] == *name).collect();
        if matches.len() != 1 || matches[0]["version"] != *version || matches[0]["source"] != source {
            return Err(format!("{name}: expected exactly {version} from {source}"));
        }
    }
    for (name, allowed) in [("craft-pdf", "read"), ("hayro-syntax", "std")] {
        let package = packages.iter().find(|p| p["name"] == name).ok_or_else(|| format!("missing {name}"))?;
        let node = nodes.iter().find(|n| n["id"] == package["id"]).ok_or_else(|| format!("{name}: missing resolved node"))?;
        let features = node["features"].as_array().ok_or_else(|| format!("{name}: missing resolved features"))?;
        if features.len() != 1 || features.first().and_then(Value::as_str) != Some(allowed) {
            return Err(format!("{name}: expected only the {allowed} feature"));
        }
    }
    for package in packages {
        let name = package["name"].as_str().unwrap_or("");
        let active = nodes.iter().any(|node| node["id"] == package["id"]);
        if active && matches!(name, "hayro" | "hayro-interpret" | "hayro-cmap" | "krilla") {
            return Err(format!("{name}: EffectCraft must use craft-pdf with only the read feature"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata() -> Value {
        let packages: Vec<_> = PACKAGES.iter().map(|(name, version)| {
            json!({"id": name, "name": name, "version": version, "source": format!("git+https://github.com/storytold/craft-libs?rev={REV}#{REV}")})
        }).collect();
        json!({"packages": packages, "resolve": {"nodes": [
            {"id": "hayro-syntax", "features": ["std"]}, {"id": "craft-pdf", "features": ["read"]}
        ]}})
    }

    #[test]
    fn accepts_the_pinned_parser_only_graph() {
        assert!(validate(&metadata()).is_ok());
    }

    #[test]
    fn rejects_unpinned_duplicate_and_missing_parsers() {
        let mut m = metadata();
        m["packages"][0]["source"] = json!("registry");
        assert!(validate(&m).is_err());
        let mut m = metadata();
        let duplicate = m["packages"][0].clone();
        m["packages"].as_array_mut().unwrap().push(duplicate);
        assert!(validate(&m).is_err());
        m["packages"].as_array_mut().unwrap().retain(|p| p["name"] != "hayro-syntax");
        assert!(validate(&m).is_err());
    }

    #[test]
    fn rejects_shared_reader_source_drift() {
        let mut m = metadata();
        m["packages"][0]["source"] = Value::Null;
        assert!(validate(&m).is_err());
    }

    #[test]
    fn rejects_extra_features_and_missing_active_nodes() {
        for (index, feature) in [(0, "images"), (1, "write"), (1, "raster")] {
            let mut m = metadata();
            m["resolve"]["nodes"][index]["features"].as_array_mut().unwrap().push(json!(feature));
            assert!(validate(&m).is_err());
        }
        let mut m = metadata();
        m["resolve"]["nodes"].as_array_mut().unwrap().pop();
        assert!(validate(&m).is_err());
    }

    #[test]
    fn rejects_backend_asset_features_and_missing_resolution() {
        for name in ["hayro", "hayro-interpret", "hayro-cmap", "krilla"] {
            let mut m = metadata();
            m["packages"].as_array_mut().unwrap().push(json!({"id": name, "name": name}));
            m["resolve"]["nodes"].as_array_mut().unwrap().push(json!({"id": name}));
            assert!(validate(&m).is_err());
        }
        let mut m = metadata();
        m["resolve"] = Value::Null;
        assert!(validate(&m).is_err());
    }
}
