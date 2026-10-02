//! The legacy `Lighting` service and the colours the renderer derives from it.
//!
//! Sources: App/v8datamodel/Lighting.cpp (defaults), Rendering/g3d/g3dcpp/LightingParameters.cpp
//! (sun direction and colour splines), Rendering/GfxRender/RenderView.cpp::presetLighting
//! (key/fill/ambient). All colours are non-linear (gamma-space) values: the legacy renderer
//! never converts to linear, tonemaps, or uses sRGB framebuffers.
use bevy::prelude::Vec3;
use blox_model::{Model, ModelError, ModelResult, Property};

const DAY: f32 = 86_400.0;
const SUNRISE: f32 = 21_600.0;
const SUNSET: f32 = 64_800.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    pub ambient: [f32; 3],
    pub brightness: f32,
    pub color_shift_top: [f32; 3],
    pub color_shift_bottom: [f32; 3],
    /// Seconds since midnight.
    pub time_of_day: f32,
    /// Degrees.
    pub geographic_latitude: f32,
    pub fog_color: [f32; 3],
    pub fog_start: f32,
    pub fog_end: f32,
    pub outlines: bool,
}

impl Default for Lighting {
    /// Lighting.cpp constructor defaults.
    fn default() -> Self {
        Self {
            ambient: [0.4; 3],
            brightness: 1.0,
            color_shift_top: [0.0; 3],
            color_shift_bottom: [0.0; 3],
            time_of_day: 14.0 * 3600.0,
            geographic_latitude: 41.7333,
            fog_color: [0.75; 3],
            fog_start: 0.0,
            fog_end: 100_000.0,
            outlines: true,
        }
    }
}

/// What the part shader needs, derived once from [`Lighting`].
#[derive(Clone, Debug, PartialEq)]
pub struct Shading {
    /// Unit vector from the surface toward the light (sun, or moon after dark).
    pub direction_to_light: Vec3,
    pub ambient: [f32; 3],
    /// `Lamp0Color`: the directional key light.
    pub key: [f32; 3],
    /// `Lamp1Color`: the fill light from the opposite direction.
    pub fill: [f32; 3],
    /// Sky tint (`skyAmbient`) and the colour the framebuffer is cleared to.
    pub sky_ambient: [f32; 3],
}

impl Lighting {
    pub fn from_model(model: &Model) -> ModelResult<Self> {
        let mut lighting = Self::default();
        let Some(item) = model.instances.iter().find(|i| i.class == "Lighting") else {
            return Ok(lighting);
        };
        let bad = |name: &str| ModelError(format!("Lighting.{name} has an incompatible type"));
        let color = |name: &str, target: &mut [f32; 3]| match item.properties.get(name) {
            Some(Property::Color3(value)) => {
                *target = *value;
                Ok(())
            }
            None => Ok(()),
            Some(_) => Err(bad(name)),
        };
        let number = |name: &str, target: &mut f32| match item.properties.get(name) {
            Some(Property::Float(v)) => {
                *target = *v as f32;
                Ok(())
            }
            Some(Property::Int(v)) => {
                *target = *v as f32;
                Ok(())
            }
            Some(Property::Uint(v)) => {
                *target = *v as f32;
                Ok(())
            }
            None => Ok(()),
            Some(_) => Err(bad(name)),
        };
        color("Ambient", &mut lighting.ambient)?;
        color("ColorShift_Top", &mut lighting.color_shift_top)?;
        color("ColorShift_Bottom", &mut lighting.color_shift_bottom)?;
        color("FogColor", &mut lighting.fog_color)?;
        number("Brightness", &mut lighting.brightness)?;
        number("GeographicLatitude", &mut lighting.geographic_latitude)?;
        number("FogStart", &mut lighting.fog_start)?;
        number("FogEnd", &mut lighting.fog_end)?;
        match item.properties.get("Outlines") {
            Some(Property::Bool(v)) => lighting.outlines = *v,
            None => {}
            Some(_) => return Err(bad("Outlines")),
        }
        match item.properties.get("TimeOfDay") {
            Some(Property::Text(text)) => lighting.time_of_day = parse_time_of_day(text)?,
            None => {}
            Some(_) => return Err(bad("TimeOfDay")),
        }
        let finite = lighting
            .ambient
            .iter()
            .chain(&lighting.fog_color)
            .chain(&lighting.color_shift_top)
            .chain(&lighting.color_shift_bottom)
            .chain([
                &lighting.brightness,
                &lighting.geographic_latitude,
                &lighting.fog_start,
                &lighting.fog_end,
            ])
            .all(|v| v.is_finite());
        if !finite || lighting.fog_end < lighting.fog_start {
            return Err(ModelError(
                "Lighting contains non-finite or inverted values".into(),
            ));
        }
        Ok(lighting)
    }

