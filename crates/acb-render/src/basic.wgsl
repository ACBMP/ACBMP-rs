// Placeholder material: diffuse texture, sun + hemisphere ambient, distance
// fog. Stands in until the retail SM3 shaders are translated.

struct Frame {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    sun_dir: vec4<f32>,   // towards the sun, world space (Z up)
    fog: vec4<f32>,       // rgb colour, w = density
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var diffuse: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var<uniform> alpha_ref: vec4<f32>;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    @location(7) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.pos, 1.0);
    var o: VsOut;
    o.clip = frame.view_proj * world;
    o.world = world.xyz;
    o.normal = normalize((model * vec4<f32>(v.normal, 0.0)).xyz);
    o.uv = v.uv;
    o.color = v.color;
    return o;
}

fn shade(i: VsOut, front: bool, albedo: vec3<f32>) -> vec4<f32> {
    var n = normalize(i.normal);
    if (!front) {
        n = -n;
    }
    let sun = max(dot(n, frame.sun_dir.xyz), 0.0) * vec3<f32>(1.0, 0.95, 0.85);
    let sky = mix(vec3<f32>(0.30, 0.28, 0.25), vec3<f32>(0.45, 0.52, 0.62), n.z * 0.5 + 0.5);
    let lit = albedo * (sun * 0.9 + sky * 0.6);
    let d = distance(i.world, frame.eye.xyz);
    let f = 1.0 - exp(-d * frame.fog.w);
    return vec4<f32>(mix(lit, frame.fog.rgb, f), 1.0);
}

@fragment
fn fs_opaque(i: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade(i, front, textureSample(diffuse, samp, i.uv).rgb);
}

@fragment
fn fs_alpha_test(i: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let t = textureSample(diffuse, samp, i.uv);
    if (t.a < alpha_ref.x) {
        discard;
    }
    return shade(i, front, t.rgb);
}

@fragment
fn fs_blended(i: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let t = textureSample(diffuse, samp, i.uv);
    // decals fade out through vertex alpha
    return vec4<f32>(shade(i, front, t.rgb).rgb, t.a * i.color.a);
}
