use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anvil_forge::{DataFile, Forge};
use clap::{Parser, Subcommand};

mod gltf;

#[derive(Parser)]
#[command(name = "acbmp", about = "Inspect ACB MP game data")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the entries of a forge archive
    Ls { forge: PathBuf },
    /// Decode every sub-object with the schema; report results per type
    Fastload {
        forge: PathBuf,
        /// Directory holding ACB_MP.schema and ACR_MP.schema
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Rows to print
        #[arg(long, default_value_t = 30)]
        top: usize,
    },
    /// Export a map's static geometry as binary glTF
    Gltf {
        forge: PathBuf,
        out: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
    },
    /// Render one frame of a map offscreen to a PNG
    Shot {
        forge: PathBuf,
        out: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Camera position, world metres (Z up)
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true, default_values_t = [60.0, -80.0, 45.0])]
        eye: Vec<f32>,
        /// Point the camera looks at
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true, default_values_t = [0.0, 0.0, 0.0])]
        target: Vec<f32>,
        #[arg(long, default_value_t = 1600)]
        width: u32,
        #[arg(long, default_value_t = 900)]
        height: u32,
        /// Draw with the placeholder shading instead of the retail shaders
        #[arg(long)]
        placeholder: bool,
    },
    /// List placed meshes, tallest first
    Meshes {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// Load a character Entity (skinned Visuals + skeletons) and check its bind pose
    Character {
        forge: PathBuf,
        name: String,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Write the CPU-skinned character as Wavefront OBJ
        #[arg(long)]
        obj: Option<PathBuf>,
        /// Pose with this animation (name or substring)
        #[arg(long)]
        anim: Option<String>,
        /// Time into the animation, seconds
        #[arg(long, default_value_t = 0.0)]
        t: f32,
    },
    /// Decode a skinned (format 0) mesh and print layout statistics
    Skin {
        forge: PathBuf,
        name: String,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
    },
    /// Print the top-level fields of root objects whose name contains a string
    Dump {
        forge: PathBuf,
        name: String,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Expand nested objects and arrays this many levels
        #[arg(long, default_value_t = 0)]
        depth: usize,
    },
    /// Summarize a MaterialTemplate's compiled shader permutations
    Shaders {
        forge: PathBuf,
        /// Template name (substring)
        name: String,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        #[arg(long, default_value_t = 6)]
        top: usize,
        /// Print every field of each listed permutation
        #[arg(long)]
        fields: bool,
        /// Disassemble permutations whose ShaderKey equals this value
        #[arg(long, value_parser = parse_u32)]
        disasm: Option<u32>,
    },
    /// Translate every shader permutation in a forge to WGSL and validate it
    Shaderlab {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Write the first failing shader's disassembly and WGSL here
        #[arg(long)]
        sample_dir: Option<PathBuf>,
    },
    /// Census of animation track types, frame rate and bone mapping
    Animstats {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
    },
    /// Play an animation on its best-matching skeleton; draw stick figures
    Animplay {
        forge: PathBuf,
        /// Animation name (substring)
        name: String,
        out: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        #[arg(long, default_value_t = 8)]
        frames: usize,
    },
    /// FNV-1a digest over every sub-object's type hash and payload, in order
    Digest { forge: PathBuf },
    /// Count sub-objects per type hash across every .data entry
    Census {
        forge: PathBuf,
        /// Rows to print
        #[arg(long, default_value_t = 40)]
        top: usize,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Ls { forge } => {
            let forge = Forge::open(forge)?;
            for e in forge.entries() {
                println!("{:6} {:#010x} {:10} {}", e.index, e.id, e.length, e.name);
            }
        }
        Cmd::Census { forge, top } => census(forge, top)?,
        Cmd::Digest { forge } => digest(forge)?,
        Cmd::Animplay { forge, name, out, schemas, frames } => animplay(forge, &name, out, schemas, frames)?,
        Cmd::Animstats { forge, schemas } => animstats(forge, schemas)?,
        Cmd::Shaderlab { forge, schemas, sample_dir } => shaderlab(forge, schemas, sample_dir)?,
        Cmd::Shaders { forge, name, schemas, top, fields, disasm } => shaders(forge, &name, schemas, top, fields, disasm)?,
        Cmd::Character { forge, name, schemas, obj, anim, t } => character(forge, &name, schemas, obj, anim.as_deref(), t)?,
        Cmd::Skin { forge, name, schemas } => skin(forge, &name, schemas)?,
        Cmd::Dump { forge, name, schemas, depth } => dump(forge, &name, schemas, depth)?,
        Cmd::Meshes { forge, schemas, top } => meshes(forge, schemas, top)?,
        Cmd::Shot { forge, out, schemas, eye, target, width, height, placeholder } => {
            let shading =
                if placeholder { acb_render::ShadingMode::Placeholder } else { acb_render::ShadingMode::Retail };
            shot(forge, out, schemas, &eye, &target, width, height, shading)?
        }
        Cmd::Gltf { forge, out, schemas } => {
            let t0 = Instant::now();
            let schema = load_schema(&schemas)?;
            let archive = acb_world::Archive::open(&forge, &schema)?;
            let s = gltf::export(&archive, &out)?;
            println!(
                "{} instances of {} meshes ({} skipped: skinned or empty), {} triangles, {:.1}s",
                s.instances, s.meshes, s.skipped_meshes, s.triangles, t0.elapsed().as_secs_f64()
            );
        }
        Cmd::Fastload { forge, schemas, top } => fastload(forge, schemas, top)?,
    }
    Ok(())
}

