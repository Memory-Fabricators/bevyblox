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
    // x: specular intensity, y: specular power. Materials put CFG_SPECULAR_LOD and
    // CFG_GLOSS_LOD here: the far-distance values the specular map fades to.
    spec: vec4<f32>,
    // x: texture repeats per stud (getTiling), y: CFG_SPECULAR_SCALE, z: CFG_GLOSS_SCALE,
    // w: CFG_NORMAL_SHADOW_SCALE.
    tex: vec4<f32>,
    // x: normal detail tiling (CFG_NORMAL_DETAIL_TILING; plastic: its 1.3 Uv tiling),
    // y: CFG_NORMAL_DETAIL_SCALE, z: CFG_FAR_TILING, w: unused.
    detail: vec4<f32>,
    // x: CFG_FAR_DIFFUSE_CUTOFF, y: CFG_FAR_NORMAL_CUTOFF, z: CFG_FAR_SPECULAR_CUTOFF, w: unused.
    far: vec4<f32>,
    // x: six 4-bit SurfaceType values in NormalId order, y: flags.
    packed: vec4<u32>,
}

const FLAG_STUDS: u32 = 1u;        // plastic: texture 1 is the stud atlas, texture 3 its normal map
const FLAG_OUTLINE: u32 = 2u;
const FLAG_TEXTURE: u32 = 4u;      // decal / mesh texture on texture 1, sampled with mesh UVs
const FLAG_UNLIT: u32 = 8u;        // neon
const FLAG_MATERIAL: u32 = 16u;    // texture 1 is a tiled material diffuse map
const FLAG_BLEND: u32 = 32u;       // CFG_OPT_BLEND_COLOR
const FLAG_DIFFUSE_CONST: u32 = 64u;
const FLAG_NORMAL: u32 = 128u;     // texture 3 is a normal map
const FLAG_SPECULAR: u32 = 256u;   // texture 5 is a specular map
const FLAG_WHOLE_STUD_OFFSET: u32 = 512u;
const FLAG_DETAIL: u32 = 1024u;    // texture 7 is the normal detail map

// RenderView.cpp: OutlineBrightnessMax 160 and OutlineBrightnessMin 50, both over 255.
const OUTLINE_BRIGHTNESS_MUL: f32 = 110.0 / 255.0;
const OUTLINE_BRIGHTNESS_ADD: f32 = 50.0 / 255.0;

const SURFACE_NO_OUTLINES: u32 = 10u;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: Legacy;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var texture_a: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var sampler_a: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var texture_b: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var sampler_b: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var texture_c: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var sampler_c: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var texture_d: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var sampler_d: sampler;

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

// NormalId order: +X 0, +Y 1, +Z 2, -X 3, -Y 4, -Z 5. The four faces sharing an edge with `face`,
// in computeOutlineOffsetPart order (GeometryGenerator.cpp).
fn edge_neighbours(face: u32) -> vec4<u32> {
    switch face {
        case 0u: { return vec4<u32>(5u, 2u, 1u, 4u); }
        case 1u: { return vec4<u32>(3u, 0u, 2u, 5u); }
        case 2u: { return vec4<u32>(0u, 3u, 1u, 4u); }
        case 3u: { return vec4<u32>(2u, 5u, 1u, 4u); }
        case 4u: { return vec4<u32>(3u, 0u, 5u, 2u); }
        default: { return vec4<u32>(3u, 0u, 1u, 4u); }
    }
}

// Distance in studs from the face point to its nearest outline edge. An edge shared with a
// NO_OUTLINES face is pushed out by 10 studs (NO_OUTLINES), as computeOutlineOffsetPart does.
fn min_edge_distance(face: u32, p: vec3<f32>, size: vec3<f32>, surfaces: u32) -> f32 {
    let neighbours = edge_neighbours(face);
    var nearest = 1.0e9;
    for (var k = 0u; k < 4u; k++) {
        let n = neighbours[k];
        let axis = n % 3u;
        let sign = select(-1.0, 1.0, n < 3u);
        var d = 0.5 * size[axis] - sign * p[axis];
        if (((surfaces >> (4u * n)) & 15u) == SURFACE_NO_OUTLINES) {
            d += 10.0;
        }
        nearest = min(nearest, d);
    }
    return nearest;
}

