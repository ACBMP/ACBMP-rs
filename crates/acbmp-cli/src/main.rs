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
        /// Also list roots holding a dynamic property with this key
        #[arg(long, value_parser = parse_u32)]
        dyn_key: Option<u32>,
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
        /// Also time this many frames and print the average
        #[arg(long, default_value_t = 0)]
        bench: u32,
    },
    /// What a `shot` pixel shows: entity, mesh, material under it
    Pick {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true, default_values_t = [60.0, -80.0, 45.0])]
        eye: Vec<f32>,
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true, default_values_t = [0.0, 0.0, 0.0])]
        target: Vec<f32>,
        #[arg(long, default_value_t = 1600)]
        width: u32,
        #[arg(long, default_value_t = 900)]
        height: u32,
        /// Pixel x,y (repeatable)
        #[arg(long = "px", value_delimiter = ',')]
        px: Vec<f32>,
    },
    /// List placed meshes, tallest first
    Meshes {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// Decode Cloth soft bodies and check them against their visual meshes
    Cloth {
        forge: PathBuf,
        /// Name substring (empty: all)
        #[arg(default_value = "")]
        name: String,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Check ragdoll capsule conventions on this character (builder)
        #[arg(long)]
        character: Option<String>,
    },
    /// Decode the map's navmeshes; optionally export them
    Navmesh {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Write the joined navmesh as Wavefront OBJ
        #[arg(long)]
        obj: Option<PathBuf>,
    },
    /// Decode the map's freerun guidance edges and check them against collision
    Guidance {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Write the edges as OBJ polylines
        #[arg(long)]
        obj: Option<PathBuf>,
    },
    /// Bone extents of a character posed by a clip (entity space)
    Posebounds {
        forge: PathBuf,
        /// EntityBuilder
        builder: String,
        clip: String,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        #[arg(long, env = "ACBMP_EXE")]
        exe: PathBuf,
        #[arg(long, default_value_t = 0.0)]
        t: f32,
    },
    /// Write a TextureMap's top mip level as PNG (RGB and alpha side by side)
    Texture {
        forge: PathBuf,
        name: String,
        out: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
    },
    /// Print the map's lighting setup (sky, sun, ambient, fog, lightmaps)
    Ambiance {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
    },
    /// Census of the map's rigid bodies and shapes; optionally export them
    Collision {
        forge: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Write every decoded shape in world space as Wavefront OBJ
        #[arg(long)]
        obj: Option<PathBuf>,
    },
    /// List the roots of one type whose name contains a string
    Names {
        forge: PathBuf,
        #[arg(name = "TYPE")]
        type_name: String,
        #[arg(default_value = "")]
        needle: String,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
    },
    /// Render a character Entity with the game's shaders, posed by an animation
    Charshot {
        forge: PathBuf,
        name: String,
        out: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// Animation (name or substring); bind pose without
        #[arg(long)]
        anim: Option<String>,
        /// Frames across the animation, laid out left to right
        #[arg(long, default_value_t = 1)]
        frames: usize,
        /// Camera heading around the character, degrees
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        yaw: f32,
        /// Pixel size of one frame
        #[arg(long, default_value_t = 512)]
        size: u32,
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
        /// ACBMP.exe, to decode built-in tracks too
        #[arg(long, env = "ACBMP_EXE")]
        exe: Option<PathBuf>,
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
        /// Join a stride half (`*_footl`) with its other half (`*_footr`)
        #[arg(long)]
        stride: bool,
    },
    /// Root motion of animations: length, displacement, yaw, speed profile
    Rootmotion {
        forge: PathBuf,
        /// Animation names (exact, case-insensitive)
        names: Vec<String>,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        #[arg(long, env = "ACBMP_EXE")]
        exe: PathBuf,
        /// Join a stride half (`*_footl`) with its other half (`*_footr`)
        #[arg(long)]
        stride: bool,
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
    /// Authoritative headless match server: waits for --players clients,
    /// then runs the game sim at 60 Hz (our own UDP protocol, see NOTES.md
    /// "Netcode (ours)")
    Serve {
        map: PathBuf,
        #[arg(long, env = "ACBMP_SCHEMAS")]
        schemas: PathBuf,
        /// ACBMP.exe for the builtin animation tracks (clients need the same)
        #[arg(long, env = "ACBMP_EXE")]
        exe: Option<PathBuf>,
        #[arg(long, default_value_t = 7777)]
        port: u16,
        #[arg(long, default_value_t = 2)]
        players: usize,
        /// A snapshot every N ticks (2 = 30 Hz)
        #[arg(long, default_value_t = 2)]
        snapshot_every: u32,
        /// Simulated network on the server side, e.g. loss=5%,delay=40ms,jitter=10ms
        #[arg(long, default_value = "")]
        netsim: String,
        /// Start of slot i (repeatable, in slot order): x,y,z,yaw_degrees
        #[arg(long, allow_hyphen_values = true)]
        at: Vec<String>,
        /// Write every tick's quantized player state and the events here
        #[arg(long)]
        log: Option<PathBuf>,
        /// Stop after this many seconds of play
        #[arg(long)]
        duration: Option<f32>,
        /// duel (Wanted contracts, scores, respawns; any player count) or free (no match rules)
        #[arg(long, default_value = "duel")]
        mode: String,
        /// Match length, seconds
        #[arg(long, default_value_t = 300.0)]
        time_limit: f32,
        /// The first to this score wins (0: none)
        #[arg(long, default_value_t = 1000)]
        score_limit: u32,
        /// Seconds from a kill to the victim's respawn
        #[arg(long, default_value_t = acb_net::mode::RESPAWN_DELAY)]
        respawn: f32,
        /// No contracts during the first seconds
        #[arg(long, default_value_t = acb_net::mode::START_DELAY)]
        start_delay: f32,
        /// Crowd NPCs every peer simulates (blending; the map needs a navmesh)
        #[arg(long, default_value_t = 0)]
        crowd: u16,
        /// Seed of the crowd's wandering
        #[arg(long, default_value_t = 0)]
        crowd_seed: u32,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Serve { map, schemas, exe, port, players, snapshot_every, netsim, at, log, duration, mode, time_limit, score_limit, respawn, start_delay, crowd, crowd_seed } => {
            let rules = match mode.as_str() {
                "duel" => Some(acb_net::mode::MatchConfig { time_limit, score_limit, respawn_delay: respawn, start_delay }),
                "free" => None,
                m => anyhow::bail!("--mode {m}: duel or free"),
            };
            let at = at
                .iter()
                .map(|a| {
                    let v: Vec<f32> = a.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>()?;
                    anyhow::ensure!(v.len() == 4, "--at x,y,z,yaw");
                    Ok(Some([v[0], v[1], v[2], v[3]]))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let netsim = acb_net::link::NetSim::parse(&netsim)?;
            acb_net::server::run(acb_net::server::ServeConfig { map, schemas, exe, port, players, snapshot_every, netsim, at, log, duration, rules, crowd: acb_net::proto::CrowdConfig { count: crowd, seed: crowd_seed } })?;
        }
        Cmd::Ls { forge } => {
            let forge = Forge::open(forge)?;
            for e in forge.entries() {
                println!("{:6} {:#010x} {:10} {}", e.index, e.id, e.length, e.name);
            }
        }
        Cmd::Census { forge, top } => census(forge, top)?,
        Cmd::Digest { forge } => digest(forge)?,
        Cmd::Animplay { forge, name, out, schemas, frames, stride } => animplay(forge, &name, out, schemas, frames, stride)?,
        Cmd::Rootmotion { forge, names, schemas, exe, stride } => rootmotion(forge, &names, schemas, exe, stride)?,
        Cmd::Animstats { forge, schemas, exe } => animstats(forge, schemas, exe)?,
        Cmd::Shaderlab { forge, schemas, sample_dir } => shaderlab(forge, schemas, sample_dir)?,
        Cmd::Shaders { forge, name, schemas, top, fields, disasm } => shaders(forge, &name, schemas, top, fields, disasm)?,
        Cmd::Names { forge, type_name, needle, schemas } => {
            let schema = load_schema(&schemas)?;
            let archive = acb_world::Archive::open(&forge, &schema)?;
            for u in archive.uids_of(&type_name) {
                let name = archive.loc(u).map(|l| l.name.as_str()).unwrap_or("?");
                if name.to_lowercase().contains(&needle.to_lowercase()) {
                    println!("{u:#010x} {name}");
                }
            }
        }
        Cmd::Charshot { forge, name, out, schemas, anim, frames, yaw, size } => {
            charshot(forge, &name, out, schemas, anim.as_deref(), frames, yaw, size)?
        }
        Cmd::Character { forge, name, schemas, obj, anim, t } => character(forge, &name, schemas, obj, anim.as_deref(), t)?,
        Cmd::Skin { forge, name, schemas } => skin(forge, &name, schemas)?,
        Cmd::Dump { forge, name, schemas, depth } => dump(forge, &name, schemas, depth)?,
        Cmd::Meshes { forge, schemas, top } => meshes(forge, schemas, top)?,
        Cmd::Cloth { forge, schemas, character: Some(ch), .. } => cloth_capsules(forge, &ch, schemas)?,
        Cmd::Cloth { forge, name, schemas, character: None } => cloth(forge, &name, schemas)?,
        Cmd::Navmesh { forge, schemas, obj } => navmesh(forge, schemas, obj)?,
        Cmd::Guidance { forge, schemas, obj } => guidance(forge, schemas, obj)?,
        Cmd::Posebounds { forge, builder, clip, schemas, exe, t } => posebounds(forge, &builder, &clip, schemas, exe, t)?,
        Cmd::Texture { forge, name, out, schemas } => {
            let schema = load_schema(&schemas)?;
            let archive = open_map(&forge, &schema)?;
            let uid = find_uid(&archive, "TextureMap", &name)?;
            let t = archive.texture(uid)?.ok_or_else(|| anyhow::anyhow!("undecodable texture"))?;
            let rgba = t.rgba();
            let (w, h) = (t.width as usize, t.height as usize);
            // colour on the left, alpha as grey on the right
            let mut img = vec![255u8; w * 2 * h * 4];
            for y in 0..h {
                for x in 0..w {
                    let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    let o = (y * w * 2 + x) * 4;
                    img[o..o + 3].copy_from_slice(&p[..3]);
                    let o = (y * w * 2 + w + x) * 4;
                    img[o..o + 3].copy_from_slice(&[p[3]; 3]);
                }
            }
            let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&out)?), (w * 2) as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()?.write_image_data(&img)?;
            println!("{}x{} {:?}, wrote {}", t.width, t.height, t.format, out.display());
        }
        Cmd::Ambiance { forge, schemas } => {
            let schema = load_schema(&schemas)?;
            let archive = open_map(&forge, &schema)?;
            match archive.ambiance() {
                Some(a) => println!("{a:#?}"),
                None => println!("no ambiance"),
            }
        }
        Cmd::Collision { forge, schemas, obj } => collision(forge, schemas, obj)?,
        Cmd::Pick { forge, schemas, eye, target, width, height, px } => pick(forge, schemas, &eye, &target, width, height, &px)?,
        Cmd::Shot { forge, out, schemas, eye, target, width, height, placeholder, bench } => {
            let shading =
                if placeholder { acb_render::ShadingMode::Placeholder } else { acb_render::ShadingMode::Retail };
            shot(forge, out, schemas, &eye, &target, width, height, shading, bench)?
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
        Cmd::Fastload { forge, schemas, top, dyn_key } => fastload(forge, schemas, top, dyn_key)?,
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

fn fastload(path: PathBuf, schemas: PathBuf, top: usize, dyn_key: Option<u32>) -> anyhow::Result<()> {
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
            let o_name = o.name.clone();
            let slot = per_type.entry(o.type_hash).or_default();
            match codec.decode(&o.payload) {
                Ok(root) => {
                    slot.0 += 1;
                    root.obj.walk(&mut |o| {
                        nested += 1;
                        if let Some(k) = dyn_key {
                            for d in o.dyn_props.iter().flatten().filter(|d| d.name_hash == k) {
                                println!(
                                    "{:#010x} ({}) in {}: {:?}",
                                    o.id,
                                    schema.name_or_hash(o.type_hash),
                                    o_name,
                                    d.value.as_f32s().unwrap_or_default()
                                );
                            }
                        }
                    });
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
    bench: u32,
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
    if bench > 0 {
        let t = Instant::now();
        for _ in 0..bench {
            acb_render::render_to_rgba(&r, &scene, &camera, width, height);
        }
        println!("{:.2} ms per frame (with readback) over {bench}", t.elapsed().as_secs_f64() * 1000.0 / bench as f64);
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&out)?), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn pick(forge: PathBuf, schemas: PathBuf, eye: &[f32], target: &[f32], width: u32, height: u32, px: &[f32]) -> anyhow::Result<()> {
    use glam::{Mat4, Vec3, Vec4};
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    let camera = acb_render::Camera {
        eye: Vec3::from_slice(eye),
        target: Vec3::from_slice(target),
        fov_y: 60f32.to_radians(),
        aspect: width as f32 / height as f32,
    };
    let inv = camera.view_proj().inverse();
    let rays: Vec<(Vec3, Vec3)> = px
        .chunks_exact(2)
        .map(|p| {
            let x = 2.0 * (p[0] + 0.5) / width as f32 - 1.0;
            let y = 1.0 - 2.0 * (p[1] + 0.5) / height as f32;
            let un = |z: f32| {
                let v = inv * Vec4::new(x, y, z, 1.0);
                v.truncate() / v.w
            };
            let (a, b) = (un(0.0), un(1.0));
            (a, (b - a).normalize())
        })
        .collect();
    // nearest hit per ray: (t, entity, mesh, material, normal)
    let mut best: Vec<Option<(f32, u32, u32, u32, Vec3)>> = vec![None; rays.len()];
    let mut meshes: HashMap<u32, Option<acb_world::StaticMesh>> = HashMap::new();
    // what the renderer hides
    let mut hidden: HashMap<u32, bool> = HashMap::new();
    for inst in archive.instances().into_iter().filter(|i| !i.out_of_bounds) {
        let mesh = meshes.entry(inst.mesh).or_insert_with(|| archive.static_mesh(inst.mesh).ok().flatten());
        let Some(mesh) = mesh else { continue };
        let m = Mat4::from_cols_array(&inst.matrix);
        let world: Vec<Vec3> = mesh.positions.iter().map(|p| m.transform_point3(Vec3::from(*p))).collect();
        for sub in &mesh.submeshes {
            if *hidden.entry(sub.material).or_insert_with(|| archive.material(sub.material).is_some_and(|m| m.disabled)) {
                continue;
            }
            let r = sub.first_index as usize..(sub.first_index + sub.index_count) as usize;
            for t in mesh.indices[r].chunks_exact(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| world[i as usize]);
                let (e1, e2) = (b - a, c - a);
                for (ri, (o, d)) in rays.iter().enumerate() {
                    let p = d.cross(e2);
                    let det = e1.dot(p);
                    if det.abs() < 1e-12 {
                        continue;
                    }
                    let s = *o - a;
                    let u = s.dot(p) / det;
                    let q = s.cross(e1);
                    let v = d.dot(q) / det;
                    let tt = e2.dot(q) / det;
                    if u < 0.0 || v < 0.0 || u + v > 1.0 || tt <= 0.0 {
                        continue;
                    }
                    if best[ri].is_none_or(|b| tt < b.0) {
                        best[ri] = Some((tt, inst.entity, inst.mesh, sub.material, e1.cross(e2).normalize()));
                    }
                }
            }
        }
    }
    let name = |u: u32| archive.loc(u).map(|l| l.name.clone()).unwrap_or_else(|| format!("{u:#010x}"));
    for (p, b) in px.chunks_exact(2).zip(best) {
        match b {
            None => println!("{},{}: nothing", p[0], p[1]),
            Some((t, e, m, mat, n)) => {
                let template = archive
                    .root(mat)
                    .ok()
                    .and_then(|r| archive.field(&r.obj, "Template").and_then(|v| v.target_id()))
                    .map(name)
                    .unwrap_or_default();
                println!(
                    "{},{}: {:.1} m  entity {} ({e:#010x})  mesh {}  material {} ({mat:#010x})  template {}  normal {:.2?}",
                    p[0], p[1], t, name(e), name(m), name(mat), template, n.to_array()
                );
            }
        }
    }
    Ok(())
}

fn cloth_capsules(forge: PathBuf, builder: &str, schemas: PathBuf) -> anyhow::Result<()> {
    use glam::{Mat4, Vec3};
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    let uid = find_uid(&archive, "EntityBuilder", builder)?;
    let c = acb_anim::character::Character::load_built(&archive, uid).ok_or_else(|| anyhow::anyhow!("not a character"))?;
    let globals = c.skeleton.globals(&c.skeleton.bind_pose());
    let index: HashMap<u32, usize> = c.skeleton.bones.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    // which local axis child bones sit along (the bone direction)
    let mut axes = [0usize; 6];
    for bone in c.skeleton.bones.iter().filter(|b| b.parent.is_some() && b.local_pos.length() > 0.05) {
        let v = bone.local_pos.normalize();
        let i = (0..3).max_by(|&a, &b| v[a].abs().total_cmp(&v[b].abs())).unwrap();
        axes[i * 2 + usize::from(v[i] < 0.0)] += 1;
    }
    println!("child offsets by axis (+X -X +Y -Y +Z -Z): {axes:?}");
    for (ci, cl) in c.cloths.iter().enumerate() {
        let p = c.cloth_targets(ci, &globals);
        println!("cloth {ci}: {} particles, {} capsules", p.len(), cl.lod.capsules.len());
        // segment conventions in the capsule frame
        let conv: [(&str, fn(f32) -> (Vec3, Vec3)); 6] = [
            ("+X from origin", |h| (Vec3::ZERO, Vec3::X * h)),
            ("X centred", |h| (Vec3::X * -h / 2.0, Vec3::X * h / 2.0)),
            ("+Y from origin", |h| (Vec3::ZERO, Vec3::Y * h)),
            ("Y centred", |h| (Vec3::Y * -h / 2.0, Vec3::Y * h / 2.0)),
            ("+Z from origin", |h| (Vec3::ZERO, Vec3::Z * h)),
            ("Z centred", |h| (Vec3::Z * -h / 2.0, Vec3::Z * h / 2.0)),
        ];
        for (label, f) in conv {
            let (mut pen, mut depth) = (0, 0f32);
            for k in &cl.lod.capsules {
                let Some(&b) = index.get(&k.bone_id) else { continue };
                let m: Mat4 = globals[b] * k.local;
                let (a0, a1) = f(k.height);
                let (a, bb) = (m.transform_point3(a0), m.transform_point3(a1));
                for (i, q) in p.iter().enumerate() {
                    if cl.lod.flags.get(i) == Some(&1) {
                        continue;
                    }
                    let ab = bb - a;
                    let t = if ab.length_squared() > 0.0 { ((*q - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                    let d = q.distance(a + ab * t);
                    let r = k.radius + if std::env::var_os("ACBMP_CAPSULE_ONLY").is_some() { 0.0 } else { cl.lod.radius.get(i).copied().unwrap_or(0.0) };
                    if d < r {
                        pen += 1;
                        depth = depth.max(r - d);
                    }
                }
            }
            println!("  {label:16} {pen} penetrations, deepest {depth:.3} m");
        }
        for k in &cl.lod.capsules {
            let found = index.get(&k.bone_id).is_some();
            println!("  capsule bone {:#010x} {} h {:.2} r {:.2} at {:.2?}", k.bone_id, if found { "ok" } else { "MISSING" }, k.height, k.radius, k.local.w_axis.truncate().to_array());
        }
    }
    Ok(())
}

fn cloth(forge: PathBuf, needle: &str, schemas: PathBuf) -> anyhow::Result<()> {
    use acb_anim::cloth::ClothLod;
    use glam::Vec3;
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    for uid in archive.uids_of("Cloth") {
        let name = archive.loc(uid).map(|l| l.name.clone()).unwrap_or_default();
        if !name.contains(needle) {
            continue;
        }
        let Some(c) = ClothLod::load(&archive, uid) else {
            println!("{name}: no LOD with particles");
            continue;
        };
        let n = c.positions.len();
        let rest_err = c
            .constraints
            .iter()
            .map(|k| (c.positions[k.a as usize].distance(c.positions[k.b as usize]) - k.rest).abs())
            .fold(0f32, f32::max);
        let mut flags: std::collections::BTreeMap<u8, usize> = Default::default();
        c.flags.iter().for_each(|f| *flags.entry(*f).or_default() += 1);
        let range = |v: &[f32]| v.iter().fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        let wsum = c.bone_weights.iter().map(|w| w.element_sum()).fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        let mapped = c.mappings.iter().filter(|m| m.skinned).count();
        if std::env::var_os("ACBMP_CLOTH_DEBUG").is_some() {
            for f in [1u8, 2] {
                let sel = |v: &[f32]| -> Vec<f32> { v.iter().zip(&c.flags).filter(|(_, g)| **g == f).map(|(x, _)| *x).collect() };
                println!("  flag {f}: skin {:?} rigidity {:?} radius {:?}", range(&sel(&c.skin_factor)), range(&sel(&c.rigidity)), range(&sel(&c.radius)));
            }
            let same = c.skin_factor.iter().zip(&c.rigidity).filter(|(a, b)| (*a - *b).abs() < 1e-6).count();
            println!("  skin == rigidity for {same}/{n}");
        }
        let mesh = archive.skinned_mesh(c.visual_mesh).ok().flatten();
        let fit = mesh.as_ref().map(|m| {
            let vis = c.visual_positions(&c.positions);
            let errs: Vec<f32> = vis
                .iter()
                .zip(&m.positions)
                .zip(&c.mappings)
                .filter(|(_, mp)| !mp.skinned)
                .map(|((a, b), _)| a.distance(Vec3::from(*b)))
                .collect();
            // variant: offset along the interpolated particle normals
            if std::env::var_os("ACBMP_CLOTH_DEBUG").is_some() {
                for (i, (mp, v)) in c.mappings.iter().zip(&m.positions).enumerate().filter(|(_, (mp, _))| !mp.skinned) {
                    let q = mp.indices.map(|j| c.positions.get(j as usize).copied());
                    let nq = mp.indices.map(|j| c.normals.get(j as usize).copied());
                    let base: Vec3 = q.iter().zip(mp.weights).filter_map(|(x, w)| x.map(|x| x * w)).sum();
                    let ni: Vec3 = nq.iter().zip(mp.weights).filter_map(|(x, w)| x.map(|x| x * w)).sum();
                    let target = Vec3::from(*v);
                    let e_tri = vis[i].distance(target);
                    if e_tri > 0.01 {
                        println!(
                            "  v{i}: idx {:?} w {:.2?} off {:.3} | tri err {e_tri:.3} interp-normal err {:.3} no-offset err {:.3}",
                            mp.indices, mp.weights, mp.offset, (base + ni.normalize_or_zero() * mp.offset).distance(target), base.distance(target)
                        );
                    }
                }
            }
            let max = errs.iter().copied().fold(0f32, f32::max);
            let mean = errs.iter().sum::<f32>() / errs.len().max(1) as f32;
            let map = c.match_visual(m);
            let nerr: Vec<f32> = map
                .iter()
                .zip(&m.positions)
                .filter_map(|(k, p)| k.map(|k| vis[k].distance(Vec3::from(*p))))
                .collect();
            let unmatched = map.iter().filter(|k| k.is_none()).count();
            let nmax = nerr.iter().copied().fold(0f32, f32::max);
            format!(
                "visual {} verts (mesh {}), in-order fit mean {mean:.4} max {max:.4} m; matched max {nmax:.4} m, {unmatched} unmatched",
                vis.len(),
                m.positions.len()
            )
        });
        println!(
            "{name}: {n} particles, {} constraints (rest err {rest_err:.4}), {} tris, {} bones, flags {flags:?}, skin {:?}, rigidity {:?}, radius {:?}, bone w sum {wsum:?}, {mapped} skinned mappings, iters {}, {}",
            c.constraints.len(),
            c.indices.len() / 3,
            c.bones.len(),
            range(&c.skin_factor),
            range(&c.rigidity),
            range(&c.radius),
            c.settings.iterations,
            fit.unwrap_or_else(|| "no visual mesh".into())
        );
    }
    Ok(())
}

fn navmesh(forge: PathBuf, schemas: PathBuf, obj: Option<PathBuf>) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let meshes = archive.navmeshes();
    let tris: usize = meshes.iter().map(|m| m.triangles.len()).sum();
    let bad_nb = meshes
        .iter()
        .flat_map(|m| m.neighbors.iter().flatten().map(move |n| (*n, m.triangles.len())))
        .filter(|(n, len)| *n != u16::MAX && *n as usize >= *len)
        .count();
    if std::env::var_os("ACBMP_NAV_DEBUG").is_some() {
        for m in meshes.iter().filter(|m| m.triangles.len() > 20).take(2) {
            println!("mesh with {} tris, {} verts", m.triangles.len(), m.vertices.len());
            for (i, (t, n)) in m.triangles.iter().zip(&m.neighbors).enumerate().take(12) {
                println!("  t{i} {t:?} nb {:04x?}", n);
            }
        }
    }
    let managers: std::collections::HashSet<u32> = meshes.iter().map(|m| m.manager).collect();
    println!(
        "{} managers, {} navmeshes ({} inactive), {tris} triangles, {bad_nb} out-of-range neighbours",
        managers.len(),
        meshes.len(),
        meshes.iter().filter(|m| !m.active).count()
    );
    let g = archive.nav_graph();
    // connected components
    let mut comp = vec![u32::MAX; g.triangles.len()];
    let mut sizes = Vec::new();
    for s in 0..g.triangles.len() {
        if comp[s] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        let mut stack = vec![s];
        let mut n = 0;
        comp[s] = id;
        while let Some(t) = stack.pop() {
            n += 1;
            for (nb, _) in &g.links[t] {
                if comp[*nb as usize] == u32::MAX {
                    comp[*nb as usize] = id;
                    stack.push(*nb as usize);
                }
            }
        }
        sizes.push(n);
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    println!(
        "graph: {} vertices, {} triangles, {} components (largest {:?})",
        g.vertices.len(),
        g.triangles.len(),
        sizes.len(),
        &sizes[..sizes.len().min(5)]
    );
    let flows = archive.crowd_flows();
    let ids: std::collections::HashSet<u32> = flows.iter().map(|f| f.entity).collect();
    let links: usize = flows.iter().map(|f| f.head.len() + f.tail.len()).sum();
    let dangling = flows.iter().flat_map(|f| f.head.iter().chain(&f.tail)).filter(|e| !ids.contains(e)).count();
    let length: f32 = flows
        .iter()
        .flat_map(|f| f.points.windows(2).map(|w| glam::Vec3::from(w[0]).distance(glam::Vec3::from(w[1]))))
        .sum();
    println!(
        "{} crowd flows ({} one-way, {} wander), {length:.0} m of lanes, {links} links ({dangling} to unknown flows), widths {:?}",
        flows.len(),
        flows.iter().filter(|f| !f.bidirectional).count(),
        flows.iter().filter(|f| f.wander).count(),
        flows.iter().map(|f| f.width).fold((f32::MAX, f32::MIN), |(a, b), w| (a.min(w), b.max(w)))
    );
    if let Some(path) = obj {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(std::fs::File::create(&path)?);
        for v in &g.vertices {
            writeln!(w, "v {} {} {}", v[0], v[2], -v[1])?;
        }
        for t in &g.triangles {
            writeln!(w, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1)?;
        }
        println!("wrote {}", path.display());
    }
    println!("{:.1}s", t0.elapsed().as_secs_f64());
    Ok(())
}

fn guidance(forge: PathBuf, schemas: PathBuf, obj: Option<PathBuf>) -> anyhow::Result<()> {
    use glam::Vec3;
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let edges = archive.guidance_edges();
    let mut types: std::collections::BTreeMap<u32, usize> = Default::default();
    edges.iter().for_each(|e| *types.entry(e.sub_type).or_default() += 1);
    let length: f32 = edges.iter().map(|e| Vec3::from(e.a).distance(Vec3::from(e.b))).sum();
    let ents: std::collections::HashSet<u32> = edges.iter().map(|e| e.entity).collect();
    println!("{} edges on {} entities, {length:.0} m; sub types {types:?}", edges.len(), ents.len());
    // how far edge midpoints are from the collision surface
    let (world, _) = archive.world_collision();
    let tris: Vec<[Vec3; 3]> = world.triangles.iter().map(|t| t.map(|i| Vec3::from(world.vertices[i as usize]))).collect();
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        let lo = t[0].min(t[1]).min(t[2]);
        let hi = t[0].max(t[1]).max(t[2]);
        for x in (lo.x / 2.0).floor() as i32..=(hi.x / 2.0).floor() as i32 {
            for y in (lo.y / 2.0).floor() as i32..=(hi.y / 2.0).floor() as i32 {
                grid.entry((x, y)).or_default().push(i);
            }
        }
    }
    let point_tri = |p: Vec3, [a, b, c]: [Vec3; 3]| {
        // distance from p to triangle (closest point, Ericson)
        let (ab, ac, ap) = (b - a, c - a, p - a);
        let (d1, d2) = (ab.dot(ap), ac.dot(ap));
        if d1 <= 0.0 && d2 <= 0.0 {
            return p.distance(a);
        }
        let bp = p - b;
        let (d3, d4) = (ab.dot(bp), ac.dot(bp));
        if d3 >= 0.0 && d4 <= d3 {
            return p.distance(b);
        }
        let cp = p - c;
        let (d5, d6) = (ab.dot(cp), ac.dot(cp));
        if d6 >= 0.0 && d5 <= d6 {
            return p.distance(c);
        }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            return p.distance(a + ab * (d1 / (d1 - d3)));
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            return p.distance(a + ac * (d2 / (d2 - d6)));
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            return p.distance(b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6))));
        }
        let denom = 1.0 / (va + vb + vc);
        p.distance(a + ab * (vb * denom) + ac * (vc * denom))
    };
    let mut dist: Vec<f32> = edges
        .iter()
        .map(|e| {
            let m = (Vec3::from(e.a) + Vec3::from(e.b)) / 2.0;
            grid.get(&((m.x / 2.0).floor() as i32, (m.y / 2.0).floor() as i32))
                .map(|l| l.iter().map(|&i| point_tri(m, tris[i])).fold(f32::MAX, f32::min))
                .unwrap_or(f32::MAX)
        })
        .collect();
    let near = dist.iter().filter(|d| **d < 0.05).count();
    let none = dist.iter().filter(|d| **d == f32::MAX).count();
    println!("{near} edges within 5 cm of collision, {none} over cells without any collision");
    dist.sort_by(f32::total_cmp);
    let q = |f: f32| dist.get(((dist.len() as f32 * f) as usize).min(dist.len().saturating_sub(1))).copied().unwrap_or(0.0);
    println!("edge midpoint to collision: median {:.3} m, 90% {:.3} m, 99% {:.3} m", q(0.5), q(0.9), q(0.99));
    if let Some(path) = obj {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(std::fs::File::create(&path)?);
        for e in &edges {
            writeln!(w, "v {} {} {}", e.a[0], e.a[2], -e.a[1])?;
            writeln!(w, "v {} {} {}", e.b[0], e.b[2], -e.b[1])?;
        }
        for i in 0..edges.len() {
            writeln!(w, "l {} {}", 2 * i + 1, 2 * i + 2)?;
        }
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn posebounds(forge: PathBuf, builder: &str, clip: &str, schemas: PathBuf, exe: PathBuf, t: f32) -> anyhow::Result<()> {
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    let c = acb_anim::character::Character::load_built(&archive, find_uid(&archive, "EntityBuilder", builder)?)
        .ok_or_else(|| anyhow::anyhow!("not a character"))?;
    let builtins = acb_anim::BuiltinTracks::from_executable(&std::fs::read(&exe)?);
    let clip = acb_anim::pose::Clip::load_with(&archive, find_uid(&archive, "Animation", clip)?, builtins.as_ref())
        .ok_or_else(|| anyhow::anyhow!("clip"))?;
    let g = c.skeleton.globals(&clip.sample(&c.skeleton, t));
    let p: Vec<glam::Vec3> = g.iter().map(|m| m.w_axis.truncate()).collect();
    let top = p.iter().enumerate().max_by(|a, b| a.1.z.total_cmp(&b.1.z)).unwrap();
    let front = p.iter().enumerate().max_by(|a, b| a.1.y.total_cmp(&b.1.y)).unwrap();
    let low = p.iter().enumerate().min_by(|a, b| a.1.z.total_cmp(&b.1.z)).unwrap();
    let id = |i: usize| c.skeleton.bones[i].id;
    println!("highest bone {:#010x} at {:.2?}", id(top.0), top.1.to_array());
    println!("lowest bone {:#010x} at {:.2?}", id(low.0), low.1.to_array());
    println!("most forward bone {:#010x} at {:.2?}", id(front.0), front.1.to_array());
    let mut zs: Vec<(f32, usize)> = p.iter().enumerate().map(|(i, v)| (v.z, i)).collect();
    zs.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (z, i) in zs.iter().take(6) {
        println!("  {:#010x} z {z:.2} at {:.2?}", id(*i), p[*i].to_array());
    }
    Ok(())
}

fn collision(forge: PathBuf, schemas: PathBuf, obj: Option<PathBuf>) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let colliders = archive.colliders();
    let mut rows: std::collections::BTreeMap<(String, String, u32), (usize, HashMap<u32, ()>)> = Default::default();
    for c in &colliders {
        let key = (schema.name_or_hash(c.component), archive.type_name(c.shape).unwrap_or_else(|| "?".into()), c.layer);
        let row = rows.entry(key).or_default();
        row.0 += 1;
        row.1.insert(c.shape, ());
    }
    println!("{} colliders", colliders.len());
    if std::env::var_os("ACBMP_COMPONENTS").is_some() {
        // component types across the map's entities (roots and grouped)
        let mut census: std::collections::BTreeMap<String, usize> = Default::default();
        let mut count = |e: &anvil_fastload::Obj| {
            for c in archive.field(e, "Components").map(|v| v.items()).unwrap_or_default().iter().filter_map(|v| v.as_obj()) {
                *census.entry(schema.name_or_hash(c.type_hash)).or_default() += 1;
                if archive.is_type(c, "GuidanceSystem") && std::env::var_os("ACBMP_GUIDANCE").is_some() {
                    println!("  GuidanceSystem on {:#010x} {}", e.id, archive.loc(e.id).map(|l| l.name.as_str()).unwrap_or("(inline)"));
                    dump_obj(&archive, &schema, c, 2, 3);
                }
                if archive.is_type(c, "CrowdFlow") && std::env::var_os("ACBMP_FLOWS").is_some() {
                    println!("  CrowdFlow on {:#010x} {}", e.id, archive.loc(e.id).map(|l| l.name.as_str()).unwrap_or("(inline)"));
                    dump_obj(&archive, &schema, c, 2, 2);
                }
                if archive.is_type(c, "MultiInertComponent") {
                    let shape = archive.field(c, "Shape");
                    println!(
                        "  MultiInert on {:#010x}: shape {:?}",
                        e.id,
                        shape.map(|v| match v.as_obj() {
                            Some(o) => {
                                let list = |n: &str| {
                                    let items = archive.field(o, n).map(|v| v.items()).unwrap_or_default();
                                    let first: Vec<String> = items
                                        .iter()
                                        .take(3)
                                        .map(|i| match i.as_obj() {
                                            Some(x) => format!("<{}>", schema.name_or_hash(x.type_hash)),
                                            None => format!(
                                                "{:?}={}",
                                                i.target_id(),
                                                i.target_id().and_then(|t| archive.type_name(t)).unwrap_or_default()
                                            ),
                                        })
                                        .collect();
                                    format!("{n} {} {first:?}", items.len())
                                };
                                format!(
                                    "inline {} #{:#x}: {}; {}; {}",
                                    schema.name_or_hash(o.type_hash),
                                    o.id,
                                    list("ContainedEntities"),
                                    list("ContainedShapes"),
                                    list("ContainedInerts")
                                )
                            }
                            None => format!("{:?}", v.target_id()),
                        })
                    );
                }
            }
        };
        for uid in archive.uids_of_in("Entity", 0) {
            if let Ok(r) = archive.root(uid) {
                count(&r.obj);
            }
        }
        for uid in archive.uids_of_in("EntityGroup", 0) {
            if let Ok(r) = archive.root(uid) {
                r.obj.walk(&mut |o| {
                    if archive.is_type(o, "Entity") {
                        count(o)
                    }
                });
            }
        }
        for (k, v) in &census {
            println!("  {v:6} {k}");
        }
    }
    if std::env::var_os("ACBMP_LIST").is_some() {
        for c in &colliders {
            let name = |u: u32| archive.loc(u).map(|l| l.name.clone()).unwrap_or_else(|| format!("{u:#010x}"));
            println!("  layer {:2} {:24} {} -> {}", c.layer, schema.name_or_hash(c.component), name(c.entity), name(c.shape));
        }
    }
    for ((comp, shape, layer), (n, shapes)) in &rows {
        println!("{n:6} {:5} shapes  {comp:24} {shape:20} layer {layer}", shapes.len());
    }
    let flags = |f: fn(&acb_world::Collider) -> bool| colliders.iter().filter(|c| f(c)).count();
    println!(
        "ground {}  roof {}  out-of-bounds {}  not fixed {}",
        flags(|c| c.is_ground),
        flags(|c| c.is_roof),
        flags(|c| c.out_of_bounds),
        flags(|c| !c.fixed)
    );
    let (world, skipped) = archive.world_collision();
    println!("{} vertices, {} triangles; skipped {skipped:?}", world.vertices.len(), world.triangles.len());
    if let Some(path) = obj {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(std::fs::File::create(&path)?);
        for v in &world.vertices {
            writeln!(w, "v {} {} {}", v[0], v[2], -v[1])?;
        }
        for t in &world.triangles {
            writeln!(w, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1)?;
        }
        println!("wrote {}", path.display());
    }
    println!("{:.1}s", t0.elapsed().as_secs_f64());
    Ok(())
}

fn rootmotion(forge: PathBuf, names: &[String], schemas: PathBuf, exe: PathBuf, stride: bool) -> anyhow::Result<()> {
    let schema = load_schema(&schemas)?;
    let archive = open_map(&forge, &schema)?;
    let builtins = acb_anim::BuiltinTracks::from_executable(&std::fs::read(&exe)?)
        .ok_or_else(|| anyhow::anyhow!("no builtin track table in {}", exe.display()))?;
    let uids = archive.uids_of("Animation");
    for name in names {
        let Some(uid) = uids.iter().copied().find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name))) else {
            println!("{name}: not found");
            continue;
        };
        let clip = acb_anim::pose::Clip::load_with(&archive, uid, Some(&builtins)).ok_or_else(|| anyhow::anyhow!("{name}: decode"))?;
        let clip = match acb_anim::pose::stride_partner(name).filter(|_| stride) {
            Some(other) => {
                let u = uids.iter().copied().find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(&other)));
                let other = u.and_then(|u| acb_anim::pose::Clip::load_with(&archive, u, Some(&builtins))).ok_or_else(|| anyhow::anyhow!("{other}: not found"))?;
                print!("{name} + {} = ", archive.loc(other.uid).map(|l| l.name.as_str()).unwrap_or("?"));
                clip.then(&other)
            }
            None => clip,
        };
        let end = clip.root_motion(clip.length);
        let (_, r0, p0) = clip.root_motion(0.0).to_scale_rotation_translation();
        print!("start ({:.2}, {:.2}, {:.2}) yaw {:.0}; ", p0.x, p0.y, p0.z, r0.to_euler(glam::EulerRot::ZXY).0.to_degrees());
        let (_, rot, pos) = end.to_scale_rotation_translation();
        let (yaw, _, _) = rot.to_euler(glam::EulerRot::ZXY);
        // speed in 8 slices
        let n = 8;
        let speeds: Vec<String> = (0..n)
            .map(|i| {
                let (a, b) = (clip.length * i as f32 / n as f32, clip.length * (i + 1) as f32 / n as f32);
                let d = clip.root_motion(a).w_axis.truncate().distance(clip.root_motion(b).w_axis.truncate());
                format!("{:.2}", d / (b - a).max(1e-6))
            })
            .collect();
        let yaws: Vec<String> = (0..=n)
            .map(|i| {
                let (_, r, _) = clip.root_motion(clip.length * i as f32 / n as f32).to_scale_rotation_translation();
                format!("{:.0}", r.to_euler(glam::EulerRot::ZXY).0.to_degrees())
            })
            .collect();
        // Reference bone (pelvis in entity space): yaw at start and end
        let reference: Vec<String> = clip
            .tracks
            .iter()
            .filter(|(id, _)| *id == 0x2c52cbb0)
            .filter_map(|(_, t)| t.quats())
            .map(|q| {
                let y = |q: glam::Quat| q.to_euler(glam::EulerRot::ZXY).0.to_degrees();
                format!("ref yaw {:.0}..{:.0}", y(q[0]), y(*q.last().unwrap()))
            })
            .collect();
        let reference_pos: Vec<String> = clip
            .tracks
            .iter()
            .filter(|(id, _)| *id == 0x2c52cbb0)
            .filter_map(|(_, t)| t.vec3s())
            .map(|v| format!("ref pos {:.2?}..{:.2?}", v[0].to_array(), v.last().unwrap().to_array()))
            .collect();
        print!("{} {} ", reference.join(" "), reference_pos.join(" "));
        println!(
            "{name}: {:.3}s  move ({:.2}, {:.2}, {:.2})  yaw {:.0} [{}]  m/s [{}]",
            clip.length,
            pos.x,
            pos.y,
            pos.z,
            yaw.to_degrees(),
            yaws.join(" "),
            speeds.join(" ")
        );
    }
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
        .filter(|u| {
            // `0x...` selects one root by uid (nameless globals)
            needle.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok()).map_or_else(
                || archive.loc(*u).is_some_and(|l| l.name.contains(needle)),
                |want| *u == want,
            )
        })
        .collect();
    uids.sort();
    uids.dedup();
    for uid in uids.into_iter().take(std::env::var("ACBMP_DUMP_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(8)) {
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
            // ACBMP_DUMP_ITEMS=n: expand only the first n items of each array
            let max = std::env::var("ACBMP_DUMP_ITEMS").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
            for (i, it) in items.iter().enumerate().take(max) {
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
                    let lk = archive.field(p, "LightingKey").and_then(Value::as_u32);
                    let lk = lk.map(|l| format!(" lighting {l:#x}")).unwrap_or_default();
                    println!("    key {key:#010x}{lk} code {} B ver {ver:#010x} params {}", code.len(), params.join(" "));
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

fn animstats(forge: PathBuf, schemas: PathBuf, exe: Option<PathBuf>) -> anyhow::Result<()> {
    use acb_anim::{parse_tracks, BuiltinTracks, Track};
    use anvil_fastload::Value;
    let builtins = match exe {
        Some(p) => Some(BuiltinTracks::from_executable(&std::fs::read(&p)?).ok_or_else(|| anyhow::anyhow!("no builtin track table in {}", p.display()))?),
        None => None,
    };
    let (mut b_ok, mut b_failed, mut b_late) = (0usize, 0usize, 0usize);
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
            let resolved;
            let t = match (t, &builtins) {
                (Track::Builtin(b), Some(bt)) => {
                    builtin += 1;
                    match bt.track(*b) {
                        Ok(d) => {
                            b_ok += 1;
                            // shared tracks must still fit the clip
                            if d.times.last().copied().unwrap_or(0) as f32 > length * 60.0 + 1.0 {
                                b_late += 1;
                            }
                            resolved = Track::Data(d);
                            &resolved
                        }
                        Err(_) => {
                            b_failed += 1;
                            continue;
                        }
                    }
                }
                (t, _) => t,
            };
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
                                if std::env::var_os("ACBMP_DEBUG_BUILTINS").is_some() {
                                    eprintln!("bad quat {:?}/{:?} {} keys", d.compression, d.format, qs.len());
                                }
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
    if builtins.is_some() {
        println!("built-in tracks: {b_ok} decoded, {b_failed} failed, {b_late} with keys past the clip end");
    }
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

fn animplay(forge: PathBuf, needle: &str, out: PathBuf, schemas: PathBuf, frames: usize, stride: bool) -> anyhow::Result<()> {
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
    let name = archive.loc(uid).map(|l| l.name.clone()).unwrap_or_default();
    let clip = match acb_anim::pose::stride_partner(&name).filter(|_| stride) {
        Some(other) => {
            let u = anims.iter().copied().find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(&other)));
            let other = u.and_then(|u| Clip::load(&archive, u)).ok_or_else(|| anyhow::anyhow!("{other}: not found"))?;
            println!("joined with {}", archive.loc(other.uid).map(|l| l.name.as_str()).unwrap_or("?"));
            clip.then(&other)
        }
        None => clip,
    };
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
            p.materials.iter().map(|m| name(m.0)).collect::<Vec<_>>().join(", "),
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

#[allow(clippy::too_many_arguments)]
fn charshot(
    forge: PathBuf,
    needle: &str,
    out: PathBuf,
    schemas: PathBuf,
    anim: Option<&str>,
    frames: usize,
    yaw: f32,
    size: u32,
) -> anyhow::Result<()> {
    use acb_anim::{character::Character, pose::Clip};
    let schema = load_schema(&schemas)?;
    let archive = acb_world::Archive::open(&forge, &schema)?;
    let name = |u: u32| archive.loc(u).map(|l| l.name.clone()).unwrap_or_else(|| format!("{u:#010x}"));
    // an EntityBuilder of that name builds its template; else the Entity
    let (uid, c) = match find_uid(&archive, "EntityBuilder", needle) {
        Ok(b) if archive.loc(b).is_some_and(|l| l.name == needle) => (b, Character::load_built(&archive, b)),
        _ => {
            let uid = find_uid(&archive, "Entity", needle)?;
            (uid, Character::load(&archive, uid))
        }
    };
    let c = c.ok_or_else(|| anyhow::anyhow!("not a character"))?;
    let clip = match anim {
        Some(a) => Some(Clip::load(&archive, find_uid(&archive, "Animation", a)?).ok_or_else(|| anyhow::anyhow!("animation did not decode"))?),
        None => None,
    };

    let (device, queue) = pollster::block_on(acb_render::headless_device())?;
    let r = acb_render::Renderer::new(device, queue, acb_render::wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut scene = acb_render::Scene::empty(&r);
    let handles: Vec<Option<usize>> = c.parts.iter().map(|p| scene.add_skinned(&r, &archive, &p.mesh, &p.materials)).collect();
    if let Some(rs) = scene.retail_stats() {
        println!("{}: {} parts; retail {} materials, {} programs, fallbacks {:?}", name(uid), c.parts.len(), rs.materials, rs.programs, rs.fallbacks);
    }
    for p in &c.parts {
        let unmapped = p.mesh.bones.iter().zip(&p.bone_map).filter(|(_, m)| m.is_none()).count();
        let mats: Vec<String> = p.materials.iter().map(|m| name(m.0)).collect();
        println!("  {} {} verts, {} bones ({} unmapped) {:?}", name(p.mesh_uid), p.mesh.positions.len(), p.mesh.bones.len(), unmapped, mats);
        if std::env::var_os("ACBMP_DEBUG_BONES").is_some() {
            let n = p.mesh.positions.len() as f32;
            let c = p.mesh.positions.iter().fold([0.0f32; 3], |a, v| [a[0] + v[0] / n, a[1] + v[1] / n, a[2] + v[2] / n]);
            println!("    mesh-space centroid {c:.3?}");
            let band = |lo: f32, hi: f32| {
                let ys: Vec<f32> = p.mesh.positions.iter().filter(|v| v[2] >= lo && v[2] < hi).map(|v| v[1]).collect();
                (ys.len(), ys.iter().cloned().fold(f32::MAX, f32::min), ys.iter().cloned().fold(f32::MIN, f32::max))
            };
            println!("    y range z<0.04 {:.3?}, ankle z 0.10..0.14 {:.3?}", band(-1.0, 0.04), band(0.10, 0.14));
        }
        // bind-pose skinning should be one rigid transform for every bone
        if std::env::var_os("ACBMP_DEBUG_BONES").is_some() {
            let g = c.skeleton.globals(&c.skeleton.bind_pose());
            for ((b, m), inv) in p.mesh.bones.iter().zip(&p.bone_map).zip(&p.inverse_bind) {
                let skin = m.map(|m| g[m] * *inv).unwrap_or(glam::Mat4::IDENTITY);
                println!("    bone {:#010x} -> {:?}: t {:.3?}", b.id, m, skin.w_axis.truncate());
            }
        }
    }

    if let (Some(clip), true) = (&clip, std::env::var_os("ACBMP_DEBUG_BONES").is_some()) {
        let r = archive.root(clip.uid)?;
        let raw = acb_anim::parse_tracks(r.obj.custom.as_ref().unwrap())?;
        let ids: Vec<u32> = archive
            .field(&r.obj, "AnimTrackData")
            .and_then(anvil_fastload::Value::as_obj)
            .and_then(|d| archive.field(d, "AnimTrackDataMapping"))
            .map(|v| v.items().iter().filter_map(|m| m.as_obj()).map(|m| archive.field(m, "TrackID").and_then(|v| v.as_u32()).unwrap_or(0)).collect())
            .unwrap_or_default();
        for (id, t) in ids.iter().zip(&raw).filter(|(id, _)| [0, 2, 0x2c52cbb0, 0x21895793].contains(*id)) {
            match t {
                acb_anim::Track::Builtin(b) => {
                    let exe = std::env::var_os("ACBMP_EXE").and_then(|p| std::fs::read(p).ok());
                    match exe.as_deref().and_then(acb_anim::BuiltinTracks::from_executable).map(|bt| bt.track(*b)) {
                        Some(Ok(d)) => {
                            println!("  track {id:#x}: builtin {b:#x} {:?} times {:?}", d.compression, d.times);
                            if let Some(v) = d.vec3s() {
                                println!("    {v:.3?}");
                            }
                            if let Some(q) = d.quats() {
                                let yaw: Vec<f32> = q.iter().map(|q| q.to_euler(glam::EulerRot::ZYX).0.to_degrees()).collect();
                                println!("    yaw {yaw:.1?}");
                            }
                        }
                        _ => println!("  track {id:#x}: builtin {b:#x}"),
                    }
                }
                acb_anim::Track::Data(d) => println!(
                    "  track {id:#x}: {:?} {} keys, first {:?} last {:?}",
                    d.compression.kind(),
                    d.times.len(),
                    d.vec3s().map(|v| v[0]).map(|v| format!("{v:.3?}")).or(d.quats().map(|q| format!("{:.3?}", q[0]))),
                    d.vec3s().map(|v| v[v.len() - 1]).map(|v| format!("{v:.3?}")).or(d.quats().map(|q| format!("{:.3?}", q[q.len() - 1]))),
                ),
            }
        }
    }
    let frames = frames.max(1);
    let mut strip = vec![0u8; (size * size * 4) as usize * frames];
    let stride = (size * 4) as usize * frames;
    for f in 0..frames {
        let local = match &clip {
            Some(clip) => clip.sample(&c.skeleton, if frames > 1 { clip.length * f as f32 / (frames - 1) as f32 } else { 0.0 }),
            None => c.skeleton.bind_pose(),
        };
        let globals = c.skeleton.globals(&local);
        if std::env::var_os("ACBMP_DEBUG_BONES").is_some() {
            // model-space extent of the skinned vertices
            let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
            for (p, skin) in c.parts.iter().zip(c.skin_matrices(&globals)) {
                for sm in &p.mesh.submeshes {
                    let range = sm.first_vertex as usize..(sm.first_vertex + sm.vertex_count) as usize;
                    for v in range.filter(|&v| v < p.mesh.positions.len()) {
                        let mut out = glam::Vec3::ZERO;
                        for k in 0..4 {
                            let slot = p.mesh.joints[v][k] as usize;
                            let Some(&bone) = sm.palette.get(slot) else { continue };
                            let m = skin.get(bone as usize).copied().unwrap_or(glam::Mat4::IDENTITY);
                            out += p.mesh.weights[v][k] * m.transform_point3(glam::Vec3::from(p.mesh.positions[v]));
                        }
                        lo = lo.min(out);
                        hi = hi.max(out);
                    }
                }
            }
            println!("  frame {f}: skinned extent {lo:.2?} .. {hi:.2?}");
        }
        for (h, skin) in handles.iter().zip(c.skin_matrices(&globals)) {
            if let Some(h) = h {
                scene.set_skinned_pose(*h, glam::Mat4::IDENTITY, &skin);
            }
        }
        // frame the bones the parts are skinned to (add-on skeletons carry
        // helper bones far from the body)
        let used: std::collections::BTreeSet<usize> = c.parts.iter().flat_map(|p| p.bone_map.iter().flatten().copied()).collect();
        let (lo, hi) = used.iter().map(|&i| &globals[i]).fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(lo, hi), g| {
            (lo.min(g.w_axis.truncate()), hi.max(g.w_axis.truncate()))
        });
        let target = (lo + hi) * 0.5;
        let radius = ((hi - lo).length() * 0.5).max(0.9) + 0.25;
        let fov = 40f32.to_radians();
        let dir = glam::Vec3::new(yaw.to_radians().sin(), -yaw.to_radians().cos(), 0.25).normalize();
        let camera = acb_render::Camera { eye: target + dir * radius / (fov * 0.5).tan(), target, fov_y: fov, aspect: 1.0 };
        let rgba = acb_render::render_to_rgba(&r, &scene, &camera, size, size);
        for y in 0..size as usize {
            let row = &rgba[y * size as usize * 4..(y + 1) * size as usize * 4];
            let at = y * stride + f * size as usize * 4;
            strip[at..at + row.len()].copy_from_slice(row);
        }
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&out)?), size * frames as u32, size);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&strip)?;
    println!("wrote {}", out.display());
    Ok(())
}
