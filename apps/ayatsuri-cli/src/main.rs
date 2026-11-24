//! Headless Ayatsuri2D: a layered drawing in, a rigged moc3 bundle out.
//!
//!     ayatsuri-cli roles <art.psd|art.kra|art.clip>
//!     ayatsuri-cli rig   <art.psd|art.kra|art.clip> <out-dir> [--name NAME] [--no-mesh]
//!
//!     ayatsuri-cli bust  <model.model3.json|model.moc3> <out.moc3> [--mesh ID]
//!                        [--param ID] [--center U,V] [--radius RU,RV]
//!                        [--ellipse U,V,RU,RV[,W] [--param ID] [--dir DX,DY]
//!                        [--anchor U,V]]... [--mask PNG]
//!                        [--mask-param ID] [--pivot center|bottom|top]
//!                        [--lift F] [--swell F] [--rows N] [--cols N] [--dry]
//!
//! `roles` prints what each layer was taken for, to check the layer names
//! before rigging. `rig` meshes every layer, builds the standard rig over
//! them (see rig.rs) and writes NAME.moc3, NAME.model3.json,
//! NAME.physics3.json and NAME.textures/ into out-dir. `bust` wraps one
//! mesh of a finished moc3 in a warp keyed to lift and swell the chest
//! (rig::bust) and writes only the new moc3; `--dry` writes it unchanged,
//! to check what the round trip keeps.

#![forbid(unsafe_code)]

mod rig;

use std::path::Path;

use serde_json::json;

const USAGE: &str = "usage:
  ayatsuri-cli roles <art.psd|art.kra|art.clip>
  ayatsuri-cli rig   <art.psd|art.kra|art.clip> <out-dir> [--name NAME] [--no-mesh]
  ayatsuri-cli bust  <model.model3.json|model.moc3> <out.moc3> [--mesh ArtMeshTopwear]
                     [--param ParamBustY] [--center 0.5,0.4] [--radius 0.35,0.2]
                     [--ellipse U,V,RU,RV[,W] [--param ID] [--dir DX,DY] [--anchor U,V]]...
                     [--mask grey.png]
                     [--mask-param ID] [--pivot center|bottom|top]
                     [--lift 0.03] [--swell 0.05] [--rows 8] [--cols 8] [--dry]";

/// The options that take a value; the rest are flags.
const VALUED: &[&str] = &[
    "--name",
    "--mesh",
    "--param",
    "--center",
    "--radius",
    "--lift",
    "--swell",
    "--rows",
    "--cols",
    "--ellipse",
    "--mask",
    "--mask-param",
    "--pivot",
    "--dir",
    "--anchor",
];

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
    if flag("--help") || flag("-h") {
        println!("{USAGE}");
        return Ok(());
    }
    // an option's value is not a positional argument
    let mut plain = args
        .iter()
        .enumerate()
        .filter(|&(i, a)| {
            !a.starts_with("--")
                && !i
                    .checked_sub(1)
                    .and_then(|j| args.get(j))
                    .is_some_and(|o| VALUED.contains(&o.as_str()))
        })
        .map(|(_, a)| a);
    let named = option("--name");
    let mut next = || plain.next().cloned();
    let (cmd, input) = (next().ok_or(USAGE)?, next().ok_or(USAGE)?);
    if cmd == "bust" {
        let out = next().ok_or(USAGE)?;
        return bust(&input, &out, &args, &option, flag("--dry"));
    }

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