fn census(path: PathBuf, top: usize) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let mut forge = Forge::open(&path)?;
    let entries = forge.entries().to_vec();
    let mut per_type: HashMap<u32, (usize, usize)> = HashMap::new();
    let (mut files, mut objects, mut failed) = (0, 0, 0);
    for e in entries.iter().filter(|e| !e.is_metafile()) {
        let raw = forge.read(e)?;
        match DataFile::parse(&raw) {
            Ok(df) => {
                files += 1;
                for o in df.objects {
                    objects += 1;
                    let slot = per_type.entry(o.type_hash).or_default();
                    slot.0 += 1;
                    slot.1 += o.payload.len();
                }
            }
            Err(err) => {
                failed += 1;
                eprintln!("entry {} ({}): {err}", e.index, e.name);
            }
        }
    }
    println!(
        "{} entries, {files} data files, {failed} failed, {objects} objects, {} types, {:.1}s",
        entries.len(),
        per_type.len(),
        t0.elapsed().as_secs_f64()
    );
    let mut rows: Vec<_> = per_type.into_iter().collect();
    rows.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    for (hash, (count, bytes)) in rows.into_iter().take(top) {
        println!("{:10.1}MB {count:7} {hash:#010x}", bytes as f64 / 1e6);
    }
    Ok(())
}

fn digest(path: PathBuf) -> anyhow::Result<()> {
    let mut forge = Forge::open(&path)?;
    let entries = forge.entries().to_vec();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    };
    let mut n = 0;
    for e in entries.iter().filter(|e| !e.is_metafile()) {
        for o in DataFile::parse(&forge.read(e)?)?.objects {
            eat(&o.type_hash.to_le_bytes());
            eat(&o.payload);
            n += 1;
        }
    }
    println!("{n} objects digest {h:016x}");
    if let Some(out) = std::env::var_os("ACBMP_DUMP") {
        // concatenation of the same bytes, for cross-checking with other tools
        let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
        for e in entries.iter().filter(|e| !e.is_metafile()) {
            for o in DataFile::parse(&forge.read(e)?)?.objects {
                std::io::Write::write_all(&mut w, &o.type_hash.to_le_bytes())?;
                std::io::Write::write_all(&mut w, &o.payload)?;
            }
        }
    }
    Ok(())
}

fn load_schema(dir: &std::path::Path) -> anyhow::Result<anvil_schema::Schema> {
    Ok(acb_world::load_schema(dir)?)
}

fn open_map<'s>(map: &std::path::Path, schema: &'s anvil_schema::Schema) -> anyhow::Result<acb_world::Archive<'s>> {
    Ok(acb_world::open_map(map, schema)?)
}

fn fastload(path: PathBuf, schemas: PathBuf, top: usize) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let schema = load_schema(&schemas)?;
    let codec = anvil_fastload::Codec::new(&schema);
    let mut forge = Forge::open(&path)?;
    let entries = forge.entries().to_vec();
    // type -> (ok, failed, first error)
    let mut per_type: HashMap<u32, (usize, usize, Option<String>)> = HashMap::new();
    let mut nested = 0usize;
    for e in entries.iter().filter(|e| !e.is_metafile()) {
        for o in DataFile::parse(&forge.read(e)?)?.objects {
            let slot = per_type.entry(o.type_hash).or_default();
            match codec.decode(&o.payload) {
                Ok(root) => {
                    slot.0 += 1;
                    root.obj.walk(&mut |_| nested += 1);
                }
                Err(err) => {
                    slot.1 += 1;
                    slot.2.get_or_insert_with(|| err.to_string());
                }
            }
        }
    }
    let ok: usize = per_type.values().map(|v| v.0).sum();
    let bad: usize = per_type.values().map(|v| v.1).sum();
    println!(
        "{ok} decoded ({nested} objects incl. nested), {bad} failed, {} types, {:.1}s",
        per_type.len(),
        t0.elapsed().as_secs_f64()
    );
    let mut rows: Vec<_> = per_type.into_iter().filter(|r| r.1 .1 > 0).collect();
    rows.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    for (h, (ok, bad, err)) in rows.into_iter().take(top) {
        println!("{bad:6} failed {ok:6} ok  {:28} {}", schema.name_or_hash(h), err.unwrap_or_default());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn shot(
    forge: PathBuf,
    out: PathBuf,
    schemas: PathBuf,
    eye: &[f32],
    target: &[f32],
    width: u32,
    height: u32,
    shading: acb_render::ShadingMode,
) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    let (device, queue) = pollster::block_on(acb_render::headless_device())?;
    let r = acb_render::Renderer::new(device, queue, acb_render::wgpu::TextureFormat::Rgba8UnormSrgb);
    let (scene, s) = acb_render::Scene::load(&r, &archive, shading)?;
    println!(
        "{} meshes ({} skipped), {} instances, {} triangles, {} materials (blend modes {:?}, {} disabled submeshes), {} textures ({} missing), {:.1}s",
        s.meshes, s.skipped_meshes, s.instances, s.triangles, s.materials, s.blend_modes, s.disabled_submeshes, s.textures, s.missing_textures,
        t0.elapsed().as_secs_f64()
    );
    if let Some(rs) = &s.retail {
        println!(
            "retail: {} materials over {} programs; fallbacks {:?}; unhandled globals {:?}",
            rs.materials, rs.programs, rs.fallbacks, rs.unknown_globals
        );
    }
    let camera = acb_render::Camera {
        eye: glam::Vec3::from_slice(eye),
        target: glam::Vec3::from_slice(target),
        fov_y: 60f32.to_radians(),
        aspect: width as f32 / height as f32,
    };
    let rgba = acb_render::render_to_rgba(&r, &scene, &camera, width, height);
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&out)?), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn meshes(forge: PathBuf, schemas: PathBuf, top: usize) -> anyhow::Result<()> {
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let name = |uid: u32| archive.loc(uid).map(|l| l.name.clone()).unwrap_or_else(|| format!("{uid:#010x}"));
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for inst in archive.instances() {
        if !seen.insert(inst.mesh) {
            continue;
        }
        let Some(m) = archive.static_mesh(inst.mesh)? else { continue };
        let (lo, hi) = m.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
            ([lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])], [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])])
        });
        let mats: Vec<String> = m
            .submeshes
            .iter()
            .map(|s| {
                let tex = archive.material(s.material).and_then(|x| x.diffuse);
                format!("{}{}", name(s.material), if tex.is_some() { "" } else { " (no diffuse)" })
            })
            .collect();
        rows.push((hi[2] - lo[2], hi[0] - lo[0], hi[1] - lo[1], name(inst.mesh), mats));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (h, w, d, n, mats) in rows.into_iter().take(top) {
        println!("{h:7.1} x {w:7.1} x {d:7.1}  {n}  [{}]", mats.join(", "));
    }
    Ok(())
}

