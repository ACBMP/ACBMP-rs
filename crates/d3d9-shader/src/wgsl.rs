//! SM3 to WGSL translation.
//!
//! Binding model (shared with the host renderer):
//! - group 0, binding 0 (vertex) / 1 (pixel): [`Consts`] uniform struct with
//!   the stage's `c#` (256 / 224 vec4), `i#` and `b#` registers. The host
//!   uploads `def` values too, so relative addressing sees them.
//! - group 1: sampler register `s#` is texture binding `2*s` and sampler
//!   binding `2*s + 1` (vertex samplers start at binding 64).
//! - vertex inputs and inter-stage varyings use the fixed locations of
//!   [`vertex_input_location`] and [`varying_location`].
//!
//! Every value is a `vec4<f32>`, as on D3D9 hardware.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::disasm::reg_name;
use crate::parse::*;
use crate::Error;

pub const VS_CONSTS: usize = 256;
pub const PS_CONSTS: usize = 224;
pub const VS_SAMPLER_BASE: u32 = 64;

/// Vertex attribute location of a `dcl_<usage><index> v#` input. Locations
/// reach 23, so the device needs `max_vertex_attributes >= 24` for shaders
/// that use the high texcoord streams.
pub fn vertex_input_location(usage: Usage, index: u32) -> Option<u32> {
    use Usage::*;
    Some(match (usage, index) {
        (Position, 0) => 0,
        (BlendWeight, 0) => 1,
        (BlendIndices, 0) => 2,
        (Normal, 0) => 3,
        (Color, n @ 0..=1) => 4 + n,
        (Tangent, 0) => 6,
        (Binormal, 0) => 7,
        (TexCoord, n @ 0..=15) => 8 + n,
        _ => return None,
    })
}

