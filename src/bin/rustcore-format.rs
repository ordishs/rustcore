use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::io::{self, BufRead, BufWriter, Read, Write};

#[derive(Debug, Default, Clone, PartialEq)]
struct Variant {
    commented: bool,
    key: String,
    value: String,
    comment: String,
}

#[derive(Debug, Default)]
struct Setting {
    key: String,
    group: String,
    sort_by: String,
    comments: String,
    variants: Vec<Variant>,
    compact: bool,
    max_key_length: usize,
}

fn process_line(line: &str) -> Option<Variant> {
    let mut v = Variant::default();
    let mut line = line;
    if let Some(stripped) = line.strip_prefix('#') {
        v.commented = true;
        line = stripped;
    }
    let (key_part, rest) = line.split_once('=')?;
    v.key = clean_key(key_part);
    let rest = rest.trim();
    match rest.split_once('#') {
        Some((value, comment)) => {
            v.value = value.trim().to_string();
            v.comment = comment.trim().to_string();
        }
        None => v.value = rest.to_string(),
    }
    Some(v)
}

fn clean_key(key: &str) -> String {
    key.trim()
        .split('.')
        .map(|p| p.trim())
        .collect::<Vec<_>>()
        .join(".")
}

fn clean_multi_values(value: &str) -> String {
    value
        .split('|')
        .map(|p| p.trim())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn read_settings<R: Read>(reader: R) -> io::Result<Vec<Setting>> {
    let mut pending_section_comment = String::new();
    let mut current_group = String::new();
    let mut is_compact_group = false;
    let mut max_key_length: usize = 0;

    let mut settings: HashMap<String, Setting> = HashMap::new();

    let buf_reader = io::BufReader::new(reader);

    for line_result in buf_reader.lines() {
        let raw = line_result?;
        let line = raw.trim();

        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("# @group:") {
            current_group = rest.trim().to_string();
            is_compact_group = current_group.ends_with(" compact");
            if is_compact_group {
                current_group = current_group
                    .strip_suffix(" compact")
                    .unwrap_or(&current_group)
                    .to_string();
                max_key_length = 0;
            }
            continue;
        }

        if line == "# @endgroup" {
            if is_compact_group {
                for setting in settings.values_mut() {
                    if setting.group == current_group {
                        setting.compact = true;
                        setting.max_key_length = max_key_length;
                    }
                }
            }
            current_group = String::new();
            is_compact_group = false;
            continue;
        }

        match process_line(line) {
            None => {
                // Arbitrary comment line
                let comment_text = line[1..].trim();
                if pending_section_comment.is_empty() {
                    pending_section_comment = comment_text.to_string();
                } else {
                    pending_section_comment.push_str("\n# ");
                    pending_section_comment.push_str(comment_text);
                }
            }
            Some(item) => {
                let root_key = item.key.split('.').next().unwrap_or(&item.key).to_string();

                if !settings.contains_key(&root_key) {
                    let mut setting = Setting {
                        key: root_key.clone(),
                        comments: pending_section_comment.clone(),
                        compact: is_compact_group,
                        ..Default::default()
                    };

                    if !current_group.is_empty() {
                        setting.group = current_group.clone();
                        setting.sort_by = current_group.clone();
                    } else {
                        setting.sort_by = root_key.clone();
                    }

                    pending_section_comment = String::new();
                    settings.insert(root_key.clone(), setting);
                }

                if is_compact_group {
                    let key_length = if item.commented {
                        item.key.len() + 2
                    } else {
                        item.key.len()
                    };
                    if key_length > max_key_length {
                        max_key_length = key_length;
                    }
                }

                settings.get_mut(&root_key).unwrap().variants.push(item);
            }
        }
    }

    let mut result: Vec<Setting> = settings.into_values().collect();
    // Pre-sort for determinism before sort_settings does the real ordering
    result.sort_by(|a, b| a.key.cmp(&b.key));

    Ok(result)
}

fn sort_settings(settings: &mut [Setting]) {
    settings.sort_by(|a, b| {
        // First sort by sort_by
        if a.sort_by != b.sort_by {
            // Empty sort_by sorts last
            if a.sort_by.is_empty() {
                return std::cmp::Ordering::Greater;
            }
            if b.sort_by.is_empty() {
                return std::cmp::Ordering::Less;
            }
            let r1 = a.sort_by.chars().next().unwrap();
            let r2 = b.sort_by.chars().next().unwrap();
            if r1.is_uppercase() != r2.is_uppercase() {
                return if r1.is_uppercase() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            return a.sort_by.cmp(&b.sort_by);
        }

        // Same sort_by: sort by key with group prefix stripped
        let key_i = a
            .key
            .strip_prefix(&format!("{}.", a.group))
            .unwrap_or(&a.key);
        let key_j = b
            .key
            .strip_prefix(&format!("{}.", b.group))
            .unwrap_or(&b.key);

        if key_i.is_empty() {
            return std::cmp::Ordering::Greater;
        }
        if key_j.is_empty() {
            return std::cmp::Ordering::Less;
        }

        let r1 = key_i.chars().next().unwrap();
        let r2 = key_j.chars().next().unwrap();
        if r1.is_uppercase() != r2.is_uppercase() {
            return if r1.is_uppercase() {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }

        key_i.cmp(key_j)
    });
}

fn write_settings<W: Write>(writer: W, settings: &[Setting]) -> io::Result<()> {
    let mut w = BufWriter::new(writer);
    let mut current_group = String::new();
    let mut is_compact_group = false;

    for (i, setting) in settings.iter().enumerate() {
        // Blank line between settings, except between consecutive items in the same compact group
        if i > 0 && (!is_compact_group || setting.group != current_group) {
            w.write_all(b"\n")?;
        }

        if setting.group != current_group {
            if !current_group.is_empty() {
                // End the previous group
                w.write_all(b"# @endgroup\n")?;
                // Add a newline after @endgroup only for non-compact groups
                if !is_compact_group {
                    w.write_all(b"\n")?;
                }
            }
            if !setting.group.is_empty() {
                let mut group_line = format!("# @group: {}", setting.group);
                if setting.compact {
                    group_line.push_str(" compact");
                }
                group_line.push('\n');
                w.write_all(group_line.as_bytes())?;
            }
            current_group = setting.group.clone();
            is_compact_group = setting.compact;
        }

        if !setting.comments.is_empty() {
            let comment_line = format!("# {}\n", setting.comments);
            w.write_all(comment_line.as_bytes())?;
        }

        let max_key_length = if is_compact_group {
            setting.max_key_length
        } else {
            let mut mkl = 0usize;
            for variant in &setting.variants {
                let l = if variant.commented {
                    variant.key.len() + 2
                } else {
                    variant.key.len()
                };
                if l > mkl {
                    mkl = l;
                }
            }
            mkl
        };

        for variant in &setting.variants {
            let prefix = if variant.commented { "# " } else { "" };
            let length = if variant.commented {
                max_key_length.saturating_sub(2)
            } else {
                max_key_length
            };

            let value = clean_multi_values(&variant.value);

            let mut line = String::new();
            write!(line, "{}{:<width$} =", prefix, variant.key, width = length).unwrap();

            if !value.is_empty() {
                line.push(' ');
                line.push_str(&value);
            }

            if !variant.comment.is_empty() {
                line.push_str(" # ");
                line.push_str(&variant.comment);
            }

            line.push('\n');
            w.write_all(line.as_bytes())?;
        }

        // Check if this is the last setting or next setting is in a different group
        if i == settings.len() - 1 || settings[i + 1].group != current_group {
            if !current_group.is_empty() {
                w.write_all(b"# @endgroup\n")?;
            }
            current_group = String::new();
            is_compact_group = false;
        }
    }

    Ok(())
}

fn main() {
    let mut write_back = false;
    let mut help = false;
    let mut filename = String::new();

    let mut args = std::env::args().skip(1);
    for arg in args.by_ref() {
        match arg.as_str() {
            "-w" => write_back = true,
            "-h" => help = true,
            _ if !arg.starts_with('-') => filename = arg,
            _ => {}
        }
    }

    if help {
        eprintln!("  -h\tHelp");
        eprintln!("  -w\tWrite to file");
        return;
    }

    let input: Box<dyn Read> = if filename.is_empty() {
        Box::new(io::stdin())
    } else {
        match std::fs::File::open(&filename) {
            Ok(f) => Box::new(f),
            Err(e) => {
                eprintln!("Error opening file: {e}");
                return;
            }
        }
    };

    let mut settings = match read_settings(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file: {e}");
            return;
        }
    };

    sort_settings(&mut settings);

    if !filename.is_empty() && write_back {
        let tmp = format!("{filename}.tmp");
        let out = match std::fs::File::create(&tmp) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Error creating output file: {e}");
                return;
            }
        };
        if let Err(e) = write_settings(out, &settings) {
            eprintln!("Error writing file: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &filename) {
            eprintln!("Error renaming file: {e}");
        }
    } else if let Err(e) = write_settings(io::stdout(), &settings) {
        eprintln!("Error writing file: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> String {
        let mut settings = read_settings(input.as_bytes()).unwrap();
        sort_settings(&mut settings);
        let mut buf = Vec::new();
        write_settings(&mut buf, &settings).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn test_clean_multi_values() {
        let v = "1|2|3";
        assert_eq!(clean_multi_values(v), "1 | 2 | 3");
    }

    #[test]
    fn test_malformed_compact_group() {
        // A duplicate "# @group: X compact" marker resets max_key_length to 0
        // before "# @endgroup" back-patches it; the commented variant's width
        // adjustment must not underflow. Output verified against the Go tool.
        let input = "# @group: G1 compact
#aaaa=1
# @group: G1 compact
b=2
# @endgroup
";

        let output = run(input);

        assert_eq!(
            output,
            "# @group: G1 compact\n# aaaa = 1\nb = 2\n# @endgroup\n"
        );
    }

    #[test]
    fn test_empty_values() {
        let input = "
\t\t\ta=
\t\t\ta.dev= #Comment
\t\t";

        let output = run(input);

        assert_eq!(output, "a     =\na.dev = # Comment\n");
    }

    #[test]
    fn test_comments() {
        let input = "
\t\t\ta=2
\t\t\tA=1
\t\t\t#The following section is c
\t\t\tc=3 #this is the default value
\t\t\tc.dev=1
\t\t\t#c.test=2
\t\t\tc.prod=3
\t\t";

        let mut settings = read_settings(input.as_bytes()).unwrap();
        sort_settings(&mut settings);
        assert_eq!(settings.len(), 3);

        let mut buf = Vec::new();
        write_settings(&mut buf, &settings).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert_eq!(
            output,
            "A = 1\n\na = 2\n\n# The following section is c\nc        = 3 # this is the default value\nc.dev    = 1\n# c.test = 2\nc.prod   = 3\n"
        );
    }

    #[test]
    fn test_groups() {
        let input = "
\t\t\ta=2
\t\t\t# @group: S1 compact
\t\t\tA=1
\t\t\tC=3
\t\t\tB=2

\t\t\tE=5
\t\t\tD=4
\t\t\t# @endgroup

\t\t\t#The following section is c
\t\t\t# @group: c
\t\t\tc=0 #this is the default value
\t\t\tc.dev=1
\t\t\t#c.test=2 # This is not used at the moment
\t\t\tc.prod=3
\t\t\tsomething.else.c = 19
\t\t\t# @endgroup

\t\t\tb.c = 10
\t\t\tb.d = 11
\t\t";

        let mut settings = read_settings(input.as_bytes()).unwrap();
        sort_settings(&mut settings);
        assert_eq!(settings.len(), 9);

        let mut buf = Vec::new();
        write_settings(&mut buf, &settings).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert_eq!(
            output,
            "# @group: S1 compact\nA = 1\nB = 2\nC = 3\nD = 4\nE = 5\n# @endgroup\n\na = 2\n\nb.c = 10\nb.d = 11\n\n# @group: c\n# The following section is c\nc        = 0 # this is the default value\nc.dev    = 1\n# c.test = 2 # This is not used at the moment\nc.prod   = 3\n\nsomething.else.c = 19\n# @endgroup\n"
        );
    }

    #[test]
    fn test_process_line() {
        // "#a=b" -> Variant { commented: true, key: "a", value: "b", comment: "" }
        let v = process_line("#a=b").unwrap();
        assert_eq!(
            v,
            Variant {
                commented: true,
                key: "a".to_string(),
                value: "b".to_string(),
                comment: String::new(),
            }
        );

        // "a=b #comment" -> Variant { commented: false, key: "a", value: "b", comment: "comment" }
        let v = process_line("a=b #comment").unwrap();
        assert_eq!(
            v,
            Variant {
                commented: false,
                key: "a".to_string(),
                value: "b".to_string(),
                comment: "comment".to_string(),
            }
        );

        // "#comment" -> None
        assert!(process_line("#comment").is_none());
    }
}
