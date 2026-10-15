//! A map's lighting setup: `World.WorldGraphicData` -> `Ambiance`
//! (`WorldAmbiance`: `Fog`, `LayeredSky`, post effects) and the world
//! lightmaps.

use anvil_fastload::{Obj, Value};

use crate::Archive;

#[derive(Debug, Clone)]
pub struct Ambiance {
    pub name: String,
    /// Degrees.
    pub sun_azimuth: f32,
    pub sun_elevation: f32,
    /// Linear-ish 0..1 colour times `SunLightIntensity`.
    pub sun_color: [f32; 3],
    /// Effective ambient cube (Right, Left, Front, Back, Top, Down) after
    /// `AmbientCube::GetEffectiveColors`: face * Tint, HSV saturation *
    /// Saturation, then * Scale + Bias.
    pub ambient: [[f32; 3]; 6],
    pub fog_color: [f32; 3],
    pub fog_near: f32,
    pub fog_far: f32,
    pub fog_density: f32,
    pub fog_intensity: f32,
    pub fog_enabled: bool,
    /// Sky dome layers: mesh and material override.
    pub sky_layers: Vec<(u32, u32)>,
    pub lightmaps: Vec<WorldLightMap>,
    /// `AmbianceCurrentTime`, hours.
    pub time: f32,
}

#[derive(Debug, Clone)]
pub struct WorldLightMap {
    pub texture: u32,
    /// World -> UV scale (`WorldLightMapTexelWorldSize`).
    pub uv_scale: f32,
    pub z_min: f32,
    pub z_max: f32,
}

fn hsv_saturate(c: [f32; 3], s: f32) -> [f32; 3] {
    // scaling HSV saturation keeps the max channel and moves the others
    // toward it
    let max = c[0].max(c[1]).max(c[2]);
    c.map(|x| max - (max - x) * s)
}

impl Archive<'_> {
    fn f32_of(&self, o: &Obj, n: &str) -> f32 {
        self.field(o, n).and_then(Value::as_f32s).and_then(|v| v.first().copied()).unwrap_or(0.0)
    }

    fn color_of(&self, o: &Obj, n: &str) -> Option<[f32; 4]> {
        let c = self.field(o, n)?.as_obj()?;
        // IColor: bytes R G B A; Color: floats
        let byte = |k: &str| self.field(c, k).and_then(Value::as_u32);
        if self.is_type(c, "IColor") {
            return Some([byte("R")?, byte("G")?, byte("B")?, byte("A")?].map(|b| b as f32 / 255.0));
        }
        Some(["R", "G", "B", "A"].map(|k| self.f32_of(c, k)))
    }

    fn ambient_cube(&self, o: &Obj) -> [[f32; 3]; 6] {
        let face = |n: &str| self.field(o, n).and_then(Value::as_f32s).filter(|v| v.len() >= 3).map(|v| [v[0], v[1], v[2]]);
        let tint = face("Tint").unwrap_or([1.0; 3]);
        let (sat, scale, bias) = (self.f32_of(o, "Saturation"), self.f32_of(o, "Scale"), self.f32_of(o, "Bias"));
        ["Right", "Left", "Front", "Back", "Top", "Down"].map(|n| {
            let c = face(n).unwrap_or([0.5; 3]);
            let c = hsv_saturate([c[0] * tint[0], c[1] * tint[1], c[2] * tint[2]], sat);
            c.map(|x| x * scale + bias)
        })
    }

    /// The map's World's current ambiance.
    pub fn ambiance(&self) -> Option<Ambiance> {
        let world = self.uids_of_in("World", 0).into_iter().next()?;
        let w = self.root(world).ok()?;
        let gd = self.field(&w.obj, "WorldGraphicData")?.as_obj()?;
        let amb_uid = self.field(gd, "Ambiance")?.target_id()?;
        let amb = self.root(amb_uid).ok()?;
        let sky_uid = self.field(&amb.obj, "LayeredSky")?.target_id()?;
        let sky = self.root(sky_uid).ok()?;
        let s = &sky.obj;
        let fog = self.field(&amb.obj, "Fog").and_then(Value::as_obj);
        let sun = self.color_of(s, "SunLightColor").unwrap_or([1.0; 4]);
        let intensity = self.f32_of(s, "SunLightIntensity");
        let fog_color = fog.and_then(|f| self.color_of(f, "FogColor")).unwrap_or([0.6, 0.65, 0.7, 0.0]);
        let sky_layers = self
            .field(s, "Layers")
            .map(Value::items)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_obj)
            .filter_map(|l| {
                let mesh = self.field(l, "Mesh")?.target_id()?;
                Some((mesh, self.field(l, "MaterialOverride").and_then(Value::target_id).unwrap_or(0)))
            })
            .collect();
        let lightmaps = self
            .field(gd, "WorldLightMapSettings")
            .and_then(Value::as_obj)
            .and_then(|l| self.field(l, "WorldLightMaps"))
            .map(Value::items)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_obj)
            .filter_map(|m| {
                Some(WorldLightMap {
                    texture: self.field(m, "NewWorldLightMap")?.target_id()?,
                    uv_scale: self.f32_of(m, "WorldLightMapTexelWorldSize"),
                    z_min: self.f32_of(m, "NewWorldLightMapWorldZMin"),
                    z_max: self.f32_of(m, "NewWorldLightMapWorldZMax"),
                })
            })
            .collect();
        Some(Ambiance {
            name: self.loc(amb_uid).map(|l| l.name.clone()).unwrap_or_default(),
            sun_azimuth: self.f32_of(s, "SunLightAzimuth"),
            sun_elevation: self.f32_of(s, "SunLightElevation"),
            sun_color: [sun[0] * intensity, sun[1] * intensity, sun[2] * intensity],
            ambient: self.field(s, "EnvironmentAmbient").and_then(Value::as_obj).map(|a| self.ambient_cube(a)).unwrap_or([[0.4; 3]; 6]),
            fog_color: [fog_color[0], fog_color[1], fog_color[2]],
            fog_near: fog.map_or(60.0, |f| self.f32_of(f, "FogNear")),
            fog_far: fog.map_or(900.0, |f| self.f32_of(f, "FogFar")),
            fog_density: fog.map_or(0.7, |f| self.f32_of(f, "FogDensity")),
            fog_intensity: fog.map_or(1.0, |f| self.f32_of(f, "FogIntensity")),
            fog_enabled: fog.and_then(|f| self.field(f, "Enabled")).and_then(Value::as_bool).unwrap_or(false),
            sky_layers,
            lightmaps,
            time: self.f32_of(gd, "AmbianceCurrentTime"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::hsv_saturate;

    #[test]
    fn saturation_scales_toward_grey_keeping_the_max() {
        assert_eq!(hsv_saturate([1.0, 0.5, 0.0], 1.0), [1.0, 0.5, 0.0]);
        assert_eq!(hsv_saturate([1.0, 0.5, 0.0], 0.0), [1.0, 1.0, 1.0]);
        assert_eq!(hsv_saturate([1.0, 0.5, 0.0], 0.5), [1.0, 0.75, 0.5]);
    }
}
