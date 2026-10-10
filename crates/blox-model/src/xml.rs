use super::{Model, ModelError, ModelInstance, ModelResult, Property};
use glam::{Mat3, Vec3};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

// The arena keeps scalar text borrowed from the input and avoids recursive parsing or
// destruction even for deeply nested Items. Only entity expansion/mixed text allocates.
struct Element<'a> {
    tag: &'a str,
    attributes: Vec<(&'a str, Cow<'a, str>)>,
    text: Cow<'a, str>,
    children: Vec<usize>,
}

impl Element<'_> {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_ref())
    }

    fn scalar(&self) -> ModelResult<&str> {
        if !self.children.is_empty() {
            return Err(ModelError(format!("{} must contain scalar text", self.tag)));
        }
        Ok(self.text.as_ref())
    }

    fn structural(&self) -> ModelResult<()> {
        if !self.text.trim().is_empty() {
            return Err(ModelError(format!("unexpected text inside {}", self.tag)));
        }
        Ok(())
    }
}

fn xml_character(c: char) -> bool {
    matches!(c as u32, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}

fn whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

fn name_start(c: char) -> bool {
    matches!(c as u32, 0x3a | 0x41..=0x5a | 0x5f | 0x61..=0x7a | 0xc0..=0xd6 |
        0xd8..=0xf6 | 0xf8..=0x2ff | 0x370..=0x37d | 0x37f..=0x1fff |
        0x200c..=0x200d | 0x2070..=0x218f | 0x2c00..=0x2fef | 0x3001..=0xd7ff |
        0xf900..=0xfdcf | 0xfdf0..=0xfffd | 0x10000..=0xeffff)
}

fn name_character(c: char) -> bool {
    name_start(c)
        || matches!(c as u32, 0x2d | 0x2e | 0x30..=0x39 | 0xb7 | 0x300..=0x36f | 0x203f..=0x2040)
}

fn xml_text(input: &str, attribute: bool, entities: bool) -> ModelResult<Cow<'_, str>> {
    if (!entities || !input.contains('&'))
        && !input.contains('\r')
        && (!attribute || !input.contains(['\t', '\n']))
    {
        return Ok(Cow::Borrowed(input));
    }
    let mut output = String::with_capacity(input.len());
    let mut offset = 0;
    while offset < input.len() {
        let c = input[offset..].chars().next().unwrap();
        offset += c.len_utf8();
        if entities && c == '&' {
            let end = input[offset..]
                .find(';')
                .ok_or_else(|| ModelError("unterminated XML entity".into()))?;
            let entity = &input[offset..offset + end];
            let decoded = match entity {
                "lt" => '<',
                "gt" => '>',
                "amp" => '&',
                "quot" => '"',
                "apos" => '\'',
                // XmlSerializer.cpp also accepts this legacy non-XML entity as a space.
                "nbsp" => ' ',
                _ => {
                    let number = if let Some(hex) = entity.strip_prefix("#x") {
                        if hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            None
                        }
                    } else if let Some(decimal) = entity.strip_prefix('#') {
                        if decimal.bytes().all(|b| b.is_ascii_digit()) {
                            decimal.parse().ok()
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    number
                        .and_then(char::from_u32)
                        // Legacy XmlSerializer.cpp writes numeric entities for control
                        // bytes in strings (e.g. Script.Source keyboard shortcuts).
                        // Accept nonzero C0 controls only when entity-escaped; raw
                        // controls, NUL, surrogates and invalid Unicode remain errors.
                        .filter(|c| xml_character(*c) || matches!(*c as u32, 1..=0x1f))
                        .ok_or_else(|| ModelError(format!("invalid XML entity &{entity};")))?
                }
            };
            offset += end + 1;
            output.push(decoded);
        } else if c == '\r' {
            if input[offset..].starts_with('\n') {
                offset += 1;
            }
            output.push(if attribute { ' ' } else { '\n' });
        } else if attribute && matches!(c, '\n' | '\t') {
            output.push(' ');
        } else {
            output.push(c);
        }
    }
    Ok(Cow::Owned(output))
}

struct Parser<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> Parser<'a> {
    fn error(&self, message: &str) -> ModelError {
        ModelError(format!("XML byte {}: {message}", self.offset))
    }

    fn rest(&self) -> &'a str {
        &self.input[self.offset..]
    }

    fn skip_whitespace(&mut self) {
        while self
            .input
            .as_bytes()
            .get(self.offset)
            .is_some_and(|c| whitespace(*c))
        {
            self.offset += 1;
        }
    }

    fn name(&mut self) -> ModelResult<&'a str> {
        let start = self.offset;
        let mut chars = self.rest().chars();
        let first = chars.next().ok_or_else(|| self.error("missing XML name"))?;
        if !name_start(first) {
            return Err(self.error("invalid XML name"));
        }
        self.offset += first.len_utf8();
        for c in chars {
            if !name_character(c) {
                break;
            }
            self.offset += c.len_utf8();
        }
        Ok(&self.input[start..self.offset])
    }

    fn attributes(
        &mut self,
        declaration: bool,
    ) -> ModelResult<(Vec<(&'a str, Cow<'a, str>)>, bool)> {
        let mut attributes = Vec::new();
        loop {
            let before = self.offset;
            self.skip_whitespace();
            if declaration && self.rest().starts_with("?>") {
                self.offset += 2;
                return Ok((attributes, false));
            }
            if !declaration && self.rest().starts_with("/>") {
                self.offset += 2;
                return Ok((attributes, true));
            }
            if !declaration && self.rest().starts_with('>') {
                self.offset += 1;
                return Ok((attributes, false));
            }
            if self.offset == before {
                return Err(self.error("expected whitespace or tag end"));
            }
            let name = self.name()?;
            if attributes.iter().any(|(key, _)| *key == name) {
                return Err(self.error("duplicate XML attribute"));
            }
            self.skip_whitespace();
            if !self.rest().starts_with('=') {
                return Err(self.error("expected attribute '='"));
            }
            self.offset += 1;
            self.skip_whitespace();
            let quote = *self
                .input
                .as_bytes()
                .get(self.offset)
                .ok_or_else(|| self.error("missing attribute value"))?;
            if !matches!(quote, b'\'' | b'"') {
                return Err(self.error("attribute value must be quoted"));
            }
            self.offset += 1;
            let length = self
                .rest()
                .find(quote as char)
                .ok_or_else(|| self.error("unterminated attribute"))?;
            let value = &self.input[self.offset..self.offset + length];
            if value.contains('<') {
                return Err(self.error("'<' inside attribute"));
            }
            attributes.push((name, xml_text(value, true, true)?));
            self.offset += length + 1;
        }
    }

    fn document(mut self) -> ModelResult<Vec<Element<'a>>> {
        let mut arena: Vec<Element<'a>> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut declaration_seen = false;
        while self.offset < self.input.len() {
            if self.rest().starts_with("<!--") {
                self.offset += 4;
                let end = self
                    .rest()
                    .find("-->")
                    .ok_or_else(|| self.error("unterminated comment"))?;
                let body = &self.rest()[..end];
                if body.contains("--") || body.ends_with('-') {
                    return Err(self.error("invalid XML comment"));
                }
                self.offset += end + 3;
            } else if self.rest().starts_with("<![CDATA[") {
                let parent = *stack
                    .last()
                    .ok_or_else(|| self.error("CDATA outside document root"))?;
                self.offset += 9;
                let end = self
                    .rest()
                    .find("]]>")
                    .ok_or_else(|| self.error("unterminated CDATA"))?;
                let text = &self.rest()[..end];
                // CDATA is literal (no entity expansion), with XML newline normalization.
                let text = xml_text(text, false, false)?;
                append_text(&mut arena[parent].text, text);
                self.offset += end + 3;
            } else if self.rest().starts_with("<?") {
                let start = self.offset;
                self.offset += 2;
                let target = self.name()?;
                if target.eq_ignore_ascii_case("xml") {
                    if target != "xml" || declaration_seen || start != 0 || !arena.is_empty() {
                        return Err(self.error("misplaced XML declaration"));
                    }
                    declaration_seen = true;
                    let (attributes, _) = self.attributes(true)?;
                    if attributes
                        .first()
                        .map(|(name, value)| (*name, value.as_ref()))
                        != Some(("version", "1.0"))
                    {
                        return Err(self.error("expected XML version 1.0"));
                    }
                    for (name, value) in &attributes[1..] {
                        match *name {
                            "encoding" if value.eq_ignore_ascii_case("utf-8") => {}
                            "standalone" if matches!(value.as_ref(), "yes" | "no") => {}
                            _ => return Err(self.error("unsupported XML declaration attribute")),
                        }
                    }
                } else {
                    if !self.rest().starts_with("?>")
                        && !self
                            .input
                            .as_bytes()
                            .get(self.offset)
                            .is_some_and(|c| whitespace(*c))
                    {
                        return Err(self.error("invalid processing instruction"));
                    }
                    let end = self
                        .rest()
                        .find("?>")
                        .ok_or_else(|| self.error("unterminated processing instruction"))?;
                    self.offset += end + 2;
                }
            } else if self.rest().starts_with("</") {
                self.offset += 2;
                let name = self.name()?;
                self.skip_whitespace();
                if !self.rest().starts_with('>') {
                    return Err(self.error("invalid close tag"));
                }
                self.offset += 1;
                let parent = stack
                    .pop()
                    .ok_or_else(|| self.error("close tag without open tag"))?;
                if arena[parent].tag != name {
                    return Err(self.error("mismatched close tag"));
                }
            } else if self.rest().starts_with("<!") {
                // DTDs are not used by Roblox's writer; never resolve external resources.
                return Err(self.error("DTD/markup declarations are unsupported"));
            } else if self.rest().starts_with('<') {
                self.offset += 1;
                let tag = self.name()?;
                let (attributes, empty) = self.attributes(false)?;
                let index = arena.len();
                if let Some(&parent) = stack.last() {
                    arena[parent].children.push(index);
                } else if !arena.is_empty() {
                    return Err(self.error("multiple document roots"));
                }
                arena.push(Element {
                    tag,
                    attributes,
                    text: Cow::Borrowed(""),
                    children: Vec::new(),
                });
                if !empty {
                    stack.push(index);
                }
            } else {
                let end = self.rest().find('<').unwrap_or(self.rest().len());
                let raw = &self.rest()[..end];
                if raw.contains("]]>") {
                    return Err(self.error("CDATA terminator outside CDATA"));
                }
                let text = xml_text(raw, false, true)?;
                if let Some(&parent) = stack.last() {
                    append_text(&mut arena[parent].text, text);
                } else if !raw.bytes().all(whitespace) {
                    return Err(self.error("text outside document root"));
                }
                self.offset += end;
            }
        }
        if !stack.is_empty() {
            return Err(self.error("truncated XML document"));
        }
        if arena.is_empty() {
            return Err(self.error("missing document root"));
        }
        Ok(arena)
    }
}

