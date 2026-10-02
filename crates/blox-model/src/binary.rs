//! Version-zero format from App/v8xml/SerializerBinary.cpp. All byte readers
//! are bounded; numeric vectors are views of the four interleaved byte planes.
use super::{Model, ModelError, ModelInstance, ModelResult, Property};
use std::{borrow::Cow, collections::HashMap};

fn error(message: impl Into<String>) -> ModelError {
    ModelError(message.into())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
    fn take(&mut self, count: usize) -> ModelResult<&'a [u8]> {
        if count > self.remaining() {
            return Err(error("Truncated binary model data"));
        }
        let start = self.offset;
        self.offset += count;
        Ok(&self.bytes[start..self.offset])
    }
    fn byte(&mut self) -> ModelResult<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> ModelResult<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> ModelResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> ModelResult<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn f64(&mut self) -> ModelResult<f64> {
        let b = self.take(8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn string_bytes(&mut self) -> ModelResult<&'a [u8]> {
        let count = self.u32()? as usize;
        self.take(count)
    }
    fn string(&mut self) -> ModelResult<String> {
        String::from_utf8(self.string_bytes()?.to_vec())
            .map_err(|_| error("Invalid UTF-8 in binary model identifier"))
    }
    fn words(&mut self, count: usize) -> ModelResult<Words<'a>> {
        let size = count
            .checked_mul(4)
            .ok_or_else(|| error("Numeric vector size overflow"))?;
        Ok(Words {
            bytes: self.take(size)?,
            count,
        })
    }
    fn finish(&self) -> ModelResult<()> {
        if self.remaining() != 0 {
            return Err(error("Unexpected trailing bytes in binary model chunk"));
        }
        Ok(())
    }
}

struct Words<'a> {
    bytes: &'a [u8],
    count: usize,
}
impl Words<'_> {
    fn uint(&self, i: usize) -> u32 {
        u32::from_be_bytes([
            self.bytes[i],
            self.bytes[self.count + i],
            self.bytes[self.count * 2 + i],
            self.bytes[self.count * 3 + i],
        ])
    }
    fn int(&self, i: usize) -> i32 {
        let n = self.uint(i);
        ((n >> 1) as i32) ^ -((n & 1) as i32)
    }
    fn float(&self, i: usize) -> f32 {
        f32::from_bits(self.uint(i).rotate_right(1))
    }
    fn id(&self, i: usize, last: &mut i32) -> ModelResult<i32> {
        *last = last
            .checked_add(self.int(i))
            .ok_or_else(|| error("Referent delta overflow"))?;
        Ok(*last)
    }
}

fn reserve<T>(values: &mut Vec<T>, count: usize) -> ModelResult<()> {
    values
        .try_reserve_exact(count)
        .map_err(|_| error("Binary model allocation is too large"))
}

// Native LZ4 block decoding, following App/lz4/lz4_decoder.h (included by
// App/lz4/lz4.c). No prefix dictionary, frame wrapper, or external compressor.
fn lz4(bytes: &[u8], size: usize) -> ModelResult<Vec<u8>> {
    if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) || bytes.starts_with(&[0x04, 0x22, 0x4d, 0x18])
    {
        return Err(error(
            "Unsupported binary model compression (expected a raw LZ4 block)",
        ));
    }
    // Each extension byte can account for at most 255 output bytes. Check the
    // advertised size before reserving, rather than trusting a chunk header.
    let bound = bytes
        .len()
        .checked_mul(255)
        .and_then(|n| n.checked_add(16))
        .ok_or_else(|| error("LZ4 size overflow"))?;
    if size > bound {
        return Err(error("Impossible LZ4 decompressed size"));
    }
    let mut input = Reader::new(bytes);
    let mut out = Vec::new();
    reserve(&mut out, size)?;
    let mut matched = false;
    loop {
        let token = input.byte()?;
        let literals = lz4_length(&mut input, (token >> 4) as usize)?;
        if literals > size - out.len() {
            return Err(error("LZ4 literal run exceeds output size"));
        }
        out.extend_from_slice(input.take(literals)?);
        if input.remaining() == 0 {
            if out.len() != size || (matched && literals < 5) {
                return Err(error("Malformed LZ4 final literal run"));
            }
            return Ok(out);
        }
        // The reference requires the last match to begin at least 12 bytes
        // before the end and the last five output bytes to be literals.
        if size - out.len() < 12 {
            return Err(error("Malformed LZ4 match near end of block"));
        }
        let offset = input.u16()? as usize;
        if offset == 0 || offset > out.len() {
            return Err(error("Invalid LZ4 match offset"));
        }
        let mut length = lz4_length(&mut input, (token & 15) as usize)?
            .checked_add(4)
            .ok_or_else(|| error("LZ4 match size overflow"))?;
        if length > size - out.len() - 5 {
            return Err(error("LZ4 match exceeds output size"));
        }
        let start = out.len() - offset;
        // Grow the already decoded pattern, including overlapping matches.
        // Each copy reads only initialized bytes, and the reserved output
        // capacity prevents reallocating during expansion.
        while length != 0 {
            let count = length.min(out.len() - start);
            out.extend_from_within(start..start + count);
            length -= count;
        }
        matched = true;
    }
}

