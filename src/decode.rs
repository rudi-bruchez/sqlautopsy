//! Turns the bytes of a `.txt` or `.log` into lines.

/// Decodes UTF-16 LE (with or without a BOM; the `.log` has none and may
/// start in the middle of an entry) or, failing that, UTF-8, lossily: a
/// malformed character becomes U+FFFD rather than an error.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE]) || bytes.get(1) == Some(&0);
    let text = if utf16 {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.trim_start_matches('\u{FEFF}').replace('\r', "")
}

/// Splits into lines and joins the pieces of lines cut at the wrap width.
/// Trailing padding is kept: the input buffer is cut at fixed columns too.
///
/// SQL Server pads every line of the `.txt` with spaces to a fixed width, 113
/// characters up to 2019 and 187 on 2025, and cuts a longer line into pieces
/// of that width; a piece that was cut ends on a non-space character exactly
/// at the width. The width is read from the text: the length most non-empty
/// lines share. Measured on 17 dumps, that length holds 73 to 99 % of the
/// lines of a `.txt` and at most 50 % of those of a `.log`, which is not
/// padded and is left as it is.
pub fn lines(text: &str) -> Vec<String> {
    let width = wrap_width(text);
    let mut out: Vec<String> = Vec::new();
    let mut continues = false;
    for line in text.split('\n') {
        match out.last_mut() {
            Some(last) if continues => last.push_str(line),
            _ => out.push(line.to_string()),
        }
        continues = Some(line.chars().count()) == width && !line.ends_with(' ');
    }
    out
}

/// The length shared by more than two thirds of the non-empty lines.
fn wrap_width(text: &str) -> Option<usize> {
    let mut counts: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut total = 0;
    for line in text.split('\n').filter(|l| !l.is_empty()) {
        *counts.entry(line.chars().count()).or_default() += 1;
        total += 1;
    }
    let (width, count) = counts.into_iter().max_by_key(|&(_, c)| c)?;
    (count * 3 > total * 2).then_some(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    #[test]
    fn decodes_utf16_with_and_without_bom() {
        assert_eq!(decode(&utf16("a\r\nb")), "a\nb");
        assert_eq!(decode(&utf16("é")[2..]), "é");
        assert_eq!(decode(b"plain"), "plain");
    }

    #[test]
    fn odd_length_and_bad_surrogates_do_not_fail() {
        let mut bytes = utf16("ab");
        bytes.extend([0x00, 0xD8, 0x41]);
        assert_eq!(decode(&bytes), "ab\u{FFFD}");
    }

    #[test]
    fn joins_lines_cut_at_the_wrap_width() {
        for width in [113, 187] {
            let first = "x".repeat(width);
            let pad = |s: &str| format!("{s:<width$}");
            let text = [
                first.clone(),
                pad("rest"),
                pad("version 15.0.2080.9"),
                pad("next"),
            ]
            .join("\n");
            let lines = lines(&text);
            assert_eq!(lines[0].trim_end(), format!("{first}rest"), "{width}");
            assert_eq!(lines[1].trim_end(), "version 15.0.2080.9");
            assert_eq!(lines[2].trim_end(), "next");
        }
    }

    #[test]
    fn unpadded_text_is_not_joined() {
        let log = "2022-02-18 13:58:00.10 spid51      Error: 823.\nshort\nx";
        assert_eq!(lines(log).len(), 3);
    }
}