/// `bust`: read the moc3 a model3.json names (or a moc3), shape its chest
/// unless `dry`, and write the moc3 to `out`.
fn bust(
    input: &str,
    out: &str,
    args: &[String],
    option: &dyn Fn(&str) -> Option<String>,
    dry: bool,
) -> Result<(), String> {
    let at = |e: std::io::Error| format!("{input}: {e}");
    let path = Path::new(input);
    let read = std::fs::read(path).map_err(at)?;
    // a moc3 by its magic, whatever its name ends in
    let (bytes, moc_path) = if read.starts_with(b"MOC3") {
        (read, path.to_path_buf())
    } else {
        let json: serde_json::Value =
            serde_json::from_slice(&read).map_err(|e| format!("{input}: {e}"))?;
        let moc = json
            .pointer("/FileReferences/Moc")
            .and_then(|m| m.as_str())
            .ok_or(format!("{input}: no FileReferences.Moc"))?;
        let p = path.parent().unwrap_or(Path::new("")).join(moc);
        (std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?, p)
    };
    let mut model =
        ayatsuri_import::from_moc3(&bytes).map_err(|e| format!("{}: {e}", moc_path.display()))?;
    let float =
        |f: &str, s: &str| s.trim().parse::<f32>().map_err(|_| format!("{f}: not a number: {s}"));
    let number = |f: &str, d: f32| option(f).map_or(Ok(d), |s| float(f, &s));
    let floats =
        |f: &str, s: &str| s.split(',').map(|x| float(f, x)).collect::<Result<Vec<f32>, _>>();
    let pair = |f: &str, d: [f32; 2]| -> Result<[f32; 2], String> {
        let Some(s) = option(f) else { return Ok(d) };
        match floats(f, &s)?.as_slice() {
            &[a, b] => Ok([a, b]),
            _ => Err(format!("{f}: want A,B, got {s}")),
        }
    };

    // each --ellipse takes the --param right after it; one --param alone
    // moves them all
    let mut ellipses: Vec<rig::Ellipse> = Vec::new();
    let mut own: Vec<Option<String>> = Vec::new();
    let mut params = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ellipse" => {
                let s = it.next().ok_or("--ellipse: want U,V,RU,RV[,W]")?;
                let (center, radius, gain) = match *floats(a, s)?.as_slice() {
                    [u, v, ru, rv] => ([u, v], [ru, rv], 1.0),
                    [u, v, ru, rv, w] => ([u, v], [ru, rv], w),
                    _ => return Err(format!("--ellipse: want U,V,RU,RV[,W], got {s}")),
                };
                ellipses.push(rig::Ellipse {
                    center,
                    radius,
                    gain,
                    param: String::new(),
                    dir: rig::UP,
                    anchor: None,
                });
                own.push(None);
            }
            "--dir" | "--anchor" => {
                let s = it.next().ok_or(format!("{a}: want A,B"))?;
                let Some(e) = ellipses.last_mut() else {
                    return Err(format!("{a} {s}: give it after an --ellipse"));
                };
                let &[x, y] = floats(a, s)?.as_slice() else {
                    return Err(format!("{a}: want A,B, got {s}"));
                };
                if a == "--dir" {
                    e.dir = [x, y];
                } else {
                    e.anchor = Some([x, y]);
                }
            }
            "--param" => {
                let p = it.next().ok_or("--param: want an id")?.clone();
                if let Some(slot @ None) = own.last_mut() {
                    *slot = Some(p.clone());
                }
                params.push(p);
            }
            _ => {}
        }
    }
    let first = params.first().cloned().unwrap_or_else(|| "ParamBustY".into());
    for (e, p) in ellipses.iter_mut().zip(own) {
        e.param = if params.len() > 1 { p.unwrap_or_else(|| first.clone()) } else { first.clone() };
    }
    let mask = match option("--mask") {
        Some(m) => {
            let img = image::open(&m).map_err(|e| format!("{m}: {e}"))?;
            let param = option("--mask-param").unwrap_or_else(|| first.clone());
            Some((img.to_luma8(), param))
        }
        None => None,
    };
    if (ellipses.is_empty() && mask.is_none())
        || option("--center").is_some()
        || option("--radius").is_some()
    {
        let center = pair("--center", [0.5, 0.4])?;
        let radius = pair("--radius", [0.35, 0.2])?;
        ellipses.push(rig::Ellipse {
            center,
            radius,
            gain: 1.0,
            param: first.clone(),
            dir: rig::UP,
            anchor: None,
        });
    }
    let pivot = match option("--pivot").as_deref() {
        None | Some("center") => rig::Pivot::Center,
        Some("bottom") => rig::Pivot::Bottom,
        Some("top") => rig::Pivot::Top,
        Some(p) => return Err(format!("--pivot: want center, bottom or top, got {p}")),
    };
    let shape = rig::Bust {
        ellipses,
        mask,
        pivot,
        lift: number("--lift", 0.03)?,
        swell: number("--swell", 0.05)?,
        rows: number("--rows", 8.0)? as u32,
        cols: number("--cols", 8.0)? as u32,
    };
    if !dry {
        let mesh = option("--mesh").unwrap_or_else(|| "ArtMeshTopwear".into());
        rig::bust(&mut model, &mesh, &shape)?;
    }
    let version = ayatsuri_import::ExportVersion::DEFAULT;
    for w in ayatsuri_import::export_warnings(&model, version) {
        eprintln!("warning: {w}");
    }
    let moc = ayatsuri_import::to_moc3(&model, version).map_err(|e| e.to_string())?;
    std::fs::write(out, &moc).map_err(|e| format!("{out}: {e}"))?;
    println!(
        "{} parameters, {} deformers, {} meshes; wrote {out}",
        model.parameters.len(),
        model.deformers.len(),
        model.art_meshes.len()
    );
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