fn lz4_length(input: &mut Reader<'_>, initial: usize) -> ModelResult<usize> {
    let mut length = initial;
    if initial == 15 {
        loop {
            let next = input.byte()? as usize;
            length = length
                .checked_add(next)
                .ok_or_else(|| error("LZ4 run length overflow"))?;
            if next != 255 {
                break;
            }
        }
    }
    Ok(length)
}

fn orientation(encoded: u8) -> ModelResult<[[f32; 3]; 3]> {
    // Math::idToMatrix3 uses NormalId order +X,+Y,+Z,-X,-Y,-Z
    // for the first two columns and their cross product for the third.
    let id = encoded
        .checked_sub(1)
        .ok_or_else(|| error("Invalid CFrame orientation ID"))? as usize;
    let x = id / 6;
    let y = id % 6;
    if x >= 6 || x % 3 == y % 3 {
        return Err(error("Invalid CFrame orientation ID"));
    }
    const NORMALS: [[f32; 3]; 6] = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [-1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, -1.0],
    ];
    let a = NORMALS[x];
    let b = NORMALS[y];
    let c = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    Ok([[a[0], b[0], c[0]], [a[1], b[1], c[1]], [a[2], b[2], c[2]]])
}

fn rotation(input: &mut Reader<'_>, quaternion: bool) -> ModelResult<[[f32; 3]; 3]> {
    let encoded = input.byte()?;
    if encoded != 0 {
        return orientation(encoded);
    }
    if !quaternion {
        let mut matrix = [[0.0; 3]; 3];
        for row in &mut matrix {
            for value in row {
                *value = input.f32()?;
            }
        }
        return Ok(matrix);
    }
    // RBX::Quaternion stores x,y,z,w; conversion intentionally does not
    // renormalize, matching App/util/Quaternion.cpp::toRotationMatrix.
    let x = input.f32()?;
    let y = input.f32()?;
    let z = input.f32()?;
    let w = input.f32()?;
    let xc = x * 2.0;
    let yc = y * 2.0;
    let zc = z * 2.0;
    let (xx, xy, xz) = (x * xc, x * yc, x * zc);
    let (wx, wy, wz) = (w * xc, w * yc, w * zc);
    let (yy, yz, zz) = (y * yc, y * zc, z * zc);
    Ok([
        [1.0 - (yy + zz), xy - wz, xz + wy],
        [xy + wz, 1.0 - (xx + zz), yz - wx],
        [xz - wy, yz + wx, 1.0 - (xx + yy)],
    ])
}

fn type_name(format: u8) -> String {
    const NAMES: [&str; 26] = [
        "Unknown",
        "String",
        "Bool",
        "Int",
        "Float",
        "Double",
        "UDim",
        "UDim2",
        "Ray",
        "Faces",
        "Axes",
        "BrickColor",
        "Color3",
        "Vector2",
        "Vector3",
        "Vector2int16",
        "CFrameMatrix",
        "CFrameQuat",
        "Enum",
        "Ref",
        "Vector3int16",
        "NumberSequence",
        "ColorSequenceV1",
        "NumberRange",
        "Rect2D",
        "PhysicalProperties",
    ];
    NAMES
        .get(format as usize)
        .map(|s| (*s).to_owned())
        .unwrap_or_else(|| format!("Unknown({format})"))
}