    pub fn shading(&self) -> Shading {
        let t = self.time_of_day.rem_euclid(DAY);
        let sun = true_sun_position(t, self.geographic_latitude.to_radians());
        // `lightDirection` is the sun while it is above -0.3, else the moon. The moon phase is not
        // serialized; a full moon opposite the sun is used.
        let direction_to_light = if sun.y > -0.3 { sun } else { -sun };
        let sun_color = scale(light_color(t), 0.9);
        let brightness = self.brightness.clamp(0.05, 5.0);
        let shift = |color: [f32; 3]| {
            let average = (color[0] + color[1] + color[2]) * 0.333_333_3;
            color.map(|c| c - average)
        };
        let lit = |base: [f32; 3], shift: [f32; 3]| {
            let length = Vec3::from_array(base).length();
            std::array::from_fn(|i| (base[i] + shift[i] * length).clamp(0.0, 1.0))
        };
        Shading {
            direction_to_light,
            ambient: self.ambient,
            key: lit(scale(sun_color, brightness), shift(self.color_shift_top)),
            fill: lit(
                scale(sun_color, brightness * 0.4),
                shift(self.color_shift_bottom),
            ),
            sky_ambient: sky_ambient(t),
        }
    }
}

fn scale(color: [f32; 3], factor: f32) -> [f32; 3] {
    color.map(|c| c * factor)
}

/// `HH:MM:SS`, as serialized (hours may exceed 24 and wrap).
pub fn parse_time_of_day(text: &str) -> ModelResult<f32> {
    let bad = || ModelError(format!("invalid Lighting.TimeOfDay {text:?}"));
    let mut parts = text.trim().split(':');
    let mut next = || -> ModelResult<f32> {
        let value: f32 = parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        if value.is_finite() && value >= 0.0 {
            Ok(value)
        } else {
            Err(bad())
        }
    };
    let (hours, minutes, seconds) = (next()?, next()?, next()?);
    if parts.next().is_some() || minutes >= 60.0 || seconds >= 60.0 {
        return Err(bad());
    }
    Ok((hours * 3600.0 + minutes * 60.0 + seconds).rem_euclid(DAY))
}

/// LightingParameters.cpp: the sun circles in the XY plane and is tilted by the season (the
/// serializer pins day-of-year to 0, the winter solstice) and the observer's latitude.
fn true_sun_position(t: f32, latitude: f32) -> Vec3 {
    let angle = std::f32::consts::TAU * t / DAY;
    let sun = Vec3::new(angle.sin(), -angle.cos(), 0.0);
    let day_of_year = 0.0_f32;
    let offset = -23.5_f32.to_radians()
        * (std::f32::consts::PI * (day_of_year - 182.6282) / 182.6282).cos()
        - latitude;
    let axis = Vec3::Z.cross(sun).normalize_or_zero();
    // Rodrigues rotation; `axis` is perpendicular to `sun`.
    (sun * offset.cos() + axis.cross(sun) * offset.sin()).normalize()
}

fn spline(points: &[(f32, [f32; 3])], t: f32) -> [f32; 3] {
    let first = points[0];
    if t <= first.0 {
        return first.1;
    }
    for pair in points.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t <= t1 {
            let f = (t - t0) / (t1 - t0);
            return std::array::from_fn(|i| c0[i] + (c1[i] - c0[i]) * f);
        }
    }
    points[points.len() - 1].1
}

fn gray(v: f32) -> [f32; 3] {
    [v; 3]
}

