//! Text from the daemon is shown without control or format characters.

/// General category Cf (format) characters, as ranges of code points.
const FORMAT: &[(u32, u32)] = &[
    (0x00AD, 0x00AD),
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
];

fn is_hidden(c: char) -> bool {
    let code = c as u32;
    c.is_control()
        || code == 0x2028
        || code == 0x2029
        || FORMAT
            .iter()
            .any(|&(low, high)| (low..=high).contains(&code))
}

/// The text with every control, format, line and paragraph separator character replaced by a space.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if is_hidden(c) { ' ' } else { c })
        .collect()
}