fn rendering_property(name: &str) -> bool {
    matches!(
        name,
        "Name"
            | "CFrame"
            | "CoordinateFrame"
            | "Size"
            | "size"
            | "Shape"
            | "shape"
            | "BrickColor"
            | "Color3"
            | "Color"
            | "Color3uint8"
            | "Transparency"
            | "Reflectance"
            | "FieldOfView"
            | "Scale"
            | "Offset"
            | "MeshType"
            | "MeshId"
            | "Source"
    )
}

fn set_property(instance: &mut ModelInstance, name: &str, value: Property) -> ModelResult<()> {
    if name == "Name" {
        match &value {
            Property::Text(text) => instance.name.clone_from(text),
            _ => return Err(error("Instance Name is not a text string")),
        }
    }
    if instance.properties.insert(name.to_owned(), value).is_some() {
        return Err(error(format!("Duplicate binary property {name}")));
    }
    Ok(())
}

fn properties(
    input: &mut Reader<'_>,
    name: &str,
    format: u8,
    indices: &[usize],
    model: &mut Model,
) -> ModelResult<()> {
    let count = indices.len();
    match format {
        1 => {
            if count > input.remaining() / 4 {
                return Err(error("Truncated string property vector"));
            }
            for &index in indices {
                let bytes = input.string_bytes()?;
                let value = match std::str::from_utf8(bytes) {
                    Ok(text) => Property::Text(text.to_owned()),
                    Err(_) if rendering_property(name) => {
                        return Err(error(format!("Invalid UTF-8 in {name}")));
                    }
                    Err(_) => Property::Other("BinaryString".to_owned()),
                };
                set_property(&mut model.instances[index], name, value)?;
            }
        }
        2 | 9 | 10 => {
            let bytes = input.take(count)?;
            for (&index, &value) in indices.iter().zip(bytes) {
                let value = if format == 2 {
                    if value > 1 {
                        return Err(error("Invalid binary bool property"));
                    }
                    Property::Bool(value != 0)
                } else {
                    Property::Uint(value as u32)
                };
                set_property(&mut model.instances[index], name, value)?;
            }
        }
        3 | 4 | 11 | 18 | 19 => {
            let words = input.words(count)?;
            let mut last = 0;
            for (i, &index) in indices.iter().enumerate() {
                let value = match format {
                    3 => Property::Int(words.int(i) as i64),
                    4 => Property::Float(words.float(i) as f64),
                    19 => {
                        let id = words.id(i, &mut last)?;
                        if id < -1 {
                            return Err(error("Invalid property referent ID"));
                        }
                        Property::Ref((id != -1).then(|| id.to_string()))
                    }
                    _ => Property::Uint(words.uint(i)),
                };
                set_property(&mut model.instances[index], name, value)?;
            }
        }
        5 => {
            if count > input.remaining() / 8 {
                return Err(error("Truncated double property vector"));
            }
            for &index in indices {
                set_property(
                    &mut model.instances[index],
                    name,
                    Property::Float(input.f64()?),
                )?;
            }
        }
        12 | 14 => {
            let x = input.words(count)?;
            let y = input.words(count)?;
            let z = input.words(count)?;
            for (i, &index) in indices.iter().enumerate() {
                let v = [x.float(i), y.float(i), z.float(i)];
                let value = if format == 12 {
                    Property::Color3(v)
                } else {
                    Property::Vector3(v)
                };
                set_property(&mut model.instances[index], name, value)?;
            }
        }
        16 | 17 => {
            // Even axis-aligned CFrames have one rotation byte and twelve
            // translation bytes per instance; bound the allocation first.
            if count > input.remaining() / 13 {
                return Err(error("Truncated CFrame property vector"));
            }
            let mut rotations = Vec::new();
            reserve(&mut rotations, count)?;
            for _ in 0..count {
                rotations.push(rotation(input, format == 17)?);
            }
            let x = input.words(count)?;
            let y = input.words(count)?;
            let z = input.words(count)?;
            for (i, (&index, rotation)) in indices.iter().zip(rotations).enumerate() {
                let value = Property::CFrame {
                    position: [x.float(i), y.float(i), z.float(i)],
                    rotation,
                };
                set_property(&mut model.instances[index], name, value)?;
            }
        }
        20 => {
            if count > input.remaining() / 6 {
                return Err(error("Truncated Vector3int16 property vector"));
            }
            for &index in indices {
                let v = [
                    input.u16()? as i16 as f32,
                    input.u16()? as i16 as f32,
                    input.u16()? as i16 as f32,
                ];
                set_property(&mut model.instances[index], name, Property::Vector3(v))?;
            }
        }
        _ => {
            let kind = type_name(format);
            if rendering_property(name) {
                return Err(error(format!(
                    "Unsupported binary property type {kind} for {name}"
                )));
            }
            // PROP chunks are independently framed: an unrepresented type
            // can be skipped without attempting to infer its value layout.
            for &index in indices {
                set_property(
                    &mut model.instances[index],
                    name,
                    Property::Other(kind.clone()),
                )?;
            }
            input.take(input.remaining())?;
        }
    }
    input.finish()
}

