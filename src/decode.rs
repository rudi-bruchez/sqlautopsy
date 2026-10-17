//! Turns the bytes of a `.txt` or `.log` into lines.

/// SQL Server writes the `.txt` as UTF-16 LE with lines padded to 113
/// characters, and cuts a longer line into 113-character pieces. A piece that
/// was cut ends on a non-space character exactly at that width.
// ponytail: fixed width, measured on 2008 R2, 2012 and 2019 dumps; read it
// from the file if another version writes a different one.
const WRAP_WIDTH: usize = 113;

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
pub fn lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut continues = false;
    for line in text.split('\n') {
        match out.last_mut() {
            Some(last) if continues => last.push_str(line),
            _ => out.push(line.to_string()),
        }
        continues = line.chars().count() == WRAP_WIDTH && !line.ends_with(' ');
    }
    out
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
        let first = "x".repeat(WRAP_WIDTH);
        let padded = format!("{:<WRAP_WIDTH$}", "version 15.0.2080.9");
        let text = format!("{first}\nrest\n{padded}\nnext");
        let lines = lines(&text);
        assert_eq!(lines[0], format!("{first}rest"));
        assert_eq!(lines[1].trim_end(), "version 15.0.2080.9");
        assert_eq!(lines[2], "next");
    }
}
