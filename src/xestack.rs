//! Names the call stacks that Extended Events capture with
//! `package0.callstack_rva` (SQL Server 2022 and later).
//!
//! Each frame is written as
//! `<frame id="00" address="0x..." pdb="sqllang.pdb" age="2" guid="A1EE9134-..." module="sqllang" rva="0x1876742" />`,
//! inside the histogram target (one `<Slot count="N">` per distinct stack),
//! the ring buffer or an `event_file` (one stack per event), wrapped in CDATA
//! or escaped as `&lt;frame`. All of them are read the same way: a stack
//! starts at frame `00`, and a histogram slot gives its count.

use std::collections::HashMap;

use serde::Serialize;

use crate::symbols::Symbols;

#[derive(Debug, Serialize)]
pub struct Stack {
    /// How many times this stack was seen: the histogram slot's count, or the
    /// number of events that carried it.
    pub count: u64,
    pub frames: Vec<Frame>,
}

#[derive(Debug, Serialize, PartialEq, Eq, Hash, Clone)]
pub struct Frame {
    pub module: String,
    /// Offset from the module's base: `0x1876742`.
    pub rva: String,
    pub pdb: Option<String>,
    /// The symbol server key: GUID without dashes, then the age in hex.
    pub debug_id: Option<String>,
    pub function: Option<String>,
}

/// The stacks in `text`, identical ones merged, most frequent first.
pub fn parse(text: &str) -> Vec<Stack> {
    let text = text
        .replace("<![CDATA[", "")
        .replace("]]>", "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");
    let mut stacks: Vec<(u64, Vec<Frame>)> = Vec::new();
    let mut slot_count: Option<u64> = None;
    let mut rest = text.as_str();
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        let end = rest.find('>').map_or(rest.len(), |e| e + 1);
        let tag = &rest[..end];
        rest = &rest[end..];
        if tag.starts_with("<Slot ") {
            slot_count = attribute(tag, "count").and_then(|c| c.parse().ok());
        } else if tag.starts_with("<frame ") {
            let Some(frame) = parse_frame(tag) else {
                continue;
            };
            if attribute(tag, "id").is_none_or(|id| id.trim_start_matches('0').is_empty())
                || stacks.is_empty()
            {
                stacks.push((slot_count.take().unwrap_or(1), Vec::new()));
            }
            if let Some((_, frames)) = stacks.last_mut() {
                frames.push(frame);
            }
        }
    }
    let mut merged: HashMap<Vec<Frame>, u64> = HashMap::new();
    for (count, frames) in stacks {
        *merged.entry(frames).or_default() += count;
    }
    let mut stacks: Vec<Stack> = merged
        .into_iter()
        .map(|(frames, count)| Stack { count, frames })
        .collect();
    stacks.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.frames.len().cmp(&b.frames.len()))
    });
    stacks
}

/// One `<frame ... />` tag; `None` without its module or offset.
pub fn parse_frame(tag: &str) -> Option<Frame> {
    let module = attribute(tag, "module")?.to_string();
    let rva = attribute(tag, "rva")?.to_string();
    let pdb = attribute(tag, "pdb").map(str::to_string);
    // SQLCallStackResolver reads the age as a decimal integer and dbghelp
    // writes it in hex in the key.
    let debug_id = attribute(tag, "guid")
        .zip(attribute(tag, "age"))
        .and_then(|(guid, age)| {
            let guid: String = guid.chars().filter(|c| *c != '-').collect();
            let age: u32 = age.parse().ok()?;
            (guid.len() == 32 && guid.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| format!("{}{age:X}", guid.to_ascii_uppercase()))
        });
    Some(Frame {
        module,
        rva,
        pdb,
        debug_id,
        function: None,
    })
}

/// The value of `name="..."` in a tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

pub fn symbolize(stacks: &mut [Stack], store: &mut Symbols) {
    for frame in stacks.iter_mut().flat_map(|s| s.frames.iter_mut()) {
        name(frame, store);
    }
}

/// Names a frame from the PDB it carries.
pub fn name(frame: &mut Frame, store: &mut Symbols) {
    let rva = frame.rva.trim_start_matches("0x").trim_start_matches("0X");
    if let (Some(pdb), Some(id), Ok(rva)) =
        (&frame.pdb, &frame.debug_id, u32::from_str_radix(rva, 16))
    {
        frame.function = store.resolve(pdb, id, rva);
    }
}

pub fn render(stacks: &[Stack]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let total: u64 = stacks.iter().map(|s| s.count).sum();
    for (i, stack) in stacks.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "stack {}  seen {} of {total}", i + 1, stack.count);
        for (n, f) in stack.frames.iter().enumerate() {
            let place = format!("{}+{}", f.module, f.rva.trim_start_matches("0x"));
            let _ = match &f.function {
                Some(function) => writeln!(out, "  {n:02} {place:<22} {function}"),
                None => writeln!(out, "  {n:02} {place}"),
            };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape of a SQL Server 2025 histogram target, frames shortened.
    const HISTOGRAM: &str = r#"<HistogramTarget truncated="0" buckets="256"><Slot count="17"><value><![CDATA[<frame id="00" address="0xB87426742" pdb="sqllang.pdb" age="2" guid="A1EE9134-BEFA-47A3-B2A6-1129DC226721" module="sqllang" rva="0x1876742" /><frame id="01" address="0x7FF1" pdb="SqlDK.pdb" age="14" guid="5045C339-29F8-465D-8CCE-C2148EC99833" module="SqlDK" rva="0x9D35" />]]></value></Slot><Slot count="3"><value><![CDATA[<frame id="00" address="0x1" module="mystery" rva="0x10" />]]></value></Slot></HistogramTarget>"#;

    #[test]
    fn reads_a_histogram() {
        let stacks = parse(HISTOGRAM);
        assert_eq!(stacks.len(), 2);
        assert_eq!(stacks[0].count, 17);
        let f = &stacks[0].frames;
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].module, "sqllang");
        assert_eq!(f[0].rva, "0x1876742");
        assert_eq!(
            f[0].debug_id.as_deref(),
            Some("A1EE9134BEFA47A3B2A61129DC2267212")
        );
        // An age of 14 is 0xE in the key.
        assert_eq!(
            f[1].debug_id.as_deref(),
            Some("5045C33929F8465D8CCEC2148EC99833E")
        );
        assert_eq!(stacks[1].count, 3);
        assert_eq!(stacks[1].frames[0].debug_id, None);
    }

    #[test]
    fn merges_the_stacks_of_escaped_events() {
        let event = r#"<event><action name="callstack_rva"><value>&lt;frame id=&quot;00&quot; pdb=&quot;a.pdb&quot; module=&quot;a&quot; rva=&quot;0x1&quot; /&gt;&lt;frame id=&quot;01&quot; module=&quot;a&quot; rva=&quot;0x2&quot; /&gt;</value></action></event>"#;
        let other = r#"<frame id="00" module="b" rva="0x5" />"#;
        let stacks = parse(&format!("{event}{event}{other}"));
        assert_eq!(stacks.len(), 2);
        assert_eq!((stacks[0].count, stacks[0].frames.len()), (2, 2));
        assert_eq!(stacks[1].frames[0].module, "b");
    }

    #[test]
    fn ignores_what_is_not_a_frame() {
        assert!(parse("<frame id=\"00\" /> <a> < text").is_empty());
    }
}