/// LightingParameters.cpp: colour of the sun/moon light over the day.
pub fn light_color(t: f32) -> [f32; 3] {
    spline(
        &[
            (0.0, gray(0.2)),
            (SUNRISE - 3600.0, gray(0.1)),
            (SUNRISE, gray(0.0)),
            (SUNRISE + 450.0, [0.6, 0.6, 0.0]),
            (SUNRISE + 1800.0, gray(0.75)),
            (SUNSET - 1800.0, gray(0.75)),
            (SUNSET - 900.0, [0.1, 0.1, 0.075]),
            (SUNSET, [0.1, 0.05, 0.05]),
            (SUNSET + 1800.0, gray(0.1)),
            (DAY, gray(0.2)),
        ],
        t,
    )
}

/// LightingParameters.cpp: sky tint, also the framebuffer clear colour.
pub fn sky_ambient(t: f32) -> [f32; 3] {
    let h = 3600.0;
    spline(
        &[
            (0.0, gray(0.0)),
            (4.0 * h, gray(0.0)),
            (5.0 * h, [0.07, 0.07, 0.1]),
            (5.5 * h, [0.2, 0.15, 0.01]),
            (6.0 * h, [0.2, 0.15, 0.01]),
            (6.5 * h, gray(1.0)),
            (17.5 * h, gray(1.0)),
            (18.0 * h, [0.4, 0.2, 0.05]),
            (18.0 * h + 1200.0, gray(0.0)),
            (DAY, gray(0.0)),
        ],
        t,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn afternoon_sun_matches_the_traced_worked_example() {
        let lighting = Lighting {
            time_of_day: parse_time_of_day("14:06:00").unwrap(),
            ..Lighting::default()
        };
        let shading = lighting.shading();
        // (-0.496, 0.810, 0.313) from LightingParameters.cpp at latitude 41.7333.
        assert!(
            shading
                .direction_to_light
                .abs_diff_eq(Vec3::new(-0.496, 0.810, 0.313), 2e-3),
            "{:?}",
            shading.direction_to_light
        );
        // sunColor 0.75 * 0.9 = 0.675 key, fill = 0.4 * key.
        for c in shading.key {
            assert!((c - 0.675).abs() < 1e-5);
        }
        for c in shading.fill {
            assert!((c - 0.27).abs() < 1e-5);
        }
        assert_eq!(shading.sky_ambient, [1.0; 3]);
    }

    #[test]
    fn midnight_uses_the_moon_above_the_horizon_and_a_dim_light() {
        let shading = Lighting {
            time_of_day: 0.0,
            ..Lighting::default()
        }
        .shading();
        assert!(
            shading.direction_to_light.y > 0.0,
            "{:?}",
            shading.direction_to_light
        );
        assert!(shading.key.iter().all(|c| (c - 0.18).abs() < 1e-5));
        assert_eq!(shading.sky_ambient, [0.0; 3]);
    }

    #[test]
    fn color_shift_tints_by_deviation_from_gray_scaled_by_light_strength() {
        let base = Lighting::default().shading().key;
        let shifted = Lighting {
            color_shift_top: [0.3, 0.0, 0.0],
            ..Lighting::default()
        }
        .shading()
        .key;
        let strength = Vec3::from_array(base).length();
        assert!((shifted[0] - (base[0] + 0.2 * strength)).abs() < 1e-5);
        assert!((shifted[1] - (base[1] - 0.1 * strength)).abs() < 1e-5);
    }

    #[test]
    fn time_of_day_rejects_malformed_text() {
        assert_eq!(parse_time_of_day("06:30:00").unwrap(), 23_400.0);
        assert_eq!(parse_time_of_day("25:00:00").unwrap(), 3600.0);
        for bad in ["", "14:06", "14:61:00", "a:b:c", "-1:00:00", "1:2:3:4"] {
            assert!(parse_time_of_day(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn real_place_lighting_decodes_ambient_and_time() {
        let xml = r#"<roblox version="4"><Item class="Lighting" referent="l"><Properties>
          <Color3 name="Ambient">4289703855</Color3><float name="Brightness">1</float>
          <string name="TimeOfDay">14:06:00</string></Properties></Item></roblox>"#;
        let lighting = Lighting::from_model(&Model::decode(xml.as_bytes()).unwrap()).unwrap();
        assert!((lighting.ambient[0] - 175.0 / 255.0).abs() < 1e-5);
        assert_eq!(lighting.time_of_day, 14.0 * 3600.0 + 360.0);
        assert!(lighting.outlines);
        let bad = xml.replace("14:06:00", "late");
        assert!(Lighting::from_model(&Model::decode(bad.as_bytes()).unwrap()).is_err());
    }
}
