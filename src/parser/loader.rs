use std::fs;
use std::path::Path;

use encoding_rs::SHIFT_JIS;

use crate::ir::EncodingChoice;

pub struct LoadedSource {
    pub text: String,
    pub encoding_used: String,
    pub warnings: Vec<String>,
}

pub fn load_path(path: &Path, encoding: EncodingChoice) -> Result<LoadedSource, String> {
    let bytes = fs::read(path).map_err(|e| format!("failed to read input: {e}"))?;
    decode_bytes(&bytes, encoding)
}

pub fn decode_bytes(bytes: &[u8], encoding: EncodingChoice) -> Result<LoadedSource, String> {
    let mut warnings = Vec::new();
    let (decoded, encoding_used) = match encoding {
        EncodingChoice::Utf8 => {
            let text = std::str::from_utf8(bytes)
                .map_err(|e| format!("input is not valid UTF-8: {e}"))?
                .to_string();
            (text, "utf-8".to_string())
        }
        EncodingChoice::ShiftJis => {
            let (text, had_errors) = decode_shift_jis(bytes);
            if had_errors {
                warnings.push("Shift-JIS decoder replaced malformed byte sequences".to_string());
            }
            (text, "shift_jis".to_string())
        }
        EncodingChoice::Auto => match std::str::from_utf8(bytes) {
            Ok(text) => (text.to_string(), "utf-8".to_string()),
            Err(_) => {
                let (text, had_errors) = decode_shift_jis(bytes);
                if had_errors {
                    warnings.push(
                        "auto encoding selected Shift-JIS fallback with malformed byte replacements"
                            .to_string(),
                    );
                }
                (text, "shift_jis".to_string())
            }
        },
    };

    let normalized = decoded.replace("\r\n", "\n").replace('\r', "\n");
    Ok(LoadedSource {
        text: strip_comments(&normalized),
        encoding_used,
        warnings,
    })
}

pub fn strip_comments(input: &str) -> String {
    let mut out = String::new();
    for line in input.lines() {
        let mut in_quote = false;
        let mut escaped = false;
        for ch in line.chars() {
            if escaped {
                out.push(ch);
                escaped = false;
                continue;
            }
            match ch {
                '\\' if in_quote => {
                    out.push(ch);
                    escaped = true;
                }
                '"' => {
                    in_quote = !in_quote;
                    out.push(ch);
                }
                ';' if !in_quote => break,
                _ => out.push(ch),
            }
        }
        out.push('\n');
    }
    out
}

fn decode_shift_jis(bytes: &[u8]) -> (String, bool) {
    let (cow, _, had_errors) = SHIFT_JIS.decode(bytes);
    (cow.into_owned(), had_errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_jis_decodes_japanese_title() {
        let bytes = b"#title \"\x83\x65\x83\x58\x83\x67\"\r\n1 c\r\n";
        let loaded = decode_bytes(bytes, EncodingChoice::ShiftJis).unwrap();
        assert_eq!(loaded.encoding_used, "shift_jis");
        assert!(loaded.warnings.is_empty());
        assert!(loaded.text.contains("#title \"テスト\""));
        assert!(loaded.text.contains("1 c\n"));
    }

    #[test]
    fn auto_uses_shift_jis_when_utf8_fails() {
        let bytes = b"#title \"\x83\x65\x83\x58\x83\x67\"\n";
        let loaded = decode_bytes(bytes, EncodingChoice::Auto).unwrap();
        assert_eq!(loaded.encoding_used, "shift_jis");
        assert!(loaded.text.contains("テスト"));
    }

    #[test]
    fn comments_are_not_stripped_inside_quotes() {
        let loaded = decode_bytes(
            b"#title \"a;b\" ; comment\n1 c ; comment\n",
            EncodingChoice::Utf8,
        )
        .unwrap();
        assert_eq!(loaded.text, "#title \"a;b\" \n1 c \n");
    }
}
