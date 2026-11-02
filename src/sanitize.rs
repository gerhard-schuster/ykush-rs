// SPDX-License-Identifier: Apache-2.0
//! Keeps text that enters the program from carrying terminal escapes.

/// Replaces every character outside printable ASCII with `?`.
///
/// A serial number is whatever the USB descriptor says, argv is whatever the
/// caller put there, and both end up on the terminal. Without this, a device
/// or a calling script could hide an ANSI escape sequence in the text and
/// control what the terminal does with the rest of the line, reorder what the
/// line appears to say with bidirectional overrides, or make two different
/// serial numbers look alike with invisible or look-alike characters. Unicode
/// has too many of those to list; a serial number of a Yepkit board is ASCII,
/// so nothing else is let through.
///
/// The one exception is U+FFFD, which a lossy argv conversion puts where a
/// byte was not UTF-8: it prints as a visible mark, so an error message still
/// shows where the bad byte was.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            ' '..='~' | '\u{fffd}' => c,
            _ => '?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_serial_number_from_the_device_cannot_carry_terminal_escapes() {
        // What a hostile device could put into its USB descriptor.
        assert_eq!(sanitize("YK\x1b[2JEVIL"), "YK?[2JEVIL");
        assert_eq!(sanitize("YK\r\n\x07"), "YK???");
        assert_eq!(sanitize("YK\u{9b}31m"), "YK?31m");

        // An ordinary serial number is left alone.
        assert_eq!(sanitize("Y3N13808"), "Y3N13808");
        assert_eq!(sanitize("<unknown>"), "<unknown>");
    }

    #[test]
    fn a_serial_number_cannot_reorder_the_line_it_is_printed_on() {
        // Bidi overrides and isolates are not control characters, so
        // `is_control` alone lets them through. Displayed, they turn the rest
        // of the line around.
        assert_eq!(sanitize("Y3N\u{202e}80831"), "Y3N?80831");
        assert_eq!(sanitize("Y3N\u{202d}138"), "Y3N?138");
        assert_eq!(sanitize("Y3N\u{2066}138\u{2069}"), "Y3N?138?");
        assert_eq!(sanitize("Y3N\u{200f}138"), "Y3N?138");
        assert_eq!(sanitize("\u{061c}Y3N138"), "?Y3N138");
    }

    #[test]
    fn two_serial_numbers_cannot_be_made_to_look_alike() {
        // Zero width characters print as nothing, so without this a doctored
        // board could show the serial number of another one.
        assert_eq!(sanitize("Y3N\u{200b}13808"), "Y3N?13808");
        assert_eq!(sanitize("Y3N\u{feff}13808"), "Y3N?13808");
        assert_eq!(sanitize("Y3N\u{00ad}13808"), "Y3N?13808");
        assert_eq!(sanitize("Y3N\u{2060}13808"), "Y3N?13808");
    }

    #[test]
    fn nothing_outside_printable_ascii_gets_through() {
        // Tag characters, line and paragraph separators, invisible fillers,
        // combining marks and look-alike letters: none of them belongs in a
        // serial number, and listing them one by one never ends.
        for c in [
            '\u{e0041}',
            '\u{2028}',
            '\u{2029}',
            '\u{180e}',
            '\u{fff9}',
            '\u{3164}',
            '\u{034f}',
            '\u{0336}',
            '\u{0417}',
            '\u{00e4}',
            '\u{00a0}',
        ] {
            assert_eq!(sanitize(&format!("Y{c}K")), "Y?K", "U+{:04X}", c as u32);
        }
        assert_eq!(sanitize(" !~"), " !~");
    }

    #[test]
    fn an_argument_echoed_in_an_error_cannot_carry_terminal_escapes() {
        // What a calling script could pass and later see echoed on stderr.
        assert_eq!(sanitize("-\x1b[31mred"), "-?[31mred");
        assert_eq!(sanitize("5\x07"), "5?");

        // The replacement character from a lossy argv conversion is kept, so
        // the error message still shows where the bad byte was.
        assert_eq!(sanitize("YK\u{fffd}1"), "YK\u{fffd}1");
    }
}