/// Location of a vertex-to-pixel varying.
pub fn varying_location(usage: Usage, index: u32) -> Option<u32> {
    use Usage::*;
    Some(match (usage, index) {
        (TexCoord, n @ 0..=9) => n,
        (Color, n @ 0..=1) => 10 + n,
        (Fog, 0) => 12,
        (Normal, 0) => 13,
        (Tangent, 0) => 14,
        (Binormal, 0) => 15,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct IoSlot {
    pub usage: Usage,
    pub index: u32,
    pub location: u32,
}

#[derive(Debug, Clone)]
pub struct Translated {
    pub wgsl: String,
    pub entry: &'static str,
    pub inputs: Vec<IoSlot>,
    pub outputs: Vec<IoSlot>,
    pub samplers: Vec<(u32, SamplerType)>,
}

/// Translation knobs.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Pixel shader only: varyings the paired vertex shader writes. Inputs
    /// outside this set read as zero, since WGSL requires every fragment
    /// input to be produced by the vertex stage.
    pub available_varyings: Option<BTreeSet<(UsageKey, u32)>>,
}

/// Orderable stand-in for [`Usage`] in sets.
pub type UsageKey = u32;

pub fn usage_key(u: Usage) -> UsageKey {
    use Usage::*;
    match u {
        Position => 0,
        BlendWeight => 1,
        BlendIndices => 2,
        Normal => 3,
        PSize => 4,
        TexCoord => 5,
        Tangent => 6,
        Binormal => 7,
        TessFactor => 8,
        PositionT => 9,
        Color => 10,
        Fog => 11,
        Depth => 12,
        Sample => 13,
        Unknown(c) => 100 + c,
    }
}

const XYZW: [char; 4] = ['x', 'y', 'z', 'w'];

struct Gen<'p> {
    p: &'p Program,
    stage: Stage,
    out: String,
    indent: usize,
    loops: Vec<String>,
    next_id: usize,
    temps: BTreeSet<u32>,
    outputs_written: BTreeSet<(RegType, u32)>,
    uses_addr: bool,
    uses_pred: bool,
    defs: BTreeMap<u32, [f32; 4]>,
    defis: BTreeMap<u32, [i32; 4]>,
    defbs: BTreeMap<u32, bool>,
}

fn float(v: f32) -> String {
    if v.is_nan() {
        "0.0".into()
    } else if v.is_infinite() {
        if v > 0.0 { "3.4e38".into() } else { "-3.4e38".into() }
    } else {
        let s = format!("{v:?}");
        if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") }
    }
}

fn vec4f(v: [f32; 4]) -> String {
    format!("vec4<f32>({}, {}, {}, {})", float(v[0]), float(v[1]), float(v[2]), float(v[3]))
}

impl Gen<'_> {
    fn line(&mut self, s: impl AsRef<str>) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn consts(&self) -> &'static str {
        if self.stage == Stage::Vertex { "vsc" } else { "psc" }
    }

    fn max_consts(&self) -> usize {
        if self.stage == Stage::Vertex { VS_CONSTS } else { PS_CONSTS }
    }

    /// Integer index expression for a relative operand.
    fn rel_index(&self, r: &Relative, base: u32) -> Result<String, Error> {
        let addr = match r.reg {
            RegType::AddrOrTexture => format!("a{}.{}", r.num, XYZW[r.comp as usize]),
            RegType::Loop => self.loops.last().cloned().ok_or_else(|| Error::Malformed("aL outside loop".into()))?,
            other => return Err(Error::Unsupported(format!("relative via {other:?}"))),
        };
        Ok(format!("clamp({addr} + {base}, 0, {})", self.max_consts() - 1))
    }

    /// Unswizzled register value as vec4<f32>.
    fn reg(&mut self, reg: RegType, num: u32, rel: Option<&Relative>) -> Result<String, Error> {
        use RegType::*;
        Ok(match reg {
            Temp => {
                self.temps.insert(num);
                format!("r{num}")
            }
            Input => format!("v{num}"),
            Const => match rel {
                Some(r) => format!("{}.c[{}]", self.consts(), self.rel_index(r, num)?),
                None => match self.defs.get(&num) {
                    Some(v) => vec4f(*v),
                    None => format!("{}.c[{num}]", self.consts()),
                },
            },
            ConstInt => match self.defis.get(&num) {
                Some(v) => format!("vec4<f32>(vec4<i32>({}, {}, {}, {}))", v[0], v[1], v[2], v[3]),
                None => format!("vec4<f32>({}.i[{num}])", self.consts()),
            },
            ConstBool => format!("vec4<f32>(f32({}))", self.bool_reg(num)),
            Loop => {
                let l = self.loops.last().cloned().ok_or_else(|| Error::Malformed("aL outside loop".into()))?;
                format!("vec4<f32>(f32({l}))")
            }
            AddrOrTexture if self.stage == Stage::Vertex => {
                self.uses_addr = true;
                format!("vec4<f32>(a{num})")
            }
            Misc => {
                if num == 0 {
                    "vpos".into()
                } else {
                    "vface".into()
                }
            }
            Predicate => {
                self.uses_pred = true;
                format!("vec4<f32>(p{num})")
            }
            Output | RastOut | AttrOut | ColorOut | DepthOut => format!("{}", out_name(reg, num)),
            other => return Err(Error::Unsupported(format!("source register {other:?}"))),
        })
    }

    fn bool_reg(&self, num: u32) -> String {
        match self.defbs.get(&num) {
            Some(b) => format!("{b}"),
            None => format!("({}.b[{num}].x != 0u)", self.consts()),
        }
    }

    fn src(&mut self, s: &Src) -> Result<String, Error> {
        let mut e = self.reg(s.reg, s.num, s.relative.as_ref())?;
        if s.swizzle != [0, 1, 2, 3] {
            let sw: String = s.swizzle.iter().map(|&c| XYZW[c as usize]).collect();
            e = format!("({e}).{sw}");
        }
        use SrcMod::*;
        Ok(match s.modifier {
            None => e,
            Neg => format!("(-{e})"),
            Abs => format!("abs({e})"),
            AbsNeg => format!("(-abs({e}))"),
            Bias => format!("({e} - vec4<f32>(0.5))"),
            BiasNeg => format!("(-({e} - vec4<f32>(0.5)))"),
            Sign => format!("({e} * 2.0 - vec4<f32>(1.0))"),
            SignNeg => format!("(-({e} * 2.0 - vec4<f32>(1.0)))"),
            Comp => format!("(vec4<f32>(1.0) - {e})"),
            X2 => format!("({e} * 2.0)"),
            X2Neg => format!("({e} * -2.0)"),
            Dz => format!("({e} / ({e}).z)"),
            Dw => format!("({e} / ({e}).w)"),
            Not => format!("(vec4<f32>(1.0) - {e})"),
        })
    }

    /// Scalar operand of a scalar instruction (replicate swizzle).
    fn scalar(&mut self, s: &Src) -> Result<String, Error> {
        let c = if s.swizzle.iter().all(|&c| c == s.swizzle[0]) { 'x' } else { 'w' };
        Ok(format!("({}).{c}", self.src(s)?))
    }

    /// Row `k` of a matrix operand (`m4x4` etc. read consecutive registers).
    fn src_row(&mut self, s: &Src, k: u32) -> Result<String, Error> {
        let mut row = s.clone();
        row.num += k;
        self.src(&row)
    }

    fn dst_name(&mut self, d: &Dst) -> Result<String, Error> {
        use RegType::*;
        Ok(match d.reg {
            Temp => {
                self.temps.insert(d.num);
                format!("r{}", d.num)
            }
            AddrOrTexture if self.stage == Stage::Vertex => {
                self.uses_addr = true;
                format!("a{}", d.num)
            }
            Predicate => {
                self.uses_pred = true;
                format!("p{}", d.num)
            }
            Output | RastOut | AttrOut | ColorOut | DepthOut => {
                if d.relative.is_some() {
                    return Err(Error::Unsupported("relative output write".into()));
                }
                self.outputs_written.insert((d.reg, d.num));
                out_name(d.reg, d.num)
            }
            other => return Err(Error::Unsupported(format!("destination register {other:?}"))),
        })
    }

    /// Writes `value` (a vec4 expression) to `d` under its mask, saturate and
    /// the instruction's predicate.
    fn write(&mut self, d: &Dst, value: String, pred: Option<&Src>) -> Result<(), Error> {
        let name = self.dst_name(d)?;
        let mut v = value;
        if d.saturate {
            v = format!("clamp({v}, vec4<f32>(0.0), vec4<f32>(1.0))");
        }
        if d.shift != 0 {
            v = format!("({v} * {})", float(2f32.powi(d.shift as i32)));
        }
        let (conv, pre) = match d.reg {
            RegType::AddrOrTexture => ("vec4<i32>(round(t))", ""),
            RegType::Predicate => ("(t != vec4<f32>(0.0))", ""),
            _ => ("t", ""),
        };
        let _ = pre;
        let pred_expr = match pred {
            Some(p) => {
                let mut e = format!("p{}", p.num);
                self.uses_pred = true;
                if p.swizzle != [0, 1, 2, 3] {
                    let sw: String = p.swizzle.iter().map(|&c| XYZW[c as usize]).collect();
                    e = format!("{e}.{sw}");
                }
                if p.modifier == SrcMod::Not {
                    e = format!("!({e})");
                }
                Some(e)
            }
            None => None,
        };
        let comps: Vec<char> = XYZW.iter().zip(d.mask).filter(|(_, m)| *m).map(|(c, _)| *c).collect();
        if comps.len() == 4 && pred_expr.is_none() {
            self.line(format!("{{ let t = {v}; {name} = {conv}; }}"));
            return Ok(());
        }
        let mut s = format!("{{ let t = {v}; let u = {conv};");
        for c in comps {
            match &pred_expr {
                Some(p) => write!(s, " if ({p}.{c}) {{ {name}.{c} = u.{c}; }}").unwrap(),
                None => write!(s, " {name}.{c} = u.{c};").unwrap(),
            }
        }
        s.push_str(" }");
        self.line(s);
        Ok(())
    }

    fn cmp_expr(cmp: Cmp, a: &str, b: &str) -> String {
        let op = match cmp {
            Cmp::Gt => ">",
            Cmp::Eq => "==",
            Cmp::Ge => ">=",
            Cmp::Lt => "<",
            Cmp::Ne => "!=",
            Cmp::Le => "<=",
        };
        format!("({a} {op} {b})")
    }

    fn sampler_type(&self, s: u32) -> SamplerType {
        self.p
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Sampler { ty, dst } if dst.num == s => Some(*ty),
                _ => None,
            })
            .unwrap_or(SamplerType::Tex2D)
    }

    fn tex_coord(&self, s: u32, c: &str) -> String {
        match self.sampler_type(s) {
            SamplerType::Cube | SamplerType::Volume => format!("({c}).xyz"),
            _ => format!("({c}).xy"),
        }
    }

    fn instr(&mut self, i: &Instr) -> Result<(), Error> {
        use Op::*;
        let s = |g: &mut Self, k: usize| -> Result<String, Error> {
            let src = i.src.get(k).ok_or_else(|| Error::Malformed(format!("{:?} operand {k}", i.op)))?;
            g.src(src)
        };
        let sc = |g: &mut Self, k: usize| -> Result<String, Error> {
            let src = i.src.get(k).ok_or_else(|| Error::Malformed(format!("{:?} operand {k}", i.op)))?;
            g.scalar(src)
        };
        let v = match i.op {
            Nop => return Ok(()),
            Mov => s(self, 0)?,
            Mova => s(self, 0)?,
            Add => format!("({} + {})", s(self, 0)?, s(self, 1)?),
            Sub => format!("({} - {})", s(self, 0)?, s(self, 1)?),
            Mul => format!("({} * {})", s(self, 0)?, s(self, 1)?),
            Mad => format!("fma({}, {}, {})", s(self, 0)?, s(self, 1)?, s(self, 2)?),
            Min => format!("min({}, {})", s(self, 0)?, s(self, 1)?),
            Max => format!("max({}, {})", s(self, 0)?, s(self, 1)?),
            Dp3 => format!("vec4<f32>(dot(({}).xyz, ({}).xyz))", s(self, 0)?, s(self, 1)?),
            Dp4 => format!("vec4<f32>(dot({}, {}))", s(self, 0)?, s(self, 1)?),
            Dp2Add => format!("vec4<f32>(dot(({}).xy, ({}).xy) + {})", s(self, 0)?, s(self, 1)?, sc(self, 2)?),
            Rcp => format!("vec4<f32>(1.0 / {})", sc(self, 0)?),
            Rsq => format!("vec4<f32>(inverseSqrt(abs({})))", sc(self, 0)?),
            Exp | ExpP => format!("vec4<f32>(exp2({}))", sc(self, 0)?),
            Log | LogP => format!("vec4<f32>(log2(abs({})))", sc(self, 0)?),
            Pow => format!("vec4<f32>(pow(abs({}), {}))", sc(self, 0)?, sc(self, 1)?),
            Slt => format!("select(vec4<f32>(0.0), vec4<f32>(1.0), {} < {})", s(self, 0)?, s(self, 1)?),
            Sge => format!("select(vec4<f32>(0.0), vec4<f32>(1.0), {} >= {})", s(self, 0)?, s(self, 1)?),
            Frc => format!("fract({})", s(self, 0)?),
            Abs => format!("abs({})", s(self, 0)?),
            Nrm => {
                let a = s(self, 0)?;
                format!("({a} * inverseSqrt(dot(({a}).xyz, ({a}).xyz)))")
            }
            Lrp => format!("mix({}, {}, {})", s(self, 2)?, s(self, 1)?, s(self, 0)?),
            Crs => format!("vec4<f32>(cross(({}).xyz, ({}).xyz), 0.0)", s(self, 0)?, s(self, 1)?),
            Sgn => format!("sign({})", s(self, 0)?),
            SinCos => {
                let a = sc(self, 0)?;
                format!("vec4<f32>(cos({a}), sin({a}), 0.0, 0.0)")
            }
            Dst => {
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                format!("vec4<f32>(1.0, ({a}).y * ({b}).y, ({a}).z, ({b}).w)")
            }
            Lit => {
                let a = s(self, 0)?;
                format!(
                    "vec4<f32>(1.0, max(({a}).x, 0.0), select(0.0, pow(max(({a}).y, 0.0), clamp(({a}).w, -127.9961, 127.9961)), ({a}).x > 0.0), 1.0)"
                )
            }
            M4x4 | M4x3 | M3x4 | M3x3 | M3x2 => {
                let (rows, dims) = match i.op {
                    M4x4 => (4, 4),
                    M4x3 => (3, 4),
                    M3x4 => (4, 3),
                    M3x3 => (3, 3),
                    _ => (2, 3),
                };
                let a = s(self, 0)?;
                let src1 = i.src.get(1).ok_or_else(|| Error::Malformed("matrix operand".into()))?.clone();
                let mut comps = Vec::new();
                for k in 0..4 {
                    if k < rows {
                        let row = self.src_row(&src1, k)?;
                        comps.push(if dims == 4 {
                            format!("dot({a}, {row})")
                        } else {
                            format!("dot(({a}).xyz, ({row}).xyz)")
                        });
                    } else {
                        comps.push("0.0".into());
                    }
                }
                format!("vec4<f32>({})", comps.join(", "))
            }
            Cmp => format!("select({}, {}, {} >= vec4<f32>(0.0))", s(self, 2)?, s(self, 1)?, s(self, 0)?),
            Cnd => format!("select({}, {}, {} > vec4<f32>(0.5))", s(self, 2)?, s(self, 1)?, s(self, 0)?),
            Dsx => format!("dpdx({})", s(self, 0)?),
            Dsy => format!("dpdy({})", s(self, 0)?),
            Tex | TexLdl | TexLdd => {
                let samp = i.src.get(1).ok_or_else(|| Error::Malformed("texld sampler".into()))?;
                let n = samp.num;
                let c = s(self, 0)?;
                let coord = self.tex_coord(n, &c);
                let (t, sm) = (format!("tex{n}"), format!("smp{n}"));
                let sample = match i.op {
                    TexLdl => format!("textureSampleLevel({t}, {sm}, {coord}, ({c}).w)"),
                    TexLdd => {
                        let (dx, dy) = (s(self, 2)?, s(self, 3)?);
                        let (dx, dy) = (self.tex_coord(n, &dx), self.tex_coord(n, &dy));
                        format!("textureSampleGrad({t}, {sm}, {coord}, {dx}, {dy})")
                    }
                    _ if self.stage == Stage::Vertex => format!("textureSampleLevel({t}, {sm}, {coord}, 0.0)"),
                    _ => match i.control & 3 {
                        1 => {
                            let pc = self.tex_coord(n, &format!("(({c}) / ({c}).w)"));
                            format!("textureSample({t}, {sm}, {pc})")
                        }
                        2 => format!("textureSampleBias({t}, {sm}, {coord}, ({c}).w)"),
                        _ => format!("textureSample({t}, {sm}, {coord})"),
                    },
                };
                // apply the sampler operand's swizzle to the result
                if samp.swizzle != [0, 1, 2, 3] {
                    let sw: String = samp.swizzle.iter().map(|&c| XYZW[c as usize]).collect();
                    format!("({sample}).{sw}")
                } else {
                    sample
                }
            }
            TexKill => {
                let d = i.dst.as_ref().ok_or_else(|| Error::Malformed("texkill".into()))?;
                let e = self.reg(d.reg, d.num, None)?;
                let comps: String = XYZW.iter().zip(d.mask).filter(|(_, m)| *m).map(|(c, _)| *c).collect();
                let test = if comps.len() == 1 {
                    format!("({e}).{comps} < 0.0")
                } else {
                    format!("any(({e}).{comps} < vec{}<f32>(0.0))", comps.len())
                };
                self.line(format!("if ({test}) {{ discard; }}"));
                return Ok(());
            }
            Setp => {
                let cmp = i.cmp().ok_or_else(|| Error::Malformed("setp compare".into()))?;
                let e = Self::cmp_expr(cmp, &s(self, 0)?, &s(self, 1)?);
                format!("select(vec4<f32>(0.0), vec4<f32>(1.0), {e})")
            }
            If => {
                let b = i.src.first().ok_or_else(|| Error::Malformed("if".into()))?;
                let cond = match b.reg {
                    RegType::ConstBool => self.bool_reg(b.num),
                    RegType::Predicate => {
                        self.uses_pred = true;
                        let c = XYZW[b.swizzle[0] as usize];
                        let e = format!("p{}.{c}", b.num);
                        if b.modifier == SrcMod::Not { format!("!{e}") } else { e }
                    }
                    other => return Err(Error::Unsupported(format!("if on {other:?}"))),
                };
                self.line(format!("if ({cond}) {{"));
                self.indent += 1;
                return Ok(());
            }
            Ifc => {
                let cmp = i.cmp().ok_or_else(|| Error::Malformed("ifc compare".into()))?;
                let (a, b) = (sc(self, 0)?, sc(self, 1)?);
                self.line(format!("if {} {{", Self::cmp_expr(cmp, &a, &b)));
                self.indent += 1;
                return Ok(());
            }
            Else => {
                self.indent = self.indent.saturating_sub(1);
                self.line("} else {");
                self.indent += 1;
                return Ok(());
            }
            EndIf | EndRep | EndLoop => {
                if matches!(i.op, EndLoop | EndRep) {
                    self.loops.pop();
                }
                self.indent = self.indent.saturating_sub(1);
                self.line("}");
                return Ok(());
            }
            Rep | Loop => {
                let ireg = i.src.last().ok_or_else(|| Error::Malformed("loop count".into()))?;
                let iv = match self.defis.get(&ireg.num) {
                    Some(v) => format!("vec4<i32>({}, {}, {}, {})", v[0], v[1], v[2], v[3]),
                    None => format!("{}.i[{}]", self.consts(), ireg.num),
                };
                let k = self.fresh("k");
                let al = self.fresh("aL");
                self.line(format!("for (var {k}: i32 = 0; {k} < min(({iv}).x, 255); {k}++) {{"));
                self.indent += 1;
                self.line(format!("let {al}: i32 = ({iv}).y + {k} * ({iv}).z;"));
                self.loops.push(al);
                return Ok(());
            }
            Break => {
                self.line("break;");
                return Ok(());
            }
            Breakc => {
                let cmp = i.cmp().ok_or_else(|| Error::Malformed("breakc compare".into()))?;
                let (a, b) = (sc(self, 0)?, sc(self, 1)?);
                self.line(format!("if {} {{ break; }}", Self::cmp_expr(cmp, &a, &b)));
                return Ok(());
            }
            BreakP => {
                let b = &i.src[0];
                self.uses_pred = true;
                let c = XYZW[b.swizzle[0] as usize];
                let neg = if b.modifier == SrcMod::Not { "!" } else { "" };
                self.line(format!("if ({neg}p{}.{c}) {{ break; }}", b.num));
                return Ok(());
            }
            Ret => {
                // only the end-of-main ret is supported (no subroutines)
                return Ok(());
            }
            other => return Err(Error::Unsupported(format!("instruction {other:?}"))),
        };
        let d = i.dst.as_ref().ok_or_else(|| Error::Malformed(format!("{:?} without destination", i.op)))?;
        self.write(d, v, i.predicate.as_ref())
    }
}