pub(super) fn decode(bytes: &[u8]) -> ModelResult<Model> {
    let mut file = Reader::new(bytes);
    if file.take(8)? != b"<roblox!" {
        return Err(error("Unrecognized binary model magic"));
    }
    if file.take(6)? != b"\x89\xff\r\n\x1a\n" {
        return Err(error("Corrupted binary model signature"));
    }
    let version = file.u16()?;
    if version != 0 {
        return Err(error(format!("Unsupported binary model version {version}")));
    }
    let type_count = file.u32()?;
    let object_count = file.u32()?;
    if object_count > i32::MAX as u32 {
        return Err(error("Binary object count exceeds referent range"));
    }
    if file.take(8)? != [0; 8] {
        return Err(error("Unsupported binary model header flags"));
    }
    // Never allocate from header counts. Collections grow only after the
    // corresponding bounded INST payload has been checked.
    let mut model = Model {
        instances: Vec::new(),
    };
    let mut types: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut ids: HashMap<i32, usize> = HashMap::new();
    let mut parents: Vec<Option<usize>> = Vec::new();
    let mut parent_seen = Vec::new();
    let mut ended = false;
    while file.remaining() != 0 {
        let tag = file.take(4)?;
        let compressed = file.u32()? as usize;
        let size = file.u32()? as usize;
        if file.u32()? != 0 {
            return Err(error("Unsupported binary chunk flags or compression"));
        }
        let stored = file.take(if compressed == 0 { size } else { compressed })?;
        let data = if compressed == 0 {
            Cow::Borrowed(stored)
        } else {
            Cow::Owned(lz4(stored, size)?)
        };
        let mut chunk = Reader::new(&data);
        match tag {
            b"INST" => {
                let type_id = chunk.u32()?;
                if type_id >= type_count {
                    return Err(error("Instance type ID is out of bounds"));
                }
                if types.contains_key(&type_id) {
                    return Err(error("Duplicate instance type ID"));
                }
                let class = chunk.string()?;
                let format = chunk.byte()?;
                if format > 1 {
                    return Err(error("Unsupported binary instance format"));
                }
                let count = chunk.u32()? as usize;
                if count > object_count as usize - model.instances.len() {
                    return Err(error("Instance count exceeds binary model header"));
                }
                let words = chunk.words(count)?;
                if format == 1 {
                    if chunk.take(count)?.iter().any(|&v| v > 1) {
                        return Err(error("Invalid service-root bool"));
                    }
                }
                chunk.finish()?;
                let mut indices = Vec::new();
                reserve(&mut indices, count)?;
                reserve(&mut model.instances, count)?;
                reserve(&mut parents, count)?;
                reserve(&mut parent_seen, count)?;
                let mut last = 0;
                for i in 0..count {
                    let id = words.id(i, &mut last)?;
                    if id < 0 || id as u32 >= object_count {
                        return Err(error("Instance referent is out of bounds"));
                    }
                    let index = model.instances.len();
                    if ids.insert(id, index).is_some() {
                        return Err(error("Duplicate instance referent ID"));
                    }
                    indices.push(index);
                    model.instances.push(ModelInstance {
                        referent: id.to_string(),
                        class: class.clone(),
                        name: class.clone(),
                        parent: None,
                        properties: HashMap::new(),
                    });
                    parents.push(None);
                    parent_seen.push(false);
                }
                types.insert(type_id, indices);
            }
            b"PROP" => {
                let type_id = chunk.u32()?;
                let indices = types
                    .get(&type_id)
                    .ok_or_else(|| error("Property references an undefined instance type"))?;
                let name = chunk.string()?;
                let format = chunk.byte()?;
                properties(&mut chunk, &name, format, indices, &mut model)?;
            }
            b"PRNT" => {
                if chunk.byte()? != 0 {
                    return Err(error("Unsupported binary parent format"));
                }
                let count = chunk.u32()? as usize;
                if count > model.instances.len() {
                    return Err(error("Parent link count exceeds instance count"));
                }
                let children = chunk.words(count)?;
                let links = chunk.words(count)?;
                chunk.finish()?;
                let (mut last_child, mut last_parent) = (0, 0);
                for i in 0..count {
                    let child = children.id(i, &mut last_child)?;
                    let parent = links.id(i, &mut last_parent)?;
                    let &index = ids
                        .get(&child)
                        .ok_or_else(|| error("Parent link references an undefined child"))?;
                    if parent_seen[index] {
                        return Err(error("Duplicate binary parent link"));
                    }
                    let parent_index =
                        if parent == -1 {
                            None
                        } else {
                            Some(*ids.get(&parent).ok_or_else(|| {
                                error("Parent link references an undefined parent")
                            })?)
                        };
                    if parent_index == Some(index) {
                        return Err(error("Instance cannot parent itself"));
                    }
                    parents[index] = parent_index;
                    parent_seen[index] = true;
                    model.instances[index].parent = (parent != -1).then(|| parent.to_string());
                }
            }
            b"END\0" => {
                if chunk.take(chunk.remaining())? != b"</roblox>" {
                    return Err(error("Invalid binary model END footer"));
                }
                if file.remaining() != 0 {
                    return Err(error("Unexpected bytes after binary model END"));
                }
                ended = true;
                break;
            }
            _ => {} // The reference skips unknown, independently framed chunks.
        }
    }
    if !ended {
        return Err(error("Binary model is missing its END chunk"));
    }
    if types.len() != type_count as usize || model.instances.len() != object_count as usize {
        return Err(error(
            "Binary model header counts do not match decoded instances",
        ));
    }
    if parent_seen.iter().any(|&seen| !seen) {
        return Err(error("Binary model is missing parent links"));
    }
    validate_parents(&parents)?;
    for instance in &model.instances {
        for value in instance.properties.values() {
            if let Property::Ref(Some(id)) = value {
                let id = id
                    .parse::<i32>()
                    .map_err(|_| error("Invalid binary property referent"))?;
                if !ids.contains_key(&id) {
                    return Err(error("Property references an undefined instance"));
                }
            }
        }
    }
    Ok(model)
}