fn dump(forge: PathBuf, needle: &str, schemas: PathBuf, depth: usize) -> anyhow::Result<()> {
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    let mut uids: Vec<u32> = schema
        .types
        .keys()
        .flat_map(|h| archive.uids_of(&schema.name_or_hash(*h)))
        .filter(|u| archive.loc(*u).is_some_and(|l| l.name.contains(needle)))
        .collect();
    uids.sort();
    uids.dedup();
    for uid in uids.into_iter().take(8) {
        let root = archive.root(uid)?;
        println!("== {uid:#010x} {} ({})", archive.loc(uid).unwrap().name, archive.type_name(uid).unwrap_or_default());
        dump_obj(&archive, &schema, &root.obj, 1, depth);
    }
    Ok(())
}

fn dump_obj(archive: &acb_world::Archive, schema: &anvil_schema::Schema, o: &anvil_fastload::Obj, indent: usize, depth: usize) {
    for (h, v) in &o.fields {
        dump_value(archive, schema, &schema.name_or_hash(*h), v, indent, depth);
    }
    for d in o.dyn_props.iter().flatten() {
        let label = format!("dyn {:#010x} {}", d.name_hash, schema.name_or_hash(d.object_hash));
        dump_value(archive, schema, &label, &d.value, indent, depth);
    }
}

fn dump_value(archive: &acb_world::Archive, schema: &anvil_schema::Schema, label: &str, v: &anvil_fastload::Value, indent: usize, depth: usize) {
    use anvil_fastload::Value;
    let pad = "  ".repeat(indent);
    let text = match v {
        Value::Raw(b) if b.len() <= 4 => format!("{}", v.as_u32().unwrap_or(0)),
        Value::Raw(b) => format!("{:?}", v.as_f32s().unwrap_or_default().iter().take(16).collect::<Vec<_>>()).chars().take(120).collect::<String>() + if b.len() > 64 { "..." } else { "" },
        Value::Ref { id, .. } | Value::Handle { id, .. } => {
            format!("-> {id:#010x} {}", archive.loc(*id).map(|l| l.name.as_str()).unwrap_or("?"))
        }
        Value::Ptr { obj: Some(o), .. } | Value::Object(o) => format!("<{}> #{:#x}", schema.name_or_hash(o.type_hash), o.id),
        Value::Ptr { link, .. } => format!("ptr {link:?}"),
        Value::Str(b) => format!("{:?}", String::from_utf8_lossy(b)),
        Value::Packed { elem_size, data } => format!("[{} x {elem_size}B]", data.len() / elem_size.max(&1)),
        Value::Array(items) => format!("[{} items]", items.len()),
    };
    println!("{pad}{label:28} {text}");
    if depth == 0 {
        return;
    }
    match v {
        Value::Array(items) => {
            for (i, it) in items.iter().enumerate() {
                dump_value(archive, schema, &format!("[{i}]"), it, indent + 1, depth - 1);
            }
        }
        _ => {
            if let Some(o) = v.as_obj() {
                dump_obj(archive, schema, o, indent + 1, depth - 1);
            }
        }
    }
}

