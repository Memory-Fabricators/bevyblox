// Port of the legacy forward "HQ" part shader (shaders/source/default.hlsl, plastic.hlsl,
// material.hlsl, common.h). Colours are non-linear (gamma-space) throughout, exactly like the
// original, which has no tonemap or sRGB framebuffer; only the final colour is converted so
// that Bevy's sRGB framebuffer writes the same numbers the original would have.
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_functions::{get_world_from_local, get_local_from_world},
    mesh_view_bindings as view_bindings,
    shadows,
};

struct Legacy {
    // Part colour (vertex colour) in gamma space; alpha is 1 - transparency.
    color: vec4<f32>,
    // Lighting.Ambient / Lamp0Color (key) / Lamp1Color (fill), rgb.
    ambient: vec4<f32>,
    key: vec4<f32>,
    fill: vec4<f32>,
    fog_color: vec4<f32>,
    // x: FogStart, y: FogEnd, z: shading distance (outline and detail fade), w: unused.
    fog: vec4<f32>,
    // x: specular intensity, y: specular power (also the far-LOD values for specular maps).
    spec: vec4<f32>,
    // x: texture repeats per stud, y: CFG_SPECULAR_SCALE, z: CFG_GLOSS_SCALE, w: CFG_NORMAL_SHADOW_SCALE.
    tex: vec4<f32>,
    // x: six 4-bit SurfaceType values in NormalId order, y: flags.
    packed: vec4<u32>,
}

const FLAG_STUDS: u32 = 1u;        // plastic: texture 1 is the stud atlas, texture 3 its normal map
const FLAG_OUTLINE: u32 = 2u;
const FLAG_TEXTURE: u32 = 4u;      // decal / mesh texture on texture 1, sampled with mesh UVs
const FLAG_UNLIT: u32 = 8u;
const FLAG_MATERIAL: u32 = 16u;    // texture 1 is a tiled material diffuse map
const FLAG_BLEND: u32 = 32u;       // CFG_OPT_BLEND_COLOR
const FLAG_DIFFUSE_CONST: u32 = 64u;
const FLAG_NORMAL: u32 = 128u;     // texture 3 is a normal map
const FLAG_SPECULAR: u32 = 256u;   // texture 5 is a specular map
const FLAG_WHOLE_STUD_OFFSET: u32 = 512u;

const SURFACE_NO_OUTLINES: u32 = 10u;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: Legacy;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var texture_a: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var sampler_a: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var texture_b: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var sampler_b: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var texture_c: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var sampler_c: sampler;

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

// NormalId order: +X 0, +Y 1, +Z 2, -X 3, -Y 4, -Z 5.
fn face_of(n: vec3<f32>) -> u32 {
    let a = abs(n);
    if (a.x >= a.y && a.x >= a.z) {
        return select(3u, 0u, n.x > 0.0);
    }
    if (a.y >= a.z) {
        return select(4u, 1u, n.y > 0.0);
    }
    return select(5u, 2u, n.z > 0.0);
}

// Face-local coordinates in studs measured from the face's start corner
// (GeometryGenerator.cpp::addBlock).
fn face_uv(face: u32, p: vec3<f32>, size: vec3<f32>) -> vec2<f32> {
    let q = p + 0.5 * size;
    switch face {
        case 0u: { return vec2<f32>(size.z - q.z, size.y - q.y); }
        case 1u: { return vec2<f32>(size.x - q.x, size.z - q.z); }
        case 2u: { return vec2<f32>(q.x, size.y - q.y); }
        case 3u: { return vec2<f32>(q.z, size.y - q.y); }
        case 4u: { return vec2<f32>(size.x - q.x, q.z); }
        default: { return vec2<f32>(size.x - q.x, size.y - q.y); }
    }
}

// Local direction of increasing face `u`: GeometryGenerator.cpp `tangents`.
fn face_tangent(face: u32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3<f32>(0.0, 0.0, -1.0); }
        case 1u: { return vec3<f32>(-1.0, 0.0, 0.0); }
        case 2u: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 3u: { return vec3<f32>(0.0, 0.0, 1.0); }
        case 4u: { return vec3<f32>(-1.0, 0.0, 0.0); }
        default: { return vec3<f32>(-1.0, 0.0, 0.0); }
    }
}