fn validate_parents(parents: &[Option<usize>]) -> ModelResult<()> {
    let mut state = Vec::new();
    reserve(&mut state, parents.len())?;
    state.resize(parents.len(), 0u8);
    for start in 0..parents.len() {
        if state[start] != 0 {
            continue;
        }
        let mut next = Some(start);
        while let Some(index) = next {
            match state[index] {
                1 => return Err(error("Cycle in binary model parent structure")),
                2 => break,
                _ => state[index] = 1,
            }
            next = parents[index];
        }
        next = Some(start);
        while let Some(index) = next {
            if state[index] != 1 {
                break;
            }
            state[index] = 2;
            next = parents[index];
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shuffled(values: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        for shift in [24, 16, 8, 0] {
            out.extend(values.iter().map(|v| (v >> shift) as u8));
        }
        out
    }
    fn ints(values: &[i32]) -> Vec<u8> {
        shuffled(
            &values
                .iter()
                .map(|&v| ((v as u32) << 1) ^ ((v >> 31) as u32))
                .collect::<Vec<_>>(),
        )
    }
    fn refs(values: &[i32]) -> Vec<u8> {
        let mut last = 0;
        ints(
            &values
                .iter()
                .map(|&v| {
                    let delta = v - last;
                    last = v;
                    delta
                })
                .collect::<Vec<_>>(),
        )
    }
    fn floats(values: &[f32]) -> Vec<u8> {
        shuffled(
            &values
                .iter()
                .map(|v| v.to_bits().rotate_left(1))
                .collect::<Vec<_>>(),
        )
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8], compressed: bool) {
        out.extend_from_slice(tag);
        let mut block = Vec::new();
        if compressed {
            block.push((payload.len().min(15) as u8) << 4);
            if payload.len() >= 15 {
                let mut extra = payload.len() - 15;
                while extra >= 255 {
                    block.push(255);
                    extra -= 255;
                }
                block.push(extra as u8);
            }
            block.extend_from_slice(payload);
        }
        out.extend_from_slice(&(block.len() as u32).to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(if compressed { &block } else { payload });
    }
    fn prop(out: &mut Vec<u8>, name: &str, format: u8, values: &[u8]) {
        let mut payload = 0u32.to_le_bytes().to_vec();
        string(&mut payload, name);
        payload.push(format);
        payload.extend_from_slice(values);
        chunk(out, b"PROP", &payload, true);
    }
    fn small_model() -> Vec<u8> {
        let mut out = b"<roblox!\x89\xff\r\n\x1a\n\0\0".to_vec();
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        let mut instances = 0u32.to_le_bytes().to_vec();
        string(&mut instances, "Part");
        instances.push(0);
        instances.extend_from_slice(&2u32.to_le_bytes());
        // IDs deliberately differ from vector order, and their delta is negative.
        instances.extend(refs(&[1, 0]));
        chunk(&mut out, b"INST", &instances, true);
        let mut names = Vec::new();
        string(&mut names, "Child");
        string(&mut names, "Root");
        prop(&mut out, "Name", 1, &names);
        prop(&mut out, "Anchored", 2, &[1, 0]);
        prop(&mut out, "Transparency", 4, &floats(&[0.25, 0.0]));
        prop(&mut out, "BrickColor", 11, &shuffled(&[194, 21]));
        prop(&mut out, "shape", 18, &shuffled(&[1, 2]));
        prop(&mut out, "Count", 3, &ints(&[-7, 42]));
        prop(&mut out, "Target", 19, &refs(&[0, -1]));
        let mut size = Vec::new();
        size.extend(floats(&[2.0, 8.0]));
        size.extend(floats(&[3.0, 1.0]));
        size.extend(floats(&[4.0, 8.0]));
        prop(&mut out, "size", 14, &size);
        let mut colors = Vec::new();
        colors.extend(floats(&[1.0, 0.25]));
        colors.extend(floats(&[0.5, 0.5]));
        colors.extend(floats(&[0.0, 0.75]));
        prop(&mut out, "Color", 12, &colors);
        let mut cframe = vec![2, 0]; // identity compact orientation, then raw matrix
        for value in [0.0f32, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0] {
            cframe.extend_from_slice(&value.to_le_bytes());
        }
        cframe.extend(floats(&[10.0, 0.0]));
        cframe.extend(floats(&[20.0, 1.0]));
        cframe.extend(floats(&[30.0, 2.0]));
        prop(&mut out, "CFrame", 16, &cframe);
        let mut parents = vec![0];
        parents.extend_from_slice(&2u32.to_le_bytes());
        parents.extend(refs(&[1, 0]));
        parents.extend(refs(&[0, -1]));
        chunk(&mut out, b"PRNT", &parents, true);
        chunk(&mut out, b"END\0", b"</roblox>", false);
        out
    }

    #[test]
    fn lz4_decodes_overlapping_matches_and_extensions() {
        let encoded = [0x1f, b'a', 1, 0, 20, 0x50, b'b', b'c', b'd', b'e', b'f'];
        let mut expected = vec![b'a'; 40];
        expected.extend_from_slice(b"bcdef");
        assert_eq!(lz4(&encoded, 45).unwrap(), expected);
        let encoded = [
            0x35, b'a', b'b', b'c', 3, 0, 0x50, b'v', b'w', b'x', b'y', b'z',
        ];
        assert_eq!(lz4(&encoded, 17).unwrap(), b"abcabcabcabcvwxyz");
        assert_eq!(lz4(&[0], 0).unwrap(), b"");
    }

    #[test]
    fn lz4_rejects_truncated_and_invalid_blocks() {
        for (bytes, size) in [
            (&[][..], 0),
            (&[0xf0][..], 15),
            (&[0x30, b'a'][..], 3),
            (&[0x10, b'a', 0][..], 14),
            (&[0x10, b'a', 0, 0][..], 14),
            (&[0x10, b'a', 2, 0][..], 14),
            (&[0x1f, b'a', 1, 0][..], 30),
            (&[0x10, b'a'][..], 2),
            (&[0][..], usize::MAX),
        ] {
            assert!(lz4(bytes, size).is_err(), "accepted {bytes:?}");
        }
    }

    #[test]
    fn shuffled_numbers_preserve_signs_and_bit_patterns() {
        let int_values = [i32::MIN, -17, 0, 42, i32::MAX];
        let encoded = ints(&int_values);
        let mut reader = Reader::new(&encoded);
        let words = reader.words(int_values.len()).unwrap();
        for (i, value) in int_values.into_iter().enumerate() {
            assert_eq!(words.int(i), value);
        }
        let float_values = [
            -0.0f32,
            -12.5,
            0.0,
            123.25,
            f32::INFINITY,
            f32::from_bits(0x7fc12345),
        ];
        let encoded = floats(&float_values);
        let mut reader = Reader::new(&encoded);
        let words = reader.words(float_values.len()).unwrap();
        for (i, value) in float_values.into_iter().enumerate() {
            assert_eq!(words.float(i).to_bits(), value.to_bits());
        }
        assert!(
            Reader::new(&encoded[..encoded.len() - 1])
                .words(float_values.len())
                .is_err()
        );
    }

    #[test]
    fn binary_model_preserves_values_order_and_parent_referents() {
        let model = decode(&small_model()).unwrap();
        let child = &model.instances[0];
        let root = &model.instances[1];
        assert_eq!(
            (&*child.referent, &*child.class, &*child.name),
            ("1", "Part", "Child")
        );
        assert_eq!(child.parent.as_deref(), Some("0"));
        assert_eq!(root.name, "Root");
        assert_eq!(root.parent, None);
        assert_eq!(child.properties["Anchored"], Property::Bool(true));
        assert_eq!(child.properties["Transparency"], Property::Float(0.25));
        assert_eq!(child.properties["BrickColor"], Property::Uint(194));
        assert_eq!(child.properties["shape"], Property::Uint(1));
        assert_eq!(child.properties["Count"], Property::Int(-7));
        assert_eq!(
            child.properties["Target"],
            Property::Ref(Some("0".to_owned()))
        );
        assert_eq!(root.properties["Target"], Property::Ref(None));
        assert_eq!(child.properties["size"], Property::Vector3([2.0, 3.0, 4.0]));
        assert_eq!(child.properties["Color"], Property::Color3([1.0, 0.5, 0.0]));
        assert_eq!(
            child.properties["CFrame"],
            Property::CFrame {
                position: [10.0, 20.0, 30.0],
                rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            }
        );
        assert_eq!(
            root.properties["CFrame"],
            Property::CFrame {
                position: [0.0, 1.0, 2.0],
                rotation: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            }
        );
    }

    #[test]
    fn quaternion_and_compact_rotations_use_row_major_columns() {
        let mut encoded = vec![0];
        for v in [
            0.0f32,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ] {
            encoded.extend_from_slice(&v.to_le_bytes());
        }
        let matrix = rotation(&mut Reader::new(&encoded), true).unwrap();
        let expected = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        for row in 0..3 {
            for col in 0..3 {
                assert!((matrix[row][col] - expected[row][col]).abs() < 1e-6);
            }
        }
        assert_eq!(
            orientation(7).unwrap(),
            [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]]
        );
        assert!(orientation(1).is_err());
        assert!(orientation(37).is_err());
    }

    #[test]
    fn truncation_and_parent_cycles_are_rejected() {
        let bytes = small_model();
        for length in 0..bytes.len() {
            assert!(
                decode(&bytes[..length]).is_err(),
                "accepted truncation at {length}"
            );
        }
        assert!(validate_parents(&[Some(1), Some(0)]).is_err());
        assert!(validate_parents(&[Some(1), None]).is_ok());
        let mut bytes = bytes;
        bytes[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&bytes).is_err());
    }

    fn graph_model(instance_ids: &[i32], children: &[i32], parents: &[i32]) -> Vec<u8> {
        let mut out = b"<roblox!\x89\xff\r\n\x1a\n\0\0".to_vec();
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(instance_ids.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        let mut instances = 0u32.to_le_bytes().to_vec();
        string(&mut instances, "Folder");
        instances.push(0);
        instances.extend_from_slice(&(instance_ids.len() as u32).to_le_bytes());
        instances.extend(refs(instance_ids));
        chunk(&mut out, b"INST", &instances, false);
        let mut links = vec![0];
        links.extend_from_slice(&(children.len() as u32).to_le_bytes());
        links.extend(refs(children));
        links.extend(refs(parents));
        chunk(&mut out, b"PRNT", &links, false);
        chunk(&mut out, b"END\0", b"</roblox>", false);
        out
    }

    #[test]
    fn malformed_serialized_graphs_are_rejected() {
        for (instances, children, parents) in [
            (&[0, 0][..], &[0, 1][..], &[-1, -1][..]), // duplicate instance ID
            (&[0, 2][..], &[0, 2][..], &[-1, -1][..]), // out-of-range ID
            (&[0, 1][..], &[0, 0][..], &[-1, -1][..]), // duplicate parent link
            (&[0, 1][..], &[0, 2][..], &[-1, -1][..]), // undefined child
            (&[0, 1][..], &[0, 1][..], &[-1, 2][..]),  // undefined parent
            (&[0, 1][..], &[0, 1][..], &[-1, 1][..]),  // self-parent
            (&[0, 1][..], &[0, 1][..], &[1, 0][..]),   // cycle
            (&[0, 1][..], &[0][..], &[-1][..]),        // missing parent link
        ] {
            assert!(decode(&graph_model(instances, children, parents)).is_err());
        }
        let valid = graph_model(&[0, 1], &[1, 0], &[0, -1]);
        let model = decode(&valid).unwrap();
        assert_eq!(model.instances[1].parent.as_deref(), Some("0"));
        let mut bad_footer = valid;
        let last = bad_footer.len() - 1;
        bad_footer[last] = b'!';
        assert!(decode(&bad_footer).is_err());
    }

    #[test]
    fn unknown_property_chunk_does_not_desynchronize_later_properties() {
        let mut bytes = graph_model(&[0, 1], &[0, 1], &[-1, 0]);
        bytes.truncate(bytes.len() - 25); // replace the END header and footer
        prop(&mut bytes, "FutureMetadata", 255, &[0xff, 0x00, 0x11]);
        prop(&mut bytes, "Count", 3, &ints(&[-123, 456]));
        chunk(&mut bytes, b"END\0", b"</roblox>", false);
        let model = decode(&bytes).unwrap();
        assert_eq!(
            model.instances[0].properties["FutureMetadata"],
            Property::Other("Unknown(255)".to_owned())
        );
        assert_eq!(model.instances[0].properties["Count"], Property::Int(-123));
        assert_eq!(model.instances[1].properties["Count"], Property::Int(456));

        let mut unsupported = graph_model(&[0, 1], &[0, 1], &[-1, 0]);
        unsupported.truncate(unsupported.len() - 25);
        prop(&mut unsupported, "CFrame", 255, &[0xff, 0x00, 0x11]);
        chunk(&mut unsupported, b"END\0", b"</roblox>", false);
        assert!(decode(&unsupported).is_err());
    }

    #[test]
    fn repository_binary_fixture_decodes_terrain_lights() {
        let bytes =
            include_bytes!("../../../../RobloxTest/Tests/Serialization/TerrainLights.bin.rbxl");
        let model = decode(bytes).unwrap();
        assert_eq!(model.instances.len(), 39);
        let camera = model
            .instances
            .iter()
            .find(|i| i.class == "Camera")
            .unwrap();
        assert_eq!(camera.properties["FieldOfView"], Property::Float(70.0));
        assert!(matches!(
            camera.properties["CoordinateFrame"],
            Property::CFrame { .. }
        ));
        let baseplate = model
            .instances
            .iter()
            .find(|i| i.name == "BasePlate")
            .unwrap();
        let workspace = model
            .instances
            .iter()
            .find(|i| i.class == "Workspace")
            .unwrap();
        assert_eq!(
            baseplate.parent.as_deref(),
            Some(workspace.referent.as_str())
        );
        assert_eq!(baseplate.properties["BrickColor"], Property::Uint(194));
        assert_eq!(baseplate.properties["shape"], Property::Uint(1));
    }
}