fn shaders(forge: PathBuf, needle: &str, schemas: PathBuf, top: usize, fields: bool, disasm: Option<u32>) -> anyhow::Result<()> {
    use anvil_fastload::Value;
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    for uid in archive.uids_of("MaterialTemplate") {
        let name = &archive.loc(uid).unwrap().name;
        if !name.contains(needle) {
            continue;
        }
        let root = archive.root(uid)?;
        println!("== {uid:#010x} {name}");
        for which in ["CompiledTemplatePrimary", "CompiledTemplateSecondary"] {
            let Some(ct) = archive.field(&root.obj, which).and_then(Value::as_obj) else { continue };
            for list in ["VertexShaderPermutations", "PixelShaderPermutations", "LightingPermutations"] {
                let perms = archive.field(ct, list).map(|v| v.items()).unwrap_or_default();
                println!("  {which}.{list}: {}", perms.len());
                for (i, p) in perms.iter().filter_map(Value::as_obj).enumerate() {
                    let key = archive.field(p, "ShaderKey").and_then(Value::as_u32).unwrap_or(0);
                    // --disasm lists every permutation with that key, others up to --top
                    if disasm.map_or(i >= top, |d| d != key) {
                        continue;
                    }
                    let code = archive.field(p, "CompiledCode").and_then(Value::as_bytes).unwrap_or_default();
                    let ver = code.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
                    let params: Vec<String> = archive.field(p, "Parameters")
                        .map(|v| v.items())
                        .unwrap_or_default()
                        .iter()
                        .filter_map(Value::as_obj)
                        .map(|q| {
                            let id = archive.field(q, "PropertyID").and_then(Value::as_u32).unwrap_or(0);
                            let c = archive.field(q, "ConstantIndex").and_then(Value::as_u32).unwrap_or(0);
                            let n = archive.field(q, "NbVectors").and_then(Value::as_u32).unwrap_or(0);
                            let t = archive.field(q, "Type").and_then(Value::as_u32).unwrap_or(0);
                            format!("{}@{c}x{n}t{t}", schema.name_or_hash(id))
                        })
                        .collect();
                    println!("    key {key:#010x} code {} B ver {ver:#010x} params {}", code.len(), params.join(" "));
                    if fields {
                        for (h, v) in &p.fields {
                            let text = match v {
                                Value::Raw(b) if b.len() <= 4 => format!("{:#x}", v.as_u32().unwrap_or(0)),
                                Value::Raw(b) => format!("{} B", b.len()),
                                Value::Array(items) => format!("[{} items]", items.len()),
                                other => format!("{:.60}", format!("{other:?}")),
                            };
                            println!("      {:24} {text}", schema.name_or_hash(*h));
                        }
                    }
                    if disasm == Some(key) {
                        match d3d9_shader::parse(code) {
                            Ok(prog) => println!("{prog}"),
                            Err(e) => println!("      parse error: {e}"),
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn shaderlab(forge: PathBuf, schemas: PathBuf, sample_dir: Option<PathBuf>) -> anyhow::Result<()> {
    use anvil_fastload::Value;
    let t0 = Instant::now();
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    let mut validator = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all());
    let (mut total, mut parsed, mut translated, mut valid) = (0usize, 0usize, 0usize, 0usize);
    let mut errors: HashMap<String, usize> = HashMap::new();
    let mut seen_bytes = std::collections::HashSet::new();
    let mut samples = 0;
    for uid in archive.uids_of("MaterialTemplate") {
        let Ok(root) = archive.root(uid) else { continue };
        for which in ["CompiledTemplatePrimary", "CompiledTemplateSecondary"] {
            let Some(ct) = archive.field(&root.obj, which).and_then(Value::as_obj) else { continue };
            for list in ["VertexShaderPermutations", "PixelShaderPermutations", "LightingPermutations"] {
                for p in archive.field(ct, list).map(|v| v.items()).unwrap_or_default().iter().filter_map(Value::as_obj) {
                    let Some(code) = archive.field(p, "CompiledCode").and_then(Value::as_bytes) else { continue };
                    if code.is_empty() || !seen_bytes.insert(code.to_vec()) {
                        continue;
                    }
                    total += 1;
                    let mut fail = |kind: &str, msg: String, prog: Option<&d3d9_shader::Program>, wgsl: Option<&str>| {
                        let key: String = format!("{kind}: {msg}").chars().take(110).collect();
                        *errors.entry(key).or_default() += 1;
                        if let (Some(dir), true) = (&sample_dir, samples < 12) {
                            samples += 1;
                            let _ = std::fs::create_dir_all(dir);
                            let base = dir.join(format!("fail{samples:02}"));
                            let _ = std::fs::write(base.with_extension("txt"), format!("{msg}\n\n{}", prog.map(|p| p.to_string()).unwrap_or_default()));
                            if let Some(w) = wgsl {
                                let _ = std::fs::write(base.with_extension("wgsl"), w);
                            }
                        }
                    };
                    let prog = match d3d9_shader::parse(code) {
                        Ok(p) => p,
                        Err(e) => {
                            fail("parse", e.to_string(), None, None);
                            continue;
                        }
                    };
                    parsed += 1;
                    let t = match d3d9_shader::wgsl::translate(&prog, &Default::default()) {
                        Ok(t) => t,
                        Err(e) => {
                            fail("translate", e.to_string(), Some(&prog), None);
                            continue;
                        }
                    };
                    translated += 1;
                    let module = match naga::front::wgsl::parse_str(&t.wgsl) {
                        Ok(m) => m,
                        Err(e) => {
                            fail("wgsl", e.message().to_string(), Some(&prog), Some(&t.wgsl));
                            continue;
                        }
                    };
                    match validator.validate(&module) {
                        Ok(_) => valid += 1,
                        Err(e) => fail("validate", format!("{}", e.as_inner()), Some(&prog), Some(&t.wgsl)),
                    }
                }
            }
        }
    }
    println!(
        "{total} unique shaders: {parsed} parsed, {translated} translated, {valid} valid WGSL ({:.1}%), {:.1}s",
        100.0 * valid as f64 / total.max(1) as f64,
        t0.elapsed().as_secs_f64()
    );
    let mut rows: Vec<_> = errors.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    for (msg, n) in rows.into_iter().take(15) {
        println!("{n:7}  {msg}");
    }
    Ok(())
}

fn animstats(forge: PathBuf, schemas: PathBuf) -> anyhow::Result<()> {
    use acb_anim::{parse_tracks, Track};
    use anvil_fastload::Value;
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;

    // every BoneID of every skeleton
    let mut bones = std::collections::HashSet::new();
    for uid in archive.uids_of("Skeleton") {
        let Ok(r) = archive.root(uid) else { continue };
        for b in archive.field(&r.obj, "Bones").map(|v| v.items()).unwrap_or_default().iter().filter_map(Value::as_obj) {
            if let Some(id) = archive.field(b, "BoneID").and_then(Value::as_u32) {
                bones.insert(id);
            }
        }
    }

    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    let (mut anims, mut failed, mut tracks, mut builtin) = (0, 0, 0, 0);
    let (mut id_match, mut id_total, mut mapping_mismatch) = (0, 0, 0);
    let mut fps_samples = Vec::new();
    let (mut quats, mut bad_quats) = (0usize, 0usize);
    let mut unmatched_ids: HashMap<u32, usize> = HashMap::new();
    let mut continuity: std::collections::BTreeMap<String, Vec<f32>> = Default::default();
    for uid in archive.uids_of("Animation") {
        let Ok(r) = archive.root(uid) else { failed += 1; continue };
        let Some(tail) = &r.obj.custom else { failed += 1; continue };
        let Ok(ts) = parse_tracks(tail) else { failed += 1; continue };
        anims += 1;
        let ids: Vec<u32> = archive
            .field(&r.obj, "AnimTrackData")
            .and_then(Value::as_obj)
            .and_then(|d| archive.field(d, "AnimTrackDataMapping"))
            .map(|v| v.items().iter().filter_map(Value::as_obj).filter_map(|m| archive.field(m, "TrackID").and_then(Value::as_u32)).collect())
            .unwrap_or_default();
        if ids.len() != ts.len() {
            mapping_mismatch += 1;
        }
        for id in &ids {
            id_total += 1;
            if bones.contains(id) {
                id_match += 1;
            } else {
                *unmatched_ids.entry(*id).or_default() += 1;
            }
        }
        let length = archive.field(&r.obj, "Length").and_then(Value::as_f32s).and_then(|v| v.first().copied()).unwrap_or(0.0);
        let mut max_t = 0;
        for t in &ts {
            tracks += 1;
            match t {
                Track::Builtin(_) => builtin += 1,
                Track::Data(d) => {
                    *kinds.entry(format!("{:?}/{:?}", d.compression, d.format)).or_default() += 1;
                    max_t = max_t.max(d.times.last().copied().unwrap_or(0));
                    if let Some(qs) = d.quats() {
                        let steps = continuity.entry(format!("{:?}", d.compression)).or_default();
                        for (i, q) in qs.iter().enumerate() {
                            quats += 1;
                            if (q.length() - 1.0).abs() > 0.01 {
                                bad_quats += 1;
                            }
                            if i > 0 {
                                // angle between consecutive keys, degrees
                                steps.push(qs[i - 1].dot(*q).abs().min(1.0).acos().to_degrees() * 2.0);
                            }
                        }
                    }
                    if let Some(vs) = d.vec3s() {
                        let steps = continuity.entry(format!("{:?} (m)", d.compression)).or_default();
                        for w in vs.windows(2) {
                            steps.push(w[0].distance(w[1]));
                        }
                    }
                }
            }
        }
        if length > 0.1 && max_t > 0 {
            fps_samples.push(max_t as f32 / length);
        }
    }
    fps_samples.sort_by(|a, b| a.total_cmp(b));
    let median = fps_samples.get(fps_samples.len() / 2).copied().unwrap_or(0.0);
    println!("{anims} animations ({failed} failed), {tracks} tracks ({builtin} built-in), {} skeleton bone ids", bones.len());
    println!("frames per Length unit: median {median:.2} (p10 {:.2}, p90 {:.2})",
        fps_samples.get(fps_samples.len() / 10).copied().unwrap_or(0.0),
        fps_samples.get(fps_samples.len() * 9 / 10).copied().unwrap_or(0.0));
    println!("TrackID matches a BoneID: {id_match}/{id_total}; mapping/track count mismatches: {mapping_mismatch}");
    let mut top: Vec<_> = unmatched_ids.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    println!("most common unmatched TrackIDs: {:?}", top.iter().take(8).map(|(i, n)| format!("{i:#010x} x{n} {}", schema.name_or_hash(*i))).collect::<Vec<_>>());
    println!("decoded quats {quats}, off unit length {bad_quats}");
    println!("consecutive-key steps (rotations in degrees, vectors in metres):");
    for (k, mut v) in continuity {
        v.sort_by(|a, b| a.total_cmp(b));
        let at = |f: f32| v.get(((v.len() as f32 * f) as usize).min(v.len().saturating_sub(1))).copied().unwrap_or(0.0);
        let big = v.iter().filter(|&&x| x > 45.0).count();
        println!("  {k:22} n {:8}  median {:8.3}  p99 {:8.3}  >45: {:.2}%", v.len(), at(0.5), at(0.99), 100.0 * big as f32 / v.len().max(1) as f32);
    }
    let mut rows: Vec<_> = kinds.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, n) in rows {
        println!("{n:9}  {k}");
    }
    Ok(())
}

fn animplay(forge: PathBuf, needle: &str, out: PathBuf, schemas: PathBuf, frames: usize) -> anyhow::Result<()> {
    use acb_anim::pose::{Clip, Skeleton};
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let anims = archive.uids_of("Animation");
    let uid = anims
        .iter()
        .find(|u| archive.loc(**u).is_some_and(|l| l.name == needle))
        .or_else(|| anims.iter().find(|u| archive.loc(**u).is_some_and(|l| l.name.contains(needle))))
        .copied()
        .ok_or_else(|| anyhow::anyhow!("no animation matching {needle}"))?;
    let clip = Clip::load(&archive, uid).ok_or_else(|| anyhow::anyhow!("animation did not decode"))?;
    let skeleton = archive
        .uids_of("Skeleton")
        .into_iter()
        .filter_map(|s| Skeleton::load(&archive, s))
        .max_by_key(|s| (clip.bone_coverage(s), s.bones.len()))
        .ok_or_else(|| anyhow::anyhow!("no skeleton"))?;
    println!(
        "{} ({:.2}s, {} tracks) on {} ({} bones, {} tracks matched)",
        archive.loc(uid).unwrap().name,
        clip.length,
        clip.tracks.len(),
        archive.loc(skeleton.uid).map(|l| l.name.as_str()).unwrap_or("?"),
        skeleton.bones.len(),
        clip.bone_coverage(&skeleton)
    );

    // world-space joint positions per frame
    let poses: Vec<Vec<glam::Vec3>> = (0..frames)
        .map(|i| {
            let t = clip.length * i as f32 / (frames.max(2) - 1) as f32;
            skeleton.globals(&clip.sample(&skeleton, t)).iter().map(|m| m.w_axis.truncate()).collect()
        })
        .collect();
    let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
    for p in poses.iter().flatten() {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let cell = 220usize;
    let (w, h) = (cell * frames, cell * 2);
    let mut img = vec![255u8; w * h * 4];
    let span = (hi - lo).max_element().max(0.01);
    let mut line = |x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 3]| {
        let n = ((x1 - x0).abs().max((y1 - y0).abs()) as usize).max(1);
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let (x, y) = ((x0 + (x1 - x0) * t) as isize, (y0 + (y1 - y0) * t) as isize);
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                let o = (y as usize * w + x as usize) * 4;
                img[o..o + 3].copy_from_slice(&c);
            }
        }
    };
    for (f, joints) in poses.iter().enumerate() {
        for (row, (ax, color)) in [(0usize, [200u8, 40, 40]), (1usize, [40u8, 60, 200])].into_iter().enumerate() {
            // row 0: front view (x right, z up); row 1: side view (y right, z up)
            let map = |p: glam::Vec3| {
                let u = if ax == 0 { p.x - lo.x } else { p.y - lo.y };
                let v = p.z - lo.z;
                (
                    (f * cell) as f32 + 10.0 + u / span * (cell as f32 - 20.0),
                    (row * cell) as f32 + cell as f32 - 10.0 - v / span * (cell as f32 - 20.0),
                )
            };
            for (i, b) in skeleton.bones.iter().enumerate() {
                if let Some(pi) = b.parent {
                    let (a, c) = (map(joints[pi]), map(joints[i]));
                    line(a.0, a.1, c.0, c.1, color);
                }
            }
        }
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&out)?), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&img)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn parse_u32(s: &str) -> Result<u32, String> {
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16),
        None => s.parse(),
    }
    .map_err(|e| e.to_string())
}