fn out_name(reg: RegType, num: u32) -> String {
    match reg {
        RegType::Output => format!("o{num}"),
        RegType::RastOut => ["opos", "ofog", "opts"].get(num as usize).map_or(format!("orast{num}"), |s| s.to_string()),
        RegType::AttrOut => format!("od{num}"),
        RegType::ColorOut => format!("oc{num}"),
        RegType::DepthOut => "odepth".into(),
        _ => format!("x{num}"),
    }
}

pub fn translate(p: &Program, opts: &Options) -> Result<Translated, Error> {
    if p.instrs.iter().any(|i| matches!(i.op, Op::Call | Op::CallNz | Op::Label)) {
        return Err(Error::Unsupported("subroutines".into()));
    }
    let stage = p.stage();
    let mut g = Gen {
        p,
        stage,
        out: String::new(),
        indent: 1,
        loops: Vec::new(),
        next_id: 0,
        temps: BTreeSet::new(),
        outputs_written: BTreeSet::new(),
        uses_addr: false,
        uses_pred: false,
        defs: p.defs.iter().copied().collect(),
        defis: p.defis.iter().copied().collect(),
        defbs: p.defbs.iter().copied().collect(),
    };
    for i in &p.instrs {
        g.instr(i)?;
    }
    let body = std::mem::take(&mut g.out);

    let mut w = String::new();
    w.push_str("diagnostic(off, derivative_uniformity);\n\n");
    w.push_str("struct Consts {\n    c: array<vec4<f32>, ");
    w.push_str(&format!("{}>,\n    i: array<vec4<i32>, 16>,\n    b: array<vec4<u32>, 16>,\n}};\n", g.max_consts()));
    let (binding, cname) = if stage == Stage::Vertex { (0, "vsc") } else { (1, "psc") };
    writeln!(w, "@group(0) @binding({binding}) var<uniform> {cname}: Consts;").unwrap();

    let mut samplers = Vec::new();
    for d in &p.decls {
        if let Decl::Sampler { ty, dst } = d {
            let base = if stage == Stage::Vertex { VS_SAMPLER_BASE } else { 0 };
            let tex_ty = match ty {
                SamplerType::Cube => "texture_cube<f32>",
                SamplerType::Volume => "texture_3d<f32>",
                _ => "texture_2d<f32>",
            };
            writeln!(w, "@group(1) @binding({}) var tex{}: {tex_ty};", base + 2 * dst.num, dst.num).unwrap();
            writeln!(w, "@group(1) @binding({}) var smp{}: sampler;", base + 2 * dst.num + 1, dst.num).unwrap();
            samplers.push((dst.num, *ty));
        }
    }

    // inputs
    let mut inputs = Vec::new();
    let mut in_fields = Vec::new();
    let mut in_copies = Vec::new();
    let mut has_vpos = false;
    let mut has_vface = false;
    for d in &p.decls {
        match d {
            Decl::Semantic { usage, index, dst } if dst.reg == RegType::Input => {
                let loc = if stage == Stage::Vertex {
                    vertex_input_location(*usage, *index)
                } else {
                    varying_location(*usage, *index)
                };
                let Some(loc) = loc else {
                    return Err(Error::Unsupported(format!("input semantic {usage:?}{index}")));
                };
                let available = opts
                    .available_varyings
                    .as_ref()
                    .is_none_or(|set| stage == Stage::Vertex || set.contains(&(usage_key(*usage), *index)));
                let mask: String = XYZW.iter().zip(dst.mask).filter(|(_, m)| *m).map(|(c, _)| *c).collect();
                if available {
                    let field = format!("in{loc}");
                    if !in_fields.iter().any(|f: &String| f.contains(&format!(" {field}:"))) {
                        in_fields.push(format!("    @location({loc}) {field}: vec4<f32>,"));
                    }
                    if mask.len() == 4 {
                        in_copies.push(format!("    v{} = vin.{field};", dst.num));
                    } else {
                        for c in mask.chars() {
                            in_copies.push(format!("    v{}.{c} = vin.{field}.{c};", dst.num));
                        }
                    }
                    inputs.push(IoSlot { usage: *usage, index: *index, location: loc });
                }
            }
            Decl::Misc { dst } => {
                if dst.num == 0 {
                    has_vpos = true;
                } else {
                    has_vface = true;
                }
            }
            _ => {}
        }
    }
    let input_regs: BTreeSet<u32> = p
        .decls
        .iter()
        .filter_map(|d| match d {
            Decl::Semantic { dst, .. } if dst.reg == RegType::Input => Some(dst.num),
            _ => None,
        })
        .collect();

    // outputs
    let mut outputs = Vec::new();
    let mut out_fields = Vec::new();
    let mut out_copies = Vec::new();
    let mut out_regs = BTreeSet::new();
    if stage == Stage::Vertex {
        let mut has_pos = false;
        for d in &p.decls {
            if let Decl::Semantic { usage, index, dst } = d {
                if dst.reg != RegType::Output {
                    continue;
                }
                out_regs.insert(dst.num);
                if *usage == Usage::Position && *index == 0 {
                    out_fields.push("    @builtin(position) pos: vec4<f32>,".to_string());
                    out_copies.push(format!("    vout.pos = o{};", dst.num));
                    has_pos = true;
                    continue;
                }
                let Some(loc) = varying_location(*usage, *index) else {
                    // PSIZE and friends have no WGSL equivalent; drop them
                    continue;
                };
                let field = format!("out{loc}");
                if !out_fields.iter().any(|f| f.contains(&format!(" {field}:"))) {
                    out_fields.push(format!("    @location({loc}) {field}: vec4<f32>,"));
                }
                for (c, m) in XYZW.iter().zip(dst.mask) {
                    if m {
                        out_copies.push(format!("    vout.{field}.{c} = o{}.{c};", dst.num));
                    }
                }
                outputs.push(IoSlot { usage: *usage, index: *index, location: loc });
            }
        }
        if !has_pos {
            if g.outputs_written.contains(&(RegType::RastOut, 0)) {
                out_fields.push("    @builtin(position) pos: vec4<f32>,".into());
                out_copies.push("    vout.pos = opos;".into());
            } else {
                return Err(Error::Malformed("vertex shader without position output".into()));
            }
        }
    } else {
        for (reg, num) in &g.outputs_written {
            match reg {
                RegType::ColorOut => {
                    out_fields.push(format!("    @location({num}) c{num}: vec4<f32>,"));
                    out_copies.push(format!("    pout.c{num} = oc{num};"));
                }
                RegType::DepthOut => {
                    out_fields.push("    @builtin(frag_depth) depth: f32,".into());
                    out_copies.push("    pout.depth = odepth.x;".into());
                }
                _ => {}
            }
        }
        if out_fields.is_empty() {
            out_fields.push("    @location(0) c0: vec4<f32>,".into());
            out_copies.push("    pout.c0 = vec4<f32>(0.0);".into());
        }
    }

    let (in_struct, out_struct, entry, attr) = if stage == Stage::Vertex {
        ("VsIn", "VsOut", "vs_main", "@vertex")
    } else {
        ("PsIn", "PsOut", "ps_main", "@fragment")
    };
    if !in_fields.is_empty() || (stage == Stage::Pixel && (has_vpos || has_vface)) {
        writeln!(w, "\nstruct {in_struct} {{").unwrap();
        for f in &in_fields {
            writeln!(w, "{f}").unwrap();
        }
        if stage == Stage::Pixel && has_vpos {
            writeln!(w, "    @builtin(position) fragpos: vec4<f32>,").unwrap();
        }
        if stage == Stage::Pixel && has_vface {
            writeln!(w, "    @builtin(front_facing) front: bool,").unwrap();
        }
        writeln!(w, "}};").unwrap();
    }
    writeln!(w, "\nstruct {out_struct} {{").unwrap();
    for f in &out_fields {
        writeln!(w, "{f}").unwrap();
    }
    writeln!(w, "}};").unwrap();

    let has_in_struct = !in_fields.is_empty() || (stage == Stage::Pixel && (has_vpos || has_vface));
    let param = if has_in_struct { format!("vin: {in_struct}") } else { String::new() };
    writeln!(w, "\n{attr}\nfn {entry}({param}) -> {out_struct} {{").unwrap();
    for t in &g.temps {
        writeln!(w, "    var r{t}: vec4<f32> = vec4<f32>(0.0);").unwrap();
    }
    for r in &input_regs {
        writeln!(w, "    var v{r}: vec4<f32> = vec4<f32>(0.0);").unwrap();
    }
    for c in &in_copies {
        writeln!(w, "{c}").unwrap();
    }
    if has_vpos {
        writeln!(w, "    let vpos = vec4<f32>(vin.fragpos.xy - vec2<f32>(0.5), 0.0, 0.0);").unwrap();
    }
    if has_vface {
        writeln!(w, "    let vface = vec4<f32>(select(-1.0, 1.0, vin.front));").unwrap();
    }
    if g.uses_addr {
        writeln!(w, "    var a0: vec4<i32> = vec4<i32>(0);").unwrap();
    }
    if g.uses_pred {
        writeln!(w, "    var p0: vec4<bool> = vec4<bool>(false);").unwrap();
    }
    let mut declared = BTreeSet::new();
    for r in &out_regs {
        declared.insert(format!("o{r}"));
    }
    for (reg, num) in &g.outputs_written {
        declared.insert(out_name(*reg, *num));
    }
    if stage == Stage::Vertex {
        declared.insert("opos".into());
    }
    for name in &declared {
        let init = if name == "opos" { "vec4<f32>(0.0, 0.0, 0.0, 1.0)" } else { "vec4<f32>(0.0)" };
        writeln!(w, "    var {name}: vec4<f32> = {init};").unwrap();
    }
    w.push_str(&body);
    let ret = if stage == Stage::Vertex { "vout" } else { "pout" };
    writeln!(w, "    var {ret}: {out_struct};").unwrap();
    for c in &out_copies {
        writeln!(w, "{c}").unwrap();
    }
    writeln!(w, "    return {ret};\n}}").unwrap();

    let _ = reg_name; // shared naming lives in disasm
    Ok(Translated { wgsl: w, entry, inputs, outputs, samplers })
}
