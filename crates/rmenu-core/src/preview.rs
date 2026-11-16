pub(crate) fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut line: Vec<char> = Vec::new();
    let mut column = 0;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        // A C1 control is the 7-bit ESC plus the character 0x40 below it.
        let escape = match ch {
            '\u{1b}' => match chars.peek() {
                Some(&next) if ('@'..='_').contains(&next) => {
                    chars.next();
                    Some(next)
                }
                _ => None,
            },
            '\u{80}'..='\u{9f}' => Some(char::from(ch as u8 - 0x40)),
            _ => {
                match ch {
                    '\r' => column = 0,
                    '\n' => {
                        out.extend(line.drain(..));
                        out.push('\n');
                        column = 0;
                    }
                    _ => {
                        if column < line.len() {
                            line[column] = ch;
                        } else {
                            line.push(ch);
                        }
                        column += 1;
                    }
                }
                continue;
            }
        };
        match escape {
            Some('[') => {
                while chars.next_if(|c| ('0'..='?').contains(c)).is_some() {}
                while chars.next_if(|c| (' '..='/').contains(c)).is_some() {}
                chars.next_if(|c| ('@'..='~').contains(c));
            }
            // OSC, DCS, SOS, PM and APC run to a string terminator. Some programs end OSC with BEL.
            Some(']' | 'P' | 'X' | '^' | '_') => {
                while let Some(c) = chars.next() {
                    match c {
                        '\u{7}' | '\u{9c}' => break,
                        '\u{1b}' => {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Some(_) => {}
            // ESC ( B picks a character set. Others like it take intermediates, then a final.
            None => {
                while chars.next_if(|c| (' '..='/').contains(c)).is_some() {}
                chars.next_if(|c| ('0'..='~').contains(c));
            }
        }
    }
    out.extend(line);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_sequences_are_stripped_and_text_is_kept() {
        for (input, expected) in [
            ("\u{1b}[1;31mred\u{1b}[0m plain", "red plain"),
            ("\u{1b}[38;2;10;20;30mtrue\u{1b}[m", "true"),
            ("\u{1b}]8;;https://example.com\u{7}link\u{1b}]8;;\u{7}", "link"),
            ("\u{1b}]0;title\u{1b}\\body", "body"),
            ("\u{1b}[2K\u{1b}[1Gline", "line"),
            ("unicode 日本語 é stays", "unicode 日本語 é stays"),
            ("tabs\tand\nnewlines stay", "tabs\tand\nnewlines stay"),
        ] {
            assert_eq!(plain_text(input), expected);
        }
    }

    #[test]
    fn escapes_beyond_csi_and_osc_are_stripped() {
        for (input, expected) in [
            ("\u{1b}[1mbold\u{1b}(B\u{1b}[m plain", "bold plain"),
            ("\u{1b}(0lqk\u{1b}(B", "lqk"),
            ("\u{1b})0\u{1b}#8x", "x"),
            ("\u{1b}7a\u{1b}8\u{1b}=\u{1b}>\u{1b}c", "a"),
            ("\u{9b}31mc1\u{9b}0m", "c1"),
            ("\u{1b}Pq#0;2;0;0;0\u{1b}\\after", "after"),
            ("\u{1b}_Gf=24;AAAA\u{1b}\\img", "img"),
            ("\u{1b}^pm\u{7}\u{1b}Xsos\u{1b}\\x", "x"),
            ("\u{9d}0;title\u{9c}x\u{90}dcs\u{9c}", "x"),
            ("x\u{1b}", "x"),
            ("a\u{1b}\nb", "a\nb"),
        ] {
            assert_eq!(plain_text(input), expected, "{input:?}");
        }
    }

    #[test]
    fn carriage_return_writes_over_the_start_of_its_line() {
        for (input, expected) in [("line\r\nnext", "line\nnext"), ("10%\r100%\ndone", "100%\ndone"), ("abc\rX", "Xbc")]
        {
            assert_eq!(plain_text(input), expected, "{input:?}");
        }
    }
}