// material.hlsl sampleFar: beyond the fade the map is sampled at CFG_FAR_TILING and cross-faded
// back to the near sample as fade drops below 1 - cutoff. A zero cutoff never tiles.
fn sample_far(
    t: texture_2d<f32>,
    s: sampler,
    uv: vec2<f32>,
    dx: vec2<f32>,
    dy: vec2<f32>,
    fade: f32,
    cutoff: f32,
    far_tiling: f32,
) -> vec4<f32> {
    let near = textureSampleGrad(t, s, uv, dx, dy);
    if (cutoff == 0.0) {
        return near;
    }
    let cscale = 1.0 / (1.0 - cutoff);
    let far = textureSampleGrad(t, s, uv * far_tiling, dx * far_tiling, dy * far_tiling);
    return mix(far, near, saturate(fade * cscale - cutoff * cscale));
}

// default.hlsl linear fog on view depth. updateFog keeps FogStart below FogEnd, so the range is
// never empty; equal values mean no fog.
fn fog_alpha(depth: f32) -> f32 {
    if (material.fog.x == material.fog.y) {
        return 1.0;
    }
    return saturate((material.fog.y - depth) / (material.fog.y - material.fog.x));
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
    // default.hlsl fade: 1 at the camera, 0 at the shading distance. Detail and far maps fade with it.
    let fade = saturate(1.0 - depth / material.fog.z);

    var out: FragmentOutput;
    if ((flags & FLAG_UNLIT) != 0u) {
        // neon.hlsl: the flat part colour (glow 1), then the fog blend. No outline or lighting.
        out.color = vec4<f32>(
            to_linear(saturate(mix(material.fog_color.rgb, material.color.rgb, fog_alpha(depth)))),
            material.color.a,
        );
        return out;
    }

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
        // Differentiate position before selecting a projection. Adjacent pixels can choose
        // different faces on curved meshes; differentiating their UVs would spike the mip.
        let uv_dx = face_uv(face, p + dpdx(p), size) - uv;
        let uv_dy = face_uv(face, p + dpdy(p), size) - uv;
        // Project the U direction onto the actual surface, including wedge slopes and
        // curved geometry. Dominant-axis selection keeps this vector away from zero. The frame
        // comes from the vertex normal, before any normal map perturbs `n`.
        let projected_tangent = basis * face_tangent(face);
        let tangent = normalize(projected_tangent - n * dot(n, projected_tangent));
        let bitangent = cross(n, tangent);

        if ((flags & FLAG_STUDS) != 0u) {
            // plastic.hlsl (PIN_HQ): albedo = Color * studs.r * 2 from this surface's atlas row.
            // The stud normal map is used at full strength. Detail noise is scaled by
            // Color.a - 0.5 (CFG_BUMP_INTENSITY 0.5). z is not faded; only x and y are.
            let row = atlas_row(surface);
            let s_uv = select(
                vec2<f32>(0.0),
                vec2<f32>(uv.x * 0.5, row.y + 0.25 * fract(uv.y / 8.0)),
                row.x > 0.5,
            );
            let scale = vec2<f32>(0.5, 0.25 / 8.0) * row.x;
            let a = textureSampleGrad(texture_a, sampler_a, s_uv, uv_dx * scale, uv_dy * scale);
            albedo = albedo * a.r * 2.0;
            var t = vec3<f32>(0.0, 0.0, 1.0);
            if ((flags & FLAG_NORMAL) != 0u) {
                t = nmap_unpack(textureSampleGrad(texture_b, sampler_b, s_uv, uv_dx * scale, uv_dy * scale));
            }
            var xy = t.xy;
            if ((flags & FLAG_DETAIL) != 0u) {
                let k = material.detail.x;
                let noise = nmap_unpack(textureSampleGrad(texture_d, sampler_d, uv * k, uv_dx * k, uv_dy * k));
                xy += noise.xy * saturate(material.color.a - 0.5);
            }
            xy = xy * fade;
            n = normalize(xy.x * tangent + xy.y * bitangent + t.z * n);
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
            let tiling = material.tex.x;
            let far_tiling = material.detail.z;
            let m_uv = (uv + offset) * tiling;
            let m_dx = uv_dx * tiling;
            let m_dy = uv_dy * tiling;

            var t = vec3<f32>(0.0, 0.0, 1.0);
            if ((flags & FLAG_NORMAL) != 0u) {
                t = nmap_unpack(sample_far(texture_b, sampler_b, m_uv, m_dx, m_dy, fade, material.far.y, far_tiling));
            }
            var xy = t.xy;
            if ((flags & FLAG_DETAIL) != 0u) {
                // Applied even when the normal map is constant (CFG_OPT_NORMAL_CONST).
                let k = material.detail.x;
                let detail = nmap_unpack(textureSampleGrad(texture_d, sampler_d, m_uv * k, m_dx * k, m_dy * k));
                xy += detail.xy * material.detail.y;
            }
            xy = xy * fade;
            let shadow_factor = 1.0 + xy.x * material.tex.w;
            n = normalize(xy.x * tangent + xy.y * bitangent + t.z * n);

            if ((flags & FLAG_DIFFUSE_CONST) == 0u) {
                let d = sample_far(texture_a, sampler_a, m_uv, m_dx, m_dy, fade, material.far.x, far_tiling);
                if ((flags & FLAG_BLEND) != 0u) {
                    albedo = mix(vec3<f32>(1.0), albedo, d.a) * d.rgb;
                } else {
                    albedo = albedo * d.rgb;
                }
            }
            albedo = albedo * shadow_factor;
            if ((flags & FLAG_SPECULAR) != 0u) {
                let s = sample_far(texture_c, sampler_c, m_uv, m_dx, m_dy, fade, material.far.z, far_tiling).rg;
                // specfade = lerp((CFG_SPECULAR_LOD, CFG_GLOSS_LOD), specular * scales + (0, 0.01), fade)
                spec_intensity = mix(material.spec.x, s.r * material.tex.y, fade);
                spec_power = mix(material.spec.y, s.g * material.tex.z + 0.01, fade);
            }
        }

        if ((flags & FLAG_OUTLINE) != 0u && surface != SURFACE_NO_OUTLINES) {
            // default.hlsl PIN_HQ: outlineFade = saturate(depth / shading * mul + add), and the
            // nearest edge in pixels (outlineScaling = fov * height / 8 at OutlineThickness 40).
            let outline_fade = saturate(depth / material.fog.z * OUTLINE_BRIGHTNESS_MUL + OUTLINE_BRIGHTNESS_ADD);
            let d = min_edge_distance(face, p, size, material.packed.x);
            let edges = 0.5 + d * view.clip_from_view[1][1] * view.viewport.w / (8.0 * depth);
            outline = saturate(outline_fade * (1.5 - edges) + edges);
        }
    }

    // common.h: shadows attenuate the key light only, by at most 75% and less at low sun.
    var light_dir = vec3<f32>(0.0, 1.0, 0.0);
    var lit = 1.0;
    if (view_bindings::lights.n_directional_lights > 0u) {
        let light = view_bindings::lights.directional_lights[0];
        light_dir = light.direction_to_light;
        if ((light.flags & 1u) != 0u) {
            lit = shadows::fetch_directional_shadow(0u, in.world_position, normalize(in.world_normal), view_z, in.position.xy);
        }
    }
    let shadow = 1.0 - (1.0 - lit) * pow(saturate(light_dir.y), 0.25) * 0.75;

    // default.hlsl: (ambient + diffuse * shadow) * albedo + specular, then the outline darkens all
    // of it, then the fog blend.
    let ndotl = dot(n, light_dir);
    let to_view = normalize(view.world_position.xyz - in.world_position.xyz);
    let half_vec = normalize(light_dir + to_view);
    let diffuse = saturate(ndotl) * material.key.rgb + max(-ndotl, 0.0) * material.fill.rgb;
    let specular = material.key.rgb * spec_intensity * step(0.0, ndotl) * shadow
        * pow(saturate(dot(n, half_vec)), spec_power);
    var rgb = ((material.ambient.rgb + diffuse * shadow) * albedo + specular) * outline;

    rgb = mix(material.fog_color.rgb, rgb, fog_alpha(depth));

    var out_lit: FragmentOutput;
    out_lit.color = vec4<f32>(to_linear(saturate(rgb)), alpha);
    return out_lit;
}
