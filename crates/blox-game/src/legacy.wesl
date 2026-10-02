// Port of the legacy forward "HQ" part shader (shaders/source/default.hlsl, plastic.hlsl,
// common.h). Colours are non-linear (gamma-space) throughout, exactly like the original, which
// has no tonemap or sRGB framebuffer; only the final colour is converted so that Bevy's sRGB
// framebuffer writes the same numbers the original would have.
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
    // x: FogStart, y: FogEnd, z: shading distance (outline fade), w: unused.
    fog: vec4<f32>,
    // x: specular intensity, y: specular power.
    spec: vec4<f32>,
    // x: six 4-bit SurfaceType values in NormalId order, y: flags.
    packed: vec4<u32>,
}

const FLAG_STUDS: u32 = 1u;
const FLAG_OUTLINE: u32 = 2u;
const FLAG_TEXTURE: u32 = 4u;
const FLAG_UNLIT: u32 = 8u;

const SURFACE_STUDS: u32 = 3u;
const SURFACE_INLET: u32 = 4u;
const SURFACE_NO_OUTLINES: u32 = 10u;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: Legacy;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var surface_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var surface_sampler: sampler;

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
// (GeometryGenerator.cpp::addBlock), and the distance to the nearest face edge.
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

fn edge_distance(face: u32, p: vec3<f32>, size: vec3<f32>) -> f32 {
    let h = 0.5 * size - abs(p);
    if (face == 0u || face == 3u) { return min(h.y, h.z); }
    if (face == 1u || face == 4u) { return min(h.x, h.z); }
    return min(h.x, h.y);
}

// The legacy stud/inlet atlas is not shipped in this repository, so the relief is drawn
// procedurally: a raised disc with a dark rim, or the inverse for inlets.
fn stud_factor(surface: u32, uv: vec2<f32>, depth: f32) -> f32 {
    let cell = fract(uv) - vec2<f32>(0.5);
    let r = length(cell);
    let w = max(fwidth(r), 0.002);
    let disc = 1.0 - smoothstep(0.30 - w, 0.30 + w, r);
    let rim = smoothstep(0.30 - w, 0.30 + w, r) * (1.0 - smoothstep(0.40 - w, 0.40 + w, r));
    var f = 1.0;
    if (surface == SURFACE_STUDS) {
        f = 1.0 + 0.10 * disc - 0.22 * rim;
    } else {
        f = 1.0 - 0.20 * disc + 0.10 * rim;
    }
    return mix(f, 1.0, saturate(depth / 150.0));
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

    var albedo = material.color.rgb;
    var alpha = material.color.a;
    if ((flags & FLAG_TEXTURE) != 0u) {
        let t = textureSample(surface_texture, surface_sampler, in.uv);
        albedo = albedo * t.rgb;
        alpha = alpha * t.a;
    }

    var outline = 1.0;
    if ((flags & (FLAG_STUDS | FLAG_OUTLINE)) != 0u) {
        let world_from_local = get_world_from_local(in.instance_index);
        let local_from_world = get_local_from_world(in.instance_index);
        let size = vec3<f32>(
            length(world_from_local[0].xyz),
            length(world_from_local[1].xyz),
            length(world_from_local[2].xyz),
        );
        let p = (local_from_world * in.world_position).xyz * size;
        let n_local = transpose(mat3x3<f32>(
            world_from_local[0].xyz, world_from_local[1].xyz, world_from_local[2].xyz,
        )) * in.world_normal;
        let face = face_of(n_local);
        let surface = (material.packed.x >> (4u * face)) & 15u;
        if ((flags & FLAG_STUDS) != 0u && (surface == SURFACE_STUDS || surface == SURFACE_INLET)) {
            albedo = albedo * stud_factor(surface, face_uv(face, p, size), depth);
        }
        if ((flags & FLAG_OUTLINE) != 0u && surface != SURFACE_NO_OUTLINES) {
            // default.hlsl: darken a ~2px band along face edges, fading with distance.
            let d = edge_distance(face, p, size);
            let depth_mul = depth / material.fog.z;
            let fade = saturate(depth_mul * 0.43137 + 0.19608);
            let edges = 0.5 + d * view.clip_from_view[1][1] * view.viewport.w / (8.0 * depth);
            outline = saturate(fade * (1.5 - edges) + edges);
        }
    }

    var rgb = albedo * outline;
    if ((flags & FLAG_UNLIT) == 0u) {
        var n = normalize(in.world_normal);
        if (!is_front) { n = -n; }
        var light_dir = vec3<f32>(0.0, 1.0, 0.0);
        var lit = 1.0;
        if (view_bindings::lights.n_directional_lights > 0u) {
            let light = view_bindings::lights.directional_lights[0];
            light_dir = light.direction_to_light;
            if ((light.flags & 1u) != 0u) {
                lit = shadows::fetch_directional_shadow(0u, in.world_position, n, view_z, in.position.xy);
            }
        }
        // common.h: shadows only attenuate the key light, by at most 75% and less at low sun.
        let shadow = 1.0 - (1.0 - lit) * pow(saturate(light_dir.y), 0.25) * 0.75;
        let ndotl = dot(n, light_dir);
        let to_view = normalize(view.world_position.xyz - in.world_position.xyz);
        let half_vec = normalize(light_dir + to_view);
        let diffuse = saturate(ndotl) * material.key.rgb + max(-ndotl, 0.0) * material.fill.rgb;
        let specular = material.key.rgb * material.spec.x * step(0.0, ndotl) * shadow
            * pow(saturate(dot(n, half_vec)), material.spec.y);
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
