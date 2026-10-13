//! SM3 token stream parser.
//!
//! Token layout follows the public D3D9 shader bytecode specification
//! (d3d9types.h `D3DSHADER_*` masks).

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Pixel,
}

/// Register file (`D3DSHADER_PARAM_REGISTER_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RegType {
    Temp,
    Input,
    Const,
    /// `a0` in vertex shaders, `t#` in ps_1/2 pixel shaders.
    AddrOrTexture,
    RastOut,
    AttrOut,
    /// `o#` in vs_3_0 (texcoord out in older models).
    Output,
    ConstInt,
    ColorOut,
    DepthOut,
    Sampler,
    Const2,
    Const3,
    Const4,
    ConstBool,
    Loop,
    TempFloat16,
    Misc,
    Label,
    Predicate,
    Unknown(u32),
}

impl RegType {
    fn from_token(t: u32) -> Self {
        let v = ((t >> 28) & 0x7) | ((t >> 8) & 0x18);
        use RegType::*;
        match v {
            0 => Temp,
            1 => Input,
            2 => Const,
            3 => AddrOrTexture,
            4 => RastOut,
            5 => AttrOut,
            6 => Output,
            7 => ConstInt,
            8 => ColorOut,
            9 => DepthOut,
            10 => Sampler,
            11 => Const2,
            12 => Const3,
            13 => Const4,
            14 => ConstBool,
            15 => Loop,
            16 => TempFloat16,
            17 => Misc,
            18 => Label,
            19 => Predicate,
            v => Unknown(v),
        }
    }
}

