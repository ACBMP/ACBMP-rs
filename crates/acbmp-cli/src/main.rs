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
    },
    /// List placed meshes, tallest first
    Meshes {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// Print the top-level fields of root objects whose name contains a string
    Dump {
        forge: PathBuf,
        name: String,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
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
        Cmd::Animstats { forge, schemas } => animstats(forge, schemas)?,
        Cmd::Shaderlab { forge, schemas, sample_dir } => shaderlab(forge, schemas, sample_dir)?,
        Cmd::Shaders { forge, name, schemas, top } => shaders(forge, &name, schemas, top)?,
        Cmd::Dump { forge, name, schemas } => dump(forge, &name, schemas)?,
        Cmd::Meshes { forge, schemas, top } => meshes(forge, schemas, top)?,
        Cmd::Shot { forge, out, schemas, eye, target, width, height } => {
            shot(forge, out, schemas, &eye, &target, width, height)?
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

fn shot(
    forge: PathBuf,
    out: PathBuf,
    schemas: PathBuf,
    eye: &[f32],
    target: &[f32],
    width: u32,
    height: u32,
) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    let (device, queue) = pollster::block_on(acb_render::headless_device())?;
    let r = acb_render::Renderer::new(device, queue, acb_render::wgpu::TextureFormat::Rgba8UnormSrgb);
    let (scene, s) = acb_render::Scene::load(&r, &archive)?;
    println!(
        "{} meshes ({} skipped), {} instances, {} triangles, {} materials (blend modes {:?}, {} disabled submeshes), {} textures ({} missing), {:.1}s",
        s.meshes, s.skipped_meshes, s.instances, s.triangles, s.materials, s.blend_modes, s.disabled_submeshes, s.textures, s.missing_textures,
        t0.elapsed().as_secs_f64()
    );
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

fn dump(forge: PathBuf, needle: &str, schemas: PathBuf) -> anyhow::Result<()> {
    use anvil_fastload::Value;
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let mut uids: Vec<u32> = schema
        .types
        .keys()
        .flat_map(|h| archive.uids_of(&schema.name_or_hash(*h)))
        .filter(|u| archive.loc(*u).is_some_and(|l| l.name.contains(needle)))
        .collect();
    uids.sort();
    for uid in uids.into_iter().take(8) {
        let root = archive.root(uid)?;
        println!("== {uid:#010x} {} ({})", archive.loc(uid).unwrap().name, archive.type_name(uid).unwrap_or_default());
        for (h, v) in &root.obj.fields {
            let text = match v {
                Value::Raw(b) if b.len() <= 4 => format!("{}", v.as_u32().unwrap_or(0)),
                Value::Raw(b) => format!("{:?}", v.as_f32s().unwrap_or_default().iter().take(16).collect::<Vec<_>>()).chars().take(120).collect::<String>() + if b.len() > 64 { "..." } else { "" },
                Value::Ref { id, .. } | Value::Handle { id, .. } => {
                    format!("-> {id:#010x} {}", archive.loc(*id).map(|l| l.name.as_str()).unwrap_or("?"))
                }
                Value::Ptr { obj: Some(o), .. } | Value::Object(o) => format!("<{}>", schema.name_or_hash(o.type_hash)),
                Value::Ptr { link, .. } => format!("ptr {link:?}"),
                Value::Str(b) => format!("{:?}", String::from_utf8_lossy(b)),
                Value::Packed { elem_size, data } => format!("[{} x {elem_size}B]", data.len() / elem_size.max(&1)),
                Value::Array(items) => format!("[{} items]", items.len()),
            };
            println!("  {:28} {text}", schema.name_or_hash(*h));
        }
    }
    Ok(())
}

fn shaders(forge: PathBuf, needle: &str, schemas: PathBuf, top: usize) -> anyhow::Result<()> {
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
                for p in perms.iter().filter_map(Value::as_obj).take(top) {
                    let key = archive.field(p, "ShaderKey").and_then(Value::as_u32).unwrap_or(0);
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