fn skin(forge: PathBuf, needle: &str, schemas: PathBuf) -> anyhow::Result<()> {
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let uid = archive
        .uids_of("Mesh")
        .into_iter()
        .find(|u| archive.loc(*u).is_some_and(|l| l.name == needle))
        .ok_or_else(|| anyhow::anyhow!("no mesh named {needle}"))?;
    let m = archive.skinned_mesh(uid)?.ok_or_else(|| anyhow::anyhow!("not a skinned mesh"))?;
    println!("{} vertices, {} triangles, {} bones", m.positions.len(), m.indices.len() / 3, m.bones.len());
    let (lo, hi) = m.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
        (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
    });
    println!("bounds {lo:?} .. {hi:?}");
    let nlen: f32 = m.normals.iter().map(|n| (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()).sum::<f32>()
        / m.normals.len() as f32;
    let wsum: Vec<u32> = m.raw.chunks_exact(m.stride).map(|v| v[28..32].iter().map(|&b| b as u32).sum()).collect();
    println!(
        "mean |normal| {nlen:.3}; weight byte sums min {} max {}",
        wsum.iter().min().unwrap(),
        wsum.iter().max().unwrap()
    );
    for (i, s) in m.submeshes.iter().enumerate() {
        let idx = &m.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
        let (imin, imax) = (idx.iter().min().copied().unwrap_or(0), idx.iter().max().copied().unwrap_or(0));
        let vr = s.first_vertex as usize..(s.first_vertex + s.vertex_count) as usize;
        let jmax = m.joints[vr.clone()].iter().flat_map(|j| j.iter().copied()).max().unwrap_or(0);
        println!(
            "  sub {i}: tris {} idx {imin}..={imax} verts {:?} max joint {jmax} palette {} {:?} mat {}",
            s.index_count / 3,
            vr,
            s.palette.len(),
            &s.palette,
            archive.loc(s.material).map(|l| l.name.as_str()).unwrap_or("?")
        );
    }
    for v in m.raw.chunks_exact(m.stride).take(6) {
        println!("  {}", v.chunks(4).map(|c| c.iter().map(|b| format!("{b:02x}")).collect::<String>()).collect::<Vec<_>>().join(" "));
    }
    let ids: std::collections::HashSet<u32> = m.bones.iter().map(|b| b.id).collect();
    let skel = archive
        .uids_of("Skeleton")
        .into_iter()
        .filter_map(|s| acb_anim::pose::Skeleton::load(&archive, s))
        .max_by_key(|s| s.bones.iter().filter(|b| ids.contains(&b.id)).count());
    if let Some(skel) = skel {
        let hit = skel.bones.iter().filter(|b| ids.contains(&b.id)).count();
        println!("skeleton {} covers {hit}/{} mesh bones", archive.loc(skel.uid).map(|l| l.name.as_str()).unwrap_or("?"), ids.len());
        let g = skel.globals(&skel.bind_pose());
        if std::env::var_os("ACBMP_DUMP_BIND").is_some() {
            for b in &m.bones {
                let Some(si) = skel.bones.iter().position(|sb| sb.id == b.id) else { continue };
                let (inv, gl) = (g[si].inverse().w_axis, g[si].w_axis);
                println!("BIND {} {} {} {} {} {} {} {}", b.packed.1, b.packed.2, inv.x, inv.y, inv.z, gl.x, gl.y, gl.z);
            }
        }
        for b in m.bones.iter().take(8) {
            let Some(si) = skel.bones.iter().position(|sb| sb.id == b.id) else { continue };
            let inv = g[si].inverse();
            let j: Vec<f32> = b.packed.0[..3].iter().map(|&v| v as i16 as f32 / 32767.0).collect();
            let k: Vec<f32> = b.packed.0[3..].iter().map(|&v| v as i16 as f32 / 32767.0).collect();
            println!("  bone {:#010x} J {j:.3?} K {k:.3?} T {:#010x} {:#010x}", b.id, b.packed.1, b.packed.2);
            println!("     inv cols x {:.3} y {:.3} z {:.3} t {:.4}", inv.x_axis.truncate(), inv.y_axis.truncate(), inv.z_axis.truncate(), inv.w_axis.truncate());
        }
    }
    Ok(())
}