/// Source modifier (`D3DSPSM_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcMod {
    None,
    Neg,
    Bias,
    BiasNeg,
    Sign,
    SignNeg,
    Comp,
    X2,
    X2Neg,
    Dz,
    Dw,
    Abs,
    AbsNeg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Relative {
    pub reg: RegType,
    pub num: u32,
    /// Component of the address register.
    pub comp: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Src {
    pub reg: RegType,
    pub num: u32,
    /// Four 2-bit component selectors.
    pub swizzle: [u8; 4],
    pub modifier: SrcMod,
    pub relative: Option<Relative>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dst {
    pub reg: RegType,
    pub num: u32,
    /// x, y, z, w write enables.
    pub mask: [bool; 4],
    pub saturate: bool,
    pub partial_precision: bool,
    pub centroid: bool,
    /// Signed shift scale (ps_1_x only).
    pub shift: i8,
    pub relative: Option<Relative>,
}

/// `dcl` usage (`D3DDECLUSAGE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Usage {
    Position,
    BlendWeight,
    BlendIndices,
    Normal,
    PSize,
    TexCoord,
    Tangent,
    Binormal,
    TessFactor,
    PositionT,
    Color,
    Fog,
    Depth,
    Sample,
    Unknown(u32),
}

impl Usage {
    fn from_code(c: u32) -> Self {
        use Usage::*;
        match c {
            0 => Position,
            1 => BlendWeight,
            2 => BlendIndices,
            3 => Normal,
            4 => PSize,
            5 => TexCoord,
            6 => Tangent,
            7 => Binormal,
            8 => TessFactor,
            9 => PositionT,
            10 => Color,
            11 => Fog,
            12 => Depth,
            13 => Sample,
            c => Unknown(c),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplerType {
    Tex2D,
    Cube,
    Volume,
    Unknown(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    /// Input or output register semantic.
    Semantic { usage: Usage, index: u32, dst: Dst },
    Sampler { ty: SamplerType, dst: Dst },
    /// Pixel shader `vPos` / `vFace` (no usage token meaning).
    Misc { dst: Dst },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Gt,
    Eq,
    Ge,
    Lt,
    Ne,
    Le,
}

impl Cmp {
    fn from_control(c: u32) -> Option<Self> {
        Some(match c & 7 {
            1 => Cmp::Gt,
            2 => Cmp::Eq,
            3 => Cmp::Ge,
            4 => Cmp::Lt,
            5 => Cmp::Ne,
            6 => Cmp::Le,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instr {
    pub op: Op,
    /// Opcode-specific control bits (comparison, texld variant).
    pub control: u32,
    pub dst: Option<Dst>,
    pub src: Vec<Src>,
    /// Predicate source when the instruction is predicated.
    pub predicate: Option<Src>,
}

impl Instr {
    pub fn cmp(&self) -> Option<Cmp> {
        Cmp::from_control(self.control)
    }
}

macro_rules! opcodes {
    ($($name:ident = $v:expr),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Op { $($name,)* Unknown(u32) }
        impl Op {
            fn from_code(c: u32) -> Self {
                match c { $($v => Op::$name,)* c => Op::Unknown(c) }
            }
        }
    };
}

opcodes! {
    Nop = 0, Mov = 1, Add = 2, Sub = 3, Mad = 4, Mul = 5, Rcp = 6, Rsq = 7, Dp3 = 8, Dp4 = 9,
    Min = 10, Max = 11, Slt = 12, Sge = 13, Exp = 14, Log = 15, Lit = 16, Dst = 17, Lrp = 18,
    Frc = 19, M4x4 = 20, M4x3 = 21, M3x4 = 22, M3x3 = 23, M3x2 = 24, Call = 25, CallNz = 26,
    Loop = 27, Ret = 28, EndLoop = 29, Label = 30, Dcl = 31, Pow = 32, Crs = 33, Sgn = 34,
    Abs = 35, Nrm = 36, SinCos = 37, Rep = 38, EndRep = 39, If = 40, Ifc = 41, Else = 42,
    EndIf = 43, Break = 44, Breakc = 45, Mova = 46, DefB = 47, DefI = 48, TexCoord = 64,
    TexKill = 65, Tex = 66, ExpP = 78, LogP = 79, Cnd = 80, Def = 81, Dp2Add = 90, Dsx = 91,
    Dsy = 92, TexLdd = 93, Setp = 94, TexLdl = 95, BreakP = 96, Cmp = 88,
}

/// One entry of the CTAB constant table.
#[derive(Debug, Clone, PartialEq)]
pub struct Constant {
    pub name: String,
    pub set: RegisterSet,
    pub index: u32,
    pub count: u32,
    /// Matrix/vector shape from the type info: class, rows, columns, elements.
    pub class: u16,
    pub rows: u16,
    pub columns: u16,
    pub elements: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterSet {
    Bool,
    Int4,
    Float4,
    Sampler,
    Unknown(u16),
}

#[derive(Debug, Clone, Default)]
pub struct Program {
    pub stage: Option<Stage>,
    pub major: u8,
    pub minor: u8,
    pub decls: Vec<Decl>,
    /// `def c#` immediate float constants.
    pub defs: Vec<(u32, [f32; 4])>,
    pub defis: Vec<(u32, [i32; 4])>,
    pub defbs: Vec<(u32, bool)>,
    pub instrs: Vec<Instr>,
    pub constants: Vec<Constant>,
}

impl Program {
    pub fn stage(&self) -> Stage {
        self.stage.unwrap_or(Stage::Vertex)
    }
}

struct Tokens<'a> {
    t: &'a [u32],
    p: usize,
}

impl Tokens<'_> {
    fn next(&mut self) -> Result<u32, Error> {
        let v = *self.t.get(self.p).ok_or(Error::Truncated)?;
        self.p += 1;
        Ok(v)
    }
}

fn relative(t: &mut Tokens, token: u32, major: u8) -> Result<Option<Relative>, Error> {
    if token & (1 << 13) == 0 {
        return Ok(None);
    }
    if major < 2 {
        // vs_1_x: implicit a0.x
        return Ok(Some(Relative { reg: RegType::AddrOrTexture, num: 0, comp: 0 }));
    }
    let a = t.next()?;
    Ok(Some(Relative { reg: RegType::from_token(a), num: a & 0x7ff, comp: ((a >> 16) & 3) as u8 }))
}

fn dst(t: &mut Tokens, major: u8) -> Result<Dst, Error> {
    let v = t.next()?;
    let m = (v >> 16) & 0xf;
    let shift = ((v >> 24) & 0xf) as i8;
    Ok(Dst {
        reg: RegType::from_token(v),
        num: v & 0x7ff,
        mask: [m & 1 != 0, m & 2 != 0, m & 4 != 0, m & 8 != 0],
        saturate: v & (1 << 20) != 0,
        partial_precision: v & (2 << 20) != 0,
        centroid: v & (4 << 20) != 0,
        shift: if shift > 7 { shift - 16 } else { shift },
        relative: relative(t, v, major)?,
    })
}

fn src(t: &mut Tokens, major: u8) -> Result<Src, Error> {
    let v = t.next()?;
    let sw = (v >> 16) & 0xff;
    use SrcMod::*;
    let modifier = match (v >> 24) & 0xf {
        0 => None,
        1 => Neg,
        2 => Bias,
        3 => BiasNeg,
        4 => Sign,
        5 => SignNeg,
        6 => Comp,
        7 => X2,
        8 => X2Neg,
        9 => Dz,
        10 => Dw,
        11 => Abs,
        12 => AbsNeg,
        13 => Not,
        m => return Err(Error::Unsupported(format!("source modifier {m}"))),
    };
    Ok(Src {
        reg: RegType::from_token(v),
        num: v & 0x7ff,
        swizzle: [(sw & 3) as u8, ((sw >> 2) & 3) as u8, ((sw >> 4) & 3) as u8, ((sw >> 6) & 3) as u8],
        modifier,
        relative: relative(t, v, major)?,
    })
}

/// Number of source operands and whether there's a destination.
fn arity(op: Op) -> Option<(bool, usize)> {
    use Op::*;
    Some(match op {
        Nop | Ret | Else | EndIf | EndLoop | EndRep | Break => (false, 0),
        Mov | Rcp | Rsq | Exp | Log | Lit | Frc | Abs | Nrm | Mova | ExpP | LogP | Dsx | Dsy => (true, 1),
        Add | Sub | Mul | Dp3 | Dp4 | Min | Max | Slt | Sge | Dst | M4x4 | M4x3 | M3x4 | M3x3
        | M3x2 | Pow | Crs | Setp => (true, 2),
        Mad | Lrp | Sgn | Cnd | Cmp | Dp2Add => (true, 3),
        SinCos => (true, 1), // sm3 form; sm2 has 2 extra sources (handled by length)
        Tex | TexLdl => (true, 2),
        TexLdd => (true, 4),
        TexKill => (true, 0),
        Call | Label => (false, 1),
        CallNz | Loop | Ifc | Breakc => (false, 2),
        If | Rep | BreakP => (false, 1),
        _ => return None,
    })
}

pub fn parse(bytes: &[u8]) -> Result<Program, Error> {
    if bytes.len() % 4 != 0 || bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    let words: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let mut t = Tokens { t: &words, p: 0 };
    let version = t.next()?;
    let mut prog = Program {
        stage: Some(match version >> 16 {
            0xfffe => Stage::Vertex,
            0xffff => Stage::Pixel,
            v => return Err(Error::Unsupported(format!("version token {v:#x}"))),
        }),
        major: ((version >> 8) & 0xff) as u8,
        minor: (version & 0xff) as u8,
        ..Default::default()
    };
    let major = prog.major;
    if major != 3 {
        return Err(Error::Unsupported(format!("shader model {major}.{}", prog.minor)));
    }

    loop {
        let tok = t.next()?;
        let code = tok & 0xffff;
        if code == 0xffff {
            break;
        }
        if code == 0xfffe {
            let len = ((tok >> 16) & 0x7fff) as usize;
            let start = t.p;
            t.p += len;
            let words = words.get(start..start + len).ok_or(Error::Truncated)?;
            if words.first() == Some(&u32::from_le_bytes(*b"CTAB")) {
                prog.constants = parse_ctab(words)?;
            }
            continue;
        }
        let len = ((tok >> 24) & 0xf) as usize;
        let end = t.p + len;
        let op = Op::from_code(code);
        let control = (tok >> 16) & 0xff;
        let predicated = tok & (1 << 28) != 0;
        match op {
            Op::Dcl => {
                let usage = t.next()?;
                let d = dst(&mut t, major)?;
                prog.decls.push(match d.reg {
                    RegType::Sampler => {
                        let ty = match (usage >> 27) & 0xf {
                            2 => SamplerType::Tex2D,
                            3 => SamplerType::Cube,
                            4 => SamplerType::Volume,
                            v => SamplerType::Unknown(v),
                        };
                        Decl::Sampler { ty, dst: d }
                    }
                    RegType::Misc => Decl::Misc { dst: d },
                    _ => Decl::Semantic { usage: Usage::from_code(usage & 0x1f), index: (usage >> 16) & 0xf, dst: d },
                });
            }
            Op::Def => {
                let d = dst(&mut t, major)?;
                let mut v = [0f32; 4];
                for x in &mut v {
                    *x = f32::from_bits(t.next()?);
                }
                prog.defs.push((d.num, v));
            }
            Op::DefI => {
                let d = dst(&mut t, major)?;
                let mut v = [0i32; 4];
                for x in &mut v {
                    *x = t.next()? as i32;
                }
                prog.defis.push((d.num, v));
            }
            Op::DefB => {
                let d = dst(&mut t, major)?;
                prog.defbs.push((d.num, t.next()? != 0));
            }
            _ => {
                let (has_dst, _) = arity(op).ok_or_else(|| Error::Unsupported(format!("opcode {op:?}")))?;
                let d = if has_dst { Some(dst(&mut t, major)?) } else { None };
                let predicate = if predicated { Some(src(&mut t, major)?) } else { None };
                let mut s = Vec::new();
                while t.p < end {
                    s.push(src(&mut t, major)?);
                }
                prog.instrs.push(Instr { op, control, dst: d, src: s, predicate });
            }
        }
        if t.p != end {
            return Err(Error::Malformed(format!("{op:?}: length mismatch")));
        }
    }
    Ok(prog)
}

fn parse_ctab(words: &[u32]) -> Result<Vec<Constant>, Error> {
    // offsets are relative to the table, which starts after the fourcc
    let data: Vec<u8> = words[1..].iter().flat_map(|w| w.to_le_bytes()).collect();
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let string_at = |o: usize| {
        let s = data.get(o..).unwrap_or_default();
        let n = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        String::from_utf8_lossy(&s[..n]).into_owned()
    };
    let bad = || Error::Malformed("CTAB".into());
    let count = u32_at(12).ok_or_else(bad)? as usize;
    let info = u32_at(16).ok_or_else(bad)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let o = info + i * 20;
        let ty = u32_at(o + 12).ok_or_else(bad)? as usize;
        out.push(Constant {
            name: string_at(u32_at(o).ok_or_else(bad)? as usize),
            set: match u16_at(o + 4).ok_or_else(bad)? {
                0 => RegisterSet::Bool,
                1 => RegisterSet::Int4,
                2 => RegisterSet::Float4,
                3 => RegisterSet::Sampler,
                v => RegisterSet::Unknown(v),
            },
            index: u16_at(o + 6).ok_or_else(bad)? as u32,
            count: u16_at(o + 8).ok_or_else(bad)? as u32,
            class: u16_at(ty).unwrap_or(0),
            rows: u16_at(ty + 4).unwrap_or(0),
            columns: u16_at(ty + 6).unwrap_or(0),
            elements: u16_at(ty + 8).unwrap_or(0),
        });
    }
    Ok(out)
}