fn edge_distance(face: u32, p: vec3<f32>, size: vec3<f32>) -> f32 {
    let h = 0.5 * size - abs(p);
    if (face == 0u || face == 3u) { return min(h.y, h.z); }
    if (face == 1u || face == 4u) { return min(h.x, h.z); }
    return min(h.x, h.y);
}

// Row of the stud atlas for a SurfaceType (getStudsAtlasInfo): studs, weld/glue, inlet,
// universal. Returns (has relief, V offset); other surfaces sample the atlas at (0, 0).
fn atlas_row(surface: u32) -> vec2<f32> {
    switch surface {
        case 3u: { return vec2<f32>(1.0, 0.0); }
        case 1u, 2u: { return vec2<f32>(1.0, 0.25); }
        case 4u: { return vec2<f32>(1.0, 0.5); }
        case 5u: { return vec2<f32>(1.0, 0.75); }
        default: { return vec2<f32>(0.0, 0.0); }
    }
}

// nmapUnpack (common.h): two-channel normal maps keep x in alpha and y in green.
fn nmap_unpack(t: vec4<f32>) -> vec3<f32> {
    let xy = vec2<f32>(t.a, t.g) * 2.0 - 1.0;
    return vec3<f32>(xy, sqrt(saturate(1.0 - dot(xy, xy))));
}