fn find_uid(archive: &acb_world::Archive, type_name: &str, needle: &str) -> anyhow::Result<u32> {
    let uids = archive.uids_of(type_name);
    uids.iter()
        .find(|u| archive.loc(**u).is_some_and(|l| l.name == needle))
        .or_else(|| uids.iter().find(|u| archive.loc(**u).is_some_and(|l| l.name.contains(needle))))
        .copied()
        .ok_or_else(|| anyhow::anyhow!("no {type_name} matching {needle}"))
}

fn character(forge: PathBuf, needle: &str, schemas: PathBuf, obj: Option<PathBuf>, anim: Option<&str>, t: f32) -> anyhow::Result<()> {
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let name = |u: u32| archive.loc(u).map(|l| l.name.clone()).unwrap_or_else(|| format!("{u:#010x}"));
    let uid = find_uid(&archive, "Entity", needle)?;
    let c = acb_anim::character::Character::load(&archive, uid).ok_or_else(|| anyhow::anyhow!("not a character"))?;
    println!("{}: {} skeleton bones, {} skinned parts", name(uid), c.skeleton.bones.len(), c.parts.len());
    let globals = c.skeleton.globals(&c.skeleton.bind_pose());
    let skins = c.skin_matrices(&globals);
    for (p, skin) in c.parts.iter().zip(&skins) {
        let unmapped = p.bone_map.iter().filter(|b| b.is_none()).count();
        // bind pose: skinned position minus the authored one turned into
        // skeleton space (+90 degrees about Z); constant per rigid part
        let mut sum = glam::Vec3::ZERO;
        let mut dev = Vec::new();
        for (vi, pos) in p.mesh.positions.iter().enumerate() {
            let Some(sub) = p.mesh.submeshes.iter().find(|s| (s.first_vertex..s.first_vertex + s.vertex_count).contains(&(vi as u32))) else { continue };
            let v = glam::Vec3::from(*pos).extend(1.0);
            let mut out = glam::Vec4::ZERO;
            for k in 0..4 {
                let w = p.mesh.weights[vi][k];
                if w > 0.0 {
                    let mb = sub.palette.get(p.mesh.joints[vi][k] as usize).copied().unwrap_or(0) as usize;
                    out += w * (skin[mb] * v);
                }
            }
            let d = out.truncate() - glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * glam::Vec3::from(*pos);
            sum += d;
            dev.push(d);
        }
        let mean = sum / dev.len().max(1) as f32;
        let spread = dev.iter().map(|d| (*d - mean).length()).fold(0.0f32, f32::max);
        println!(
            "  {} : {} verts, {} bones ({unmapped} unmapped), mats [{}], bind offset {:.4} spread {:.4}",
            name(p.mesh_uid),
            p.mesh.positions.len(),
            p.mesh.bones.len(),
            p.materials.iter().map(|m| name(*m)).collect::<Vec<_>>().join(", "),
            mean,
            spread
        );
    }
    if std::env::var_os("ACBMP_DEBUG_PALETTE").is_some() {
        // distance from each vertex to its heaviest bone's bind origin, via the palette vs direct
        for p in &c.parts {
            let origin: Vec<glam::Vec3> = p.inverse_bind.iter().map(|m| m.inverse().w_axis.truncate()).collect();
            let (mut dp, mut dd, mut n) = (0.0f32, 0.0f32, 0);
            for (vi, pos) in p.mesh.positions.iter().enumerate() {
                let Some(sub) = p.mesh.submeshes.iter().find(|s| (s.first_vertex..s.first_vertex + s.vertex_count).contains(&(vi as u32))) else { continue };
                let k = (0..4).max_by(|a, b| p.mesh.weights[vi][*a].total_cmp(&p.mesh.weights[vi][*b])).unwrap();
                let j = p.mesh.joints[vi][k] as usize;
                let v = glam::Vec3::from(*pos);
                if let Some(&mb) = sub.palette.get(j) { dp += (origin[mb as usize] - v).length(); }
                if let Some(o) = origin.get(j) { dd += (*o - v).length(); }
                n += 1;
            }
            println!("  {} palette {:.3} direct {:.3}", name(p.mesh_uid), dp / n as f32, dd / n as f32);
            if p.mesh.bones.len() > 40 {
                let mut acc = vec![(glam::Vec3::ZERO, 0usize); p.mesh.bones.len()];
                for (vi, pos) in p.mesh.positions.iter().enumerate() {
                    let Some(sub) = p.mesh.submeshes.iter().find(|s| (s.first_vertex..s.first_vertex + s.vertex_count).contains(&(vi as u32))) else { continue };
                    let k = (0..4).max_by(|a, b| p.mesh.weights[vi][*a].total_cmp(&p.mesh.weights[vi][*b])).unwrap();
                    if let Some(&mb) = sub.palette.get(p.mesh.joints[vi][k] as usize) {
                        acc[mb as usize].0 += glam::Vec3::from(*pos);
                        acc[mb as usize].1 += 1;
                    }
                }
                for (i, (sum, n)) in acc.iter().enumerate() {
                    if *n > 0 {
                        println!("    mb {i:2} -> sk {:?} origin {:.2} centroid {:.2} n {n}", p.bone_map[i], origin[i], *sum / *n as f32);
                    }
                }
            }
        }
    }
    if let Some(out) = obj {
        use std::io::Write;
        let local = match anim {
            Some(a) => {
                let clip = acb_anim::pose::Clip::load(&archive, find_uid(&archive, "Animation", a)?)
                    .ok_or_else(|| anyhow::anyhow!("animation did not decode"))?;
                println!("posed with {} at {t:.2}s of {:.2}s ({} bones animated)", name(clip.uid), clip.length, clip.bone_coverage(&c.skeleton));
                let posed = clip.sample(&c.skeleton, t);
                posed
            }
            None => c.skeleton.bind_pose(),
        };
        let globals = c.skeleton.globals(&local);
        let skins = c.skin_matrices(&globals);
        let mut w = std::io::BufWriter::new(std::fs::File::create(&out)?);
        let mut base = 1usize;
        if std::env::var_os("ACBMP_OBJ_JOINTS").is_some() {
            // a thin prism per bone, parent -> child
            writeln!(w, "o joints")?;
            for (i, b) in c.skeleton.bones.iter().enumerate() {
                let Some(p) = b.parent else { continue };
                let (a, e) = (globals[p].w_axis.truncate(), globals[i].w_axis.truncate());
                let d = 0.008;
                for o in [glam::Vec3::X * d, glam::Vec3::Y * d, glam::Vec3::Z * d] {
                    writeln!(w, "v {} {} {}\nv {} {} {}\nv {} {} {}", a.x - o.x, a.y - o.y, a.z - o.z, a.x + o.x, a.y + o.y, a.z + o.z, e.x, e.y, e.z)?;
                    writeln!(w, "f {} {} {}", base, base + 1, base + 2)?;
                    base += 3;
                }
            }
        }
        for (p, skin) in c.parts.iter().zip(&skins) {
            writeln!(w, "o {}", name(p.mesh_uid))?;
            for (vi, pos) in p.mesh.positions.iter().enumerate() {
                let sub = p.mesh.submeshes.iter().find(|s| (s.first_vertex..s.first_vertex + s.vertex_count).contains(&(vi as u32)));
                let v = glam::Vec3::from(*pos).extend(1.0);
                let mut o = glam::Vec4::ZERO;
                for k in 0..4 {
                    let wt = p.mesh.weights[vi][k];
                    let mb = sub.and_then(|s| s.palette.get(p.mesh.joints[vi][k] as usize)).copied().unwrap_or(0) as usize;
                    o += wt * (skin[mb] * v);
                }
                writeln!(w, "v {} {} {}", o.x, o.y, o.z)?;
            }
            for t in p.mesh.indices.chunks_exact(3) {
                writeln!(w, "f {} {} {}", base + t[0] as usize, base + t[1] as usize, base + t[2] as usize)?;
            }
            base += p.mesh.positions.len();
        }
        println!("wrote {}", out.display());
    }
    Ok(())
}