fn append_text<'a>(target: &mut Cow<'a, str>, text: Cow<'a, str>) {
    if text.is_empty() {
        return;
    }
    if target.is_empty() {
        *target = text;
    } else {
        target.to_mut().push_str(&text);
    }
}

fn number<T: std::str::FromStr>(text: &str, tag: &str) -> ModelResult<T> {
    text.trim()
        .parse()
        .map_err(|_| ModelError(format!("invalid {tag} number: {text:?}")))
}

fn component(arena: &[Element<'_>], element: &Element<'_>, tag: &str) -> ModelResult<f32> {
    let mut matching = element
        .children
        .iter()
        .map(|i| &arena[*i])
        .filter(|child| child.tag == tag);
    let child = matching
        .next()
        .ok_or_else(|| ModelError(format!("{} is missing {tag}", element.tag)))?;
    if matching.next().is_some() {
        return Err(ModelError(format!(
            "duplicate {} component {tag}",
            element.tag
        )));
    }
    number(child.scalar()?, tag)
}

fn packed_color(value: u32) -> Property {
    Property::Color3(Vec3::new(
        ((value >> 16) & 255) as f32 / 255.0,
        ((value >> 8) & 255) as f32 / 255.0,
        (value & 255) as f32 / 255.0,
    ))
}

fn property(arena: &[Element<'_>], element: &Element<'_>) -> ModelResult<Property> {
    if let Some(nil) = element.attribute("xsi:nil") {
        match nil {
            "true" | "1" => {
                return Ok(if element.tag == "Content" {
                    Property::Text(String::new())
                } else {
                    Property::Other(element.tag.into())
                });
            }
            "false" | "0" => {}
            _ => return Err(ModelError("invalid xsi:nil boolean".into())),
        }
    }
    Ok(match element.tag {
        "string" | "ProtectedString" => Property::Text(element.scalar()?.into()),
        "bool" => Property::Bool(match element.scalar()?.trim() {
            "true" | "1" => true,
            "false" | "0" => false,
            value => return Err(ModelError(format!("invalid bool value {value:?}"))),
        }),
        "int" | "int64" | "token" => Property::Int(number(element.scalar()?, element.tag)?),
        "float" | "double" => Property::Float(number(element.scalar()?, element.tag)?),
        "Vector3" => {
            element.structural()?;
            Property::Vector3(Vec3::new(
                component(arena, element, "X")?,
                component(arena, element, "Y")?,
                component(arena, element, "Z")?,
            ))
        }
        "CoordinateFrame" => {
            element.structural()?;
            // Stored row-major: R00 R01 R02 / R10 R11 R12 / R20 R21 R22.
            let row = |prefix: &str| -> ModelResult<Vec3> {
                Ok(Vec3::new(
                    component(arena, element, &format!("{prefix}0"))?,
                    component(arena, element, &format!("{prefix}1"))?,
                    component(arena, element, &format!("{prefix}2"))?,
                ))
            };
            let (r0, r1, r2) = (row("R0")?, row("R1")?, row("R2")?);
            Property::CFrame {
                position: Vec3::new(
                    component(arena, element, "X")?,
                    component(arena, element, "Y")?,
                    component(arena, element, "Z")?,
                ),
                rotation: Mat3::from_cols(
                    Vec3::new(r0.x, r1.x, r2.x),
                    Vec3::new(r0.y, r1.y, r2.y),
                    Vec3::new(r0.z, r1.z, r2.z),
                ),
            }
        }
        "Color3" if !element.children.is_empty() => {
            element.structural()?;
            Property::Color3(Vec3::new(
                component(arena, element, "R")?,
                component(arena, element, "G")?,
                component(arena, element, "B")?,
            ))
        }
        "Color3" | "Color3uint8" => packed_color(number(element.scalar()?, element.tag)?),
        "Ref" => Property::Ref(match element.scalar()?.trim() {
            "" | "null" | "nil" => None,
            value => Some(value.into()),
        }),
        "Content" => {
            element.structural()?;
            if element.children.len() != 1 {
                return Err(ModelError("Content requires one url/null child".into()));
            }
            let child = &arena[element.children[0]];
            match child.tag {
                "url" => Property::Text(child.scalar()?.into()),
                "null" if child.scalar()?.trim().is_empty() => Property::Text(String::new()),
                "binary" | "hash" => Property::Other(format!("Content/{}", child.tag)),
                _ => {
                    return Err(ModelError(format!(
                        "unsupported Content child {}",
                        child.tag
                    )));
                }
            }
        }
        // `BinaryString` is base64 in XML (`EnumProperty.cpp::readValue`) so it can round-trip
        // through a text format.
        "BinaryString" => Property::Binary(decode_base64(element.scalar()?)?),
        tag => Property::Other(tag.into()),
    })
}

/// Standard-alphabet base64, ignoring whitespace, as `util/base64.hpp` accepts.
fn decode_base64(text: &str) -> ModelResult<Vec<u8>> {
    const VALUE: [u8; 256] = {
        let mut table = [255u8; 256];
        let mut index = 0;
        while index < 26 {
            table[b'A' as usize + index] = index as u8;
            table[b'a' as usize + index] = index as u8 + 26;
            index += 1;
        }
        let mut digit = 0;
        while digit < 10 {
            table[b'0' as usize + digit] = digit as u8 + 52;
            digit += 1;
        }
        table[b'+' as usize] = 62;
        table[b'/' as usize] = 63;
        table
    };
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    let mut padding = 0;
    for byte in text.bytes() {
        if byte == b'=' {
            padding += 1;
            continue;
        }
        let value = VALUE[byte as usize];
        if value == 255 {
            if byte.is_ascii_whitespace() {
                continue;
            }
            return Err(ModelError("invalid base64 in BinaryString".into()));
        }
        if padding != 0 {
            return Err(ModelError("base64 data after padding".into()));
        }
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    Ok(out)
}

pub(super) fn decode(bytes: &[u8]) -> ModelResult<Model> {
    let input =
        std::str::from_utf8(bytes).map_err(|e| ModelError(format!("XML is not UTF-8: {e}")))?;
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    if !input.chars().all(xml_character) {
        return Err(ModelError("invalid XML character".into()));
    }
    let arena = Parser { input, offset: 0 }.document()?;
    let root = &arena[0];
    if root.tag != "roblox" {
        return Err(ModelError("expected roblox XML root".into()));
    }
    root.structural()?;
    let mut instances: Vec<ModelInstance> = Vec::new();
    let mut referents = HashSet::new();
    let mut pending: Vec<(usize, Option<usize>)> = root
        .children
        .iter()
        .rev()
        .filter(|index| arena[**index].tag == "Item")
        .map(|index| (*index, None))
        .collect();
    while let Some((index, parent)) = pending.pop() {
        let item = &arena[index];
        item.structural()?;
        let class = item
            .attribute("class")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ModelError("Item is missing class".into()))?;
        let referent = item
            .attribute("referent")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ModelError("Item is missing referent".into()))?;
        if !referents.insert(referent) {
            return Err(ModelError(format!("duplicate Item referent {referent}")));
        }
        let mut instance = ModelInstance {
            referent: referent.into(),
            class: class.into(),
            name: class.into(),
            parent: parent.map(|p| instances[p].referent.clone()),
            properties: HashMap::new(),
        };
        let mut properties_seen = false;
        for &child_index in &item.children {
            let child = &arena[child_index];
            if child.tag != "Properties" {
                continue;
            }
            if properties_seen {
                return Err(ModelError(format!("duplicate Properties for {referent}")));
            }
            properties_seen = true;
            child.structural()?;
            for &property_index in &child.children {
                let element = &arena[property_index];
                let name = element
                    .attribute("name")
                    .ok_or_else(|| ModelError(format!("{} property missing name", element.tag)))?;
                let value = property(&arena, element)?;
                if name == "Name" {
                    if let Property::Text(text) = &value {
                        instance.name.clone_from(text);
                    }
                }
                if instance.properties.insert(name.into(), value).is_some() {
                    return Err(ModelError(format!(
                        "duplicate property {name} for {referent}"
                    )));
                }
            }
        }
        let parent = instances.len();
        instances.push(instance);
        for &child in item.children.iter().rev() {
            if arena[child].tag == "Item" {
                pending.push((child, Some(parent)));
            }
        }
    }
    Ok(Model { instances })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_nesting_truncation_and_entities() {
        for invalid in [
            "<roblox><Item></roblox>",
            "<roblox>",
            "<roblox/ ><roblox/>",
            "<roblox></roblox><roblox/>",
            "<roblox><!-- unfinished</roblox>",
            "<roblox><!-- a--b --></roblox>",
            "<roblox><![CDATA[unfinished</roblox>",
            "<roblox>&unknown;</roblox>",
            "<roblox>&amp</roblox>",
            "<roblox>&#0;</roblox>",
            "<roblox>&#x110000;</roblox>",
            "<roblox>&#xD800;</roblox>",
            "<roblox a='1' a='2'/>",
            "<roblox a='<'/>",
            "<roblox a=bare/>",
            "<roblox><Item class='Part' referent='a'/><Item class='Part' referent='a'/></roblox>",
        ] {
            assert!(decode(invalid.as_bytes()).is_err(), "accepted {invalid}");
        }
        let model = decode(br#"<?xml version="1.0" encoding="UTF-8"?><!--prefix--><roblox>
            <Item class="Script" referent="R&amp;1"><Properties>
            <string name="Name"> &lt;&gt;&amp;&quot;&apos;&#65;&#x1f600;&nbsp; </string>
            <ProtectedString name="Source"><![CDATA[if a < b then print('&amp;') end]]><!--split-->\n</ProtectedString>
            </Properties></Item></roblox><!--suffix-->"#).unwrap();
        let script = &model.instances[0];
        assert_eq!(script.referent, "R&1");
        assert_eq!(script.name, " <>&\"'A😀  ");
        assert_eq!(
            script.properties["Source"],
            Property::Text("if a < b then print('&amp;') end\\n".into())
        );
    }

    #[test]
    fn decodes_legacy_place_control_entities() {
        let model = Model::decode(br#"<roblox version="4">
            <External>null</External><External>nil</External>
            <Item class="Workspace" referent="w"><Properties>
                <string name="Name">Workspace</string>
            </Properties><Item class="Script" referent="s"><Properties>
                <ProtectedString name="Source">if key == &quot;&#17;&quot; or key == &quot;&#x12;&quot; then&#13;&#10;&#9;end</ProtectedString>
            </Properties></Item></Item></roblox>"#).unwrap();
        assert_eq!(model.instances.len(), 2);
        assert_eq!(model.instances[1].parent.as_deref(), Some("w"));
        assert_eq!(
            model.instances[1].properties["Source"],
            Property::Text("if key == \"\u{11}\" or key == \"\u{12}\" then\r\n\tend".into())
        );
        for value in ["\u{11}", "&#0;", "&#x0;", "&#xD800;", "&#x110000;"] {
            let xml = format!(
                "<roblox><Item class='Script' referent='s'><Properties><ProtectedString name='Source'>{value}</ProtectedString></Properties></Item></roblox>"
            );
            assert!(Model::decode(xml.as_bytes()).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn decodes_hierarchy_properties_and_row_major_frame() {
        let model = decode(br#"<roblox><External>null</External><Item class="Model" referent="m">
            <Properties><string name="Name">Scene</string><Ref name="PrimaryPart">p</Ref></Properties>
            <Item class="Part" referent="p"><Properties>
                <string name="Name">Block</string><int name="BrickColor">24</int>
                <Vector3 name="size"><X>2</X><Y>3</Y><Z>4</Z></Vector3>
                <CoordinateFrame name="CFrame"><X>5</X><Y>6</Y><Z>7</Z>
                <R00>0</R00><R01>-1</R01><R02>0</R02><R10>1</R10><R11>0</R11><R12>0</R12>
                <R20>0</R20><R21>0</R21><R22>1</R22></CoordinateFrame>
                <Color3 name="Color">4279312947</Color3><Color3uint8 name="Color3uint8">1122867</Color3uint8>
                <Color3 name="VertexColor"><R>0.25</R><G>0.5</G><B>1</B></Color3>
                <token name="shape">2</token><bool name="Anchored">true</bool><float name="Transparency">0.5</float>
                <double name="Reflectance">0.25</double><Ref name="Missing">nil</Ref>
                <PhysicalProperties name="CustomPhysicalProperties"><CustomPhysics>false</CustomPhysics></PhysicalProperties>
            </Properties><Item class="SpecialMesh" referent="s"><Properties>
                <Content name="MeshId"><url>rbxasset://mesh&amp;name</url></Content>
                <Content name="TextureId"><null/></Content>
            </Properties></Item></Item></Item></roblox>"#).unwrap();
        assert_eq!(
            model.instances[0].properties["PrimaryPart"],
            Property::Ref(Some("p".into()))
        );
        let part = &model.instances[1];
        assert_eq!(
            (
                part.class.as_str(),
                part.name.as_str(),
                part.parent.as_deref()
            ),
            ("Part", "Block", Some("m"))
        );
        assert_eq!(
            part.properties["size"],
            Property::Vector3(Vec3::new(2.0, 3.0, 4.0))
        );
        assert!(!part.properties.contains_key("Size"));
        assert_eq!(
            part.properties["CFrame"],
            Property::CFrame {
                position: Vec3::new(5.0, 6.0, 7.0),
                rotation: Mat3::from_cols(
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(-1.0, 0.0, 0.0),
                    Vec3::new(0.0, 0.0, 1.0),
                ),
            }
        );
        assert_eq!(part.properties["BrickColor"], Property::Int(24));
        assert_eq!(part.properties["shape"], Property::Int(2));
        assert_eq!(part.properties["Anchored"], Property::Bool(true));
        assert_eq!(part.properties["Transparency"], Property::Float(0.5));
        assert_eq!(part.properties["Reflectance"], Property::Float(0.25));
        assert_eq!(
            part.properties["Color"],
            Property::Color3(Vec3::new(17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0))
        );
        assert_eq!(part.properties["Color3uint8"], part.properties["Color"]);
        assert_eq!(
            part.properties["VertexColor"],
            Property::Color3(Vec3::new(0.25, 0.5, 1.0))
        );
        assert_eq!(part.properties["Missing"], Property::Ref(None));
        assert_eq!(
            part.properties["CustomPhysicalProperties"],
            Property::Other("PhysicalProperties".into())
        );
        let mesh = &model.instances[2];
        assert_eq!(mesh.parent.as_deref(), Some("p"));
        assert_eq!(
            mesh.properties["MeshId"],
            Property::Text("rbxasset://mesh&name".into())
        );
        assert_eq!(mesh.properties["TextureId"], Property::Text(String::new()));
    }

    #[test]
    fn rejects_incomplete_typed_values() {
        for property in [
            "<Vector3 name='size'><X>1</X><Y>2</Y></Vector3>",
            "<Vector3 name='size'><X>1</X><X>2</X><Y>2</Y><Z>3</Z></Vector3>",
            "<CoordinateFrame name='CFrame'><X>0</X><Y>0</Y><Z>0</Z></CoordinateFrame>",
            "<float name='Transparency'>no</float>",
            "<bool name='Anchored'>perhaps</bool>",
            "<Color3uint8 name='Color'>4294967296</Color3uint8>",
            "<Content name='MeshId'><url>a</url><null/></Content>",
        ] {
            let xml = format!(
                "<roblox><Item class='Part' referent='p'><Properties>{property}</Properties></Item></roblox>"
            );
            assert!(decode(xml.as_bytes()).is_err(), "accepted {property}");
        }
    }

    #[test]
    fn decodes_repository_character_fixture() {
        let model = decode(include_bytes!("../../../../content/other/character.rbxm")).unwrap();
        let get = |id: &str| {
            model
                .instances
                .iter()
                .find(|instance| instance.referent == id)
                .unwrap()
        };
        let character = get("RBX0");
        assert_eq!(character.class, "Model");
        assert_eq!(character.name, "erik.cassel");
        assert_eq!(character.parent, None);
        assert_eq!(
            character.properties["PrimaryPart"],
            Property::Ref(Some("RBX1".into()))
        );
        let head = get("RBX1");
        assert_eq!(head.name, "Head");
        assert_eq!(head.parent.as_deref(), Some("RBX0"));
        assert_eq!(head.properties["BrickColor"], Property::Int(24));
        assert_eq!(
            head.properties["size"],
            Property::Vector3(Vec3::new(2.0, 1.0, 1.0))
        );
        assert_eq!(
            head.properties["CFrame"],
            Property::CFrame {
                position: Vec3::new(0.0, 4.5, 25.5),
                rotation: Mat3::from_cols(
                    Vec3::new(-1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(0.0, 0.0, -1.0),
                ),
            }
        );
        let mesh = get("RBX2");
        assert_eq!(mesh.class, "SpecialMesh");
        assert_eq!(mesh.parent.as_deref(), Some("RBX1"));
        assert_eq!(
            mesh.properties["Scale"],
            Property::Vector3(Vec3::new(1.25, 1.25, 1.25))
        );
        assert_eq!(mesh.properties["MeshId"], Property::Text(String::new()));
        assert_eq!(
            get("RBX3").properties["Texture"],
            Property::Text("rbxasset://textures/face.png".into())
        );
        assert_eq!(get("RBX9").name, "Right Leg");
        assert_eq!(get("RBX10").properties["WalkToPart"], Property::Ref(None));
    }
}
