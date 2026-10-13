//! Assembly-like text form of a parsed program (`Display` for [`Program`]).

use std::fmt;

use crate::parse::{Decl, Dst, Program, RegType, Src, SrcMod, Stage};

pub(crate) fn reg_name(reg: RegType, num: u32, stage: Stage) -> String {
    use RegType::*;
    match reg {
        Temp => format!("r{num}"),
        Input => format!("v{num}"),
        Const => format!("c{num}"),
        AddrOrTexture => if stage == Stage::Vertex { format!("a{num}") } else { format!("t{num}") },
        RastOut => ["oPos", "oFog", "oPts"].get(num as usize).map_or(format!("oRast{num}"), |s| s.to_string()),
        AttrOut => format!("oD{num}"),
        Output => format!("o{num}"),
        ConstInt => format!("i{num}"),
        ColorOut => format!("oC{num}"),
        DepthOut => "oDepth".into(),
        Sampler => format!("s{num}"),
        ConstBool => format!("b{num}"),
        Loop => "aL".into(),
        Misc => if num == 0 { "vPos".into() } else { "vFace".into() },
        Predicate => format!("p{num}"),
        r => format!("{r:?}{num}"),
    }
}

const XYZW: [char; 4] = ['x', 'y', 'z', 'w'];

impl fmt::Display for Dst {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = reg_name(self.reg, self.num, Stage::Vertex);
        if self.mask != [true; 4] {
            s.push('.');
            s.extend(XYZW.iter().zip(self.mask).filter(|(_, m)| *m).map(|(c, _)| *c));
        }
        if self.saturate {
            s.insert_str(0, "sat ");
        }
        f.write_str(&s)
    }
}

impl fmt::Display for Src {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = reg_name(self.reg, self.num, Stage::Vertex);
        if let Some(r) = &self.relative {
            s = format!("{}[{}.{} + {}]", &s[..1], reg_name(r.reg, r.num, Stage::Vertex), XYZW[r.comp as usize], self.num);
        }
        if self.swizzle != [0, 1, 2, 3] {
            s.push('.');
            s.extend(self.swizzle.iter().map(|&c| XYZW[c as usize]));
        }
        let s = match self.modifier {
            SrcMod::None => s,
            SrcMod::Neg => format!("-{s}"),
            SrcMod::Abs => format!("|{s}|"),
            SrcMod::AbsNeg => format!("-|{s}|"),
            m => format!("{m:?}({s})"),
        };
        f.write_str(&s)
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let st = if self.stage() == Stage::Vertex { "vs" } else { "ps" };
        writeln!(f, "{st}_{}_{}", self.major, self.minor)?;
        for c in &self.constants {
            writeln!(f, "// {:?}{} x{} {}", c.set, c.index, c.count, c.name)?;
        }
        for d in &self.decls {
            match d {
                Decl::Semantic { usage, index, dst } => writeln!(f, "dcl_{usage:?}{index} {dst}")?,
                Decl::Sampler { ty, dst } => writeln!(f, "dcl_{ty:?} {dst}")?,
                Decl::Misc { dst } => writeln!(f, "dcl {}", reg_name(dst.reg, dst.num, self.stage()))?,
            }
        }
        for (n, v) in &self.defs {
            writeln!(f, "def c{n}, {}, {}, {}, {}", v[0], v[1], v[2], v[3])?;
        }
        for (n, v) in &self.defis {
            writeln!(f, "defi i{n}, {}, {}, {}, {}", v[0], v[1], v[2], v[3])?;
        }
        for i in &self.instrs {
            let mut ops: Vec<String> = i.dst.iter().map(|d| d.to_string()).collect();
            ops.extend(i.src.iter().map(|s| s.to_string()));
            let pred = i.predicate.as_ref().map(|p| format!("({p}) ")).unwrap_or_default();
            let cmp = i.cmp().map(|c| format!("_{c:?}")).unwrap_or_default();
            writeln!(f, "{pred}{:?}{cmp} {}", i.op, ops.join(", "))?;
        }
        Ok(())
    }
}
