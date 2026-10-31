//! Headless Ayatsuri2D: a layered drawing in, a rigged moc3 bundle out.
//!
//!     ayatsuri-cli roles <art.psd|art.kra|art.clip>
//!     ayatsuri-cli rig   <art.psd|art.kra|art.clip> <out-dir> [--name NAME] [--no-mesh]
//!
//! `roles` prints what each layer was taken for, to check the layer names
//! before rigging. `rig` meshes every layer, builds the standard rig over
//! them (see rig.rs) and writes NAME.moc3, NAME.model3.json,
//! NAME.physics3.json and NAME.textures/ into out-dir.

#![forbid(unsafe_code)]

mod rig;

use std::path::Path;

use serde_json::json;

const USAGE: &str = "usage:
  ayatsuri-cli roles <art.psd|art.kra|art.clip>
  ayatsuri-cli rig   <art.psd|art.kra|art.clip> <out-dir> [--name NAME] [--no-mesh]";

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let option = |f: &str| args.iter().skip_while(|a| *a != f).nth(1).cloned();
    let mut plain = args.iter().filter(|a| !a.starts_with("--"));
    // --name's value is not a positional argument
    let named = option("--name");
    let mut next = || plain.find(|a| Some(*a) != named.as_ref()).cloned();
    let (cmd, input) = (next().ok_or(USAGE)?, next().ok_or(USAGE)?);

    let path = Path::new(&input);
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| format!("{input}: {e}"))?;
    let (doc, _) =
        ayatsuri_import::read_document(&bytes, &ext).map_err(|e| format!("{input}: {e}"))?;
    let imported = ayatsuri_import::from_layers_atlas(&doc).map_err(|e| format!("{input}: {e}"))?;
    let mut model = imported.model;
    let roles = rig::roles(&model, &imported.mesh_names, &imported.part_names);
    let parts = rig::parts(&model, &roles, &imported.mesh_names, &imported.part_names);
    for (i, (role, part)) in roles.iter().zip(&parts).enumerate() {
        let name = imported.mesh_names.get(i).map_or("?", String::as_str);
        println!("{:<11} {:<11} {name}", role.name(), part.name());
    }
    match cmd.as_str() {
        "roles" => return Ok(()),
        "rig" => {}
        _ => return Err(USAGE.into()),
    }
    let out = next().ok_or(USAGE)?;

    if !flag("--no-mesh") {
        let pages: Vec<image::RgbaImage> = imported
            .textures
            .iter()
            .filter_map(|p| image::load_from_memory(p).ok().map(|i| i.to_rgba8()))
            .collect();
        let n = rig::automesh(&mut model, &pages);
        println!("meshed {n} of {} layers", model.art_meshes.len());
    }
    rig::rig(&mut model, &roles, &parts)?;
    println!(
        "{} parameters, {} deformers, {} physics groups",
        model.parameters.len(),
        model.deformers.len(),
        model.physics.as_ref().map_or(0, |p| p.groups.len())
    );

    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = safe(&named.unwrap_or(stem));
    let written = write_bundle(Path::new(&out), &name, &model, &imported.textures)?;
    for w in written {
        println!("wrote {w}");
    }
    Ok(())
}

/// A file name: letters, digits, '-' and '_' kept, the rest '_'.
fn safe(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() { "model".into() } else { s }
}

/// The moc3 bundle, laid out as the editor's export lays it out.
fn write_bundle(
    dir: &Path,
    name: &str,
    model: &ayatsuri_format::model::Model,
    textures: &[Vec<u8>],
) -> Result<Vec<String>, String> {
    let version = ayatsuri_import::ExportVersion::DEFAULT;
    for w in ayatsuri_import::export_warnings(model, version) {
        eprintln!("warning: {w}");
    }
    let mut written = Vec::new();
    let mut write = |rel: &str, bytes: &[u8]| -> Result<(), String> {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
        written.push(p.display().to_string());
        Ok(())
    };

    let moc = ayatsuri_import::to_moc3(model, version).map_err(|e| e.to_string())?;
    write(&format!("{name}.moc3"), &moc)?;
    let mut tex_refs = Vec::new();
    for (i, png) in textures.iter().enumerate() {
        let rel = format!("{name}.textures/texture_{i:02}.png");
        write(&rel, png)?;
        tex_refs.push(rel);
    }
    let mut refs = json!({ "Moc": format!("{name}.moc3"), "Textures": tex_refs });
    if let Some(rig) = &model.physics {
        let rel = format!("{name}.physics3.json");
        write(&rel, &ayatsuri_import::to_physics3(rig))?;
        if let Some(r) = refs.as_object_mut() {
            r.insert("Physics".into(), json!(rel));
        }
    }
    // apps drive auto blink and lip sync off these two groups
    let model3 = json!({
        "Version": 3,
        "FileReferences": refs,
        "Groups": [
            { "Target": "Parameter", "Name": "EyeBlink", "Ids": ["ParamEyeLOpen", "ParamEyeROpen"] },
            { "Target": "Parameter", "Name": "LipSync", "Ids": ["ParamMouthOpenY"] },
        ],
    });
    write(&format!("{name}.model3.json"), &ayatsuri_import::cubism_json(&model3))?;
    Ok(written)
}