// Stand-in for the original's per-part random surface offset (kUniformRandomTable): a stable
// hash of the part's world position.
fn part_random(world_from_local: mat4x4<f32>) -> vec2<f32> {
    let p = world_from_local[3].xyz;
    return vec2<f32>(
        fract(sin(dot(p, vec3<f32>(12.9898, 78.233, 37.719))) * 43758.5453),
        fract(sin(dot(p, vec3<f32>(39.346, 11.135, 83.155))) * 24634.6345),
    );
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let flags = material.packed.y;
    let view = view_bindings::view;
    let view_z = dot(vec4<f32>(
        view.view_from_world[0].z,
        view.view_from_world[1].z,
        view.view_from_world[2].z,
        view.view_from_world[3].z,
    ), in.world_position);
    let depth = max(-view_z, 0.001);
    let fade = saturate(1.0 - depth / material.fog.z);

    var albedo = material.color.rgb;
    var alpha = material.color.a;
    var n = normalize(in.world_normal);
    if (!is_front) { n = -n; }
    var spec_intensity = material.spec.x;
    var spec_power = material.spec.y;
    var outline = 1.0;

    if ((flags & FLAG_TEXTURE) != 0u) {
        let t = textureSample(texture_a, sampler_a, in.uv);
        albedo = albedo * t.rgb;
        alpha = alpha * t.a;
    }

    if ((flags & (FLAG_STUDS | FLAG_OUTLINE | FLAG_MATERIAL)) != 0u) {
        let world_from_local = get_world_from_local(in.instance_index);
        let local_from_world = get_local_from_world(in.instance_index);
        let basis = mat3x3<f32>(
            world_from_local[0].xyz, world_from_local[1].xyz, world_from_local[2].xyz,
        );
        let size = vec3<f32>(length(basis[0]), length(basis[1]), length(basis[2]));
        let p = (local_from_world * in.world_position).xyz * size;
        let face = face_of(transpose(basis) * in.world_normal);
        let surface = (material.packed.x >> (4u * face)) & 15u;
        let uv = face_uv(face, p, size);
        let uv_dx = dpdx(uv);
        let uv_dy = dpdy(uv);
        // Tangent frame: u axis, its perpendicular in the face, and the normal.
        let tangent = normalize(basis * face_tangent(face));
        let bitangent = cross(n, tangent);

        if ((flags & FLAG_STUDS) != 0u) {
            // plastic.hlsl: albedo = Color * (atlas.r * 2); stud normal at half strength. Studs
            // repeat every 2 studs in U and every 8 studs in V inside the surface's atlas row.
            let row = atlas_row(surface);
            let s_uv = select(
                vec2<f32>(0.0),
                vec2<f32>(uv.x * 0.5, row.y + 0.25 * fract(uv.y / 8.0)),
                row.x > 0.5,
            );
            let scale = vec2<f32>(0.5, 0.25 / 8.0) * row.x;
            let a = textureSampleGrad(texture_a, sampler_a, s_uv, uv_dx * scale, uv_dy * scale);
            albedo = albedo * a.r * 2.0;
            if ((flags & FLAG_NORMAL) != 0u) {
                let t = nmap_unpack(textureSampleGrad(texture_b, sampler_b, s_uv, uv_dx * scale, uv_dy * scale));
                let xy = t.xy * 0.5 * fade;
                n = normalize(xy.x * tangent + xy.y * bitangent + n * sqrt(saturate(1.0 - dot(xy, xy))));
            }
        }

        if ((flags & FLAG_MATERIAL) != 0u) {
            // material.hlsl: UV = (surface offset + face coordinates) * tiling.
            let random = part_random(world_from_local);
            var offset = vec2<f32>(0.0);
            if ((flags & FLAG_WHOLE_STUD_OFFSET) != 0u) {
                offset = floor(random * 64.0);
            } else {
                offset = 0.5 * (size.x + size.y + size.z) * (random * 2.0 - 1.0);
            }
            let m_uv = (uv + offset) * material.tex.x;
            let m_dx = uv_dx * material.tex.x;
            let m_dy = uv_dy * material.tex.x;
            var shadow_factor = 1.0;
            if ((flags & FLAG_NORMAL) != 0u) {
                let t = nmap_unpack(textureSampleGrad(texture_b, sampler_b, m_uv, m_dx, m_dy));
                let xy = t.xy * fade;
                shadow_factor = 1.0 + xy.x * material.tex.w;
                n = normalize(xy.x * tangent + xy.y * bitangent + n * sqrt(saturate(1.0 - dot(xy, xy))));
            }
            if ((flags & FLAG_DIFFUSE_CONST) == 0u) {
                let d = textureSampleGrad(texture_a, sampler_a, m_uv, m_dx, m_dy);
                if ((flags & FLAG_BLEND) != 0u) {
                    albedo = mix(vec3<f32>(1.0), albedo, d.a) * d.rgb;
                } else {
                    albedo = albedo * d.rgb;
                }
            }
            albedo = albedo * shadow_factor;
            if ((flags & FLAG_SPECULAR) != 0u) {
                let s = textureSampleGrad(texture_c, sampler_c, m_uv, m_dx, m_dy);
                spec_intensity = mix(spec_intensity, s.r * material.tex.y, fade);
                spec_power = mix(spec_power, s.g * material.tex.z + 0.01, fade);
            }
        }

        if ((flags & FLAG_OUTLINE) != 0u && surface != SURFACE_NO_OUTLINES) {
            // default.hlsl: darken a ~2px band along face edges, fading with distance.
            let d = edge_distance(face, p, size);
            let depth_mul = depth / material.fog.z;
            let outline_fade = saturate(depth_mul * 0.43137 + 0.19608);
            let edges = 0.5 + d * view.clip_from_view[1][1] * view.viewport.w / (8.0 * depth);
            outline = saturate(outline_fade * (1.5 - edges) + edges);
        }
    }

    var rgb = albedo * outline;
    if ((flags & FLAG_UNLIT) == 0u) {
        var light_dir = vec3<f32>(0.0, 1.0, 0.0);
        var lit = 1.0;
        if (view_bindings::lights.n_directional_lights > 0u) {
            let light = view_bindings::lights.directional_lights[0];
            light_dir = light.direction_to_light;
            if ((light.flags & 1u) != 0u) {
                lit = shadows::fetch_directional_shadow(0u, in.world_position, normalize(in.world_normal), view_z, in.position.xy);
            }
        }
        // common.h: shadows only attenuate the key light, by at most 75% and less at low sun.
        let shadow = 1.0 - (1.0 - lit) * pow(saturate(light_dir.y), 0.25) * 0.75;
        let ndotl = dot(n, light_dir);
        let to_view = normalize(view.world_position.xyz - in.world_position.xyz);
        let half_vec = normalize(light_dir + to_view);
        let diffuse = saturate(ndotl) * material.key.rgb + max(-ndotl, 0.0) * material.fill.rgb;
        let specular = material.key.rgb * spec_intensity * step(0.0, ndotl) * shadow
            * pow(saturate(dot(n, half_vec)), spec_power);
        rgb = (material.ambient.rgb + diffuse * shadow) * rgb + specular;
    }

    // default.hlsl: linear fog on view depth; FogStart == FogEnd disables it.
    if (material.fog.x != material.fog.y) {
        let fog_alpha = saturate((material.fog.y - depth) / (material.fog.y - material.fog.x));
        rgb = mix(material.fog_color.rgb, rgb, fog_alpha);
    }

    var out: FragmentOutput;
    out.color = vec4<f32>(to_linear(saturate(rgb)), alpha);
    return out;
}
