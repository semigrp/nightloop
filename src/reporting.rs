pub fn print_pairs(pairs: &[(&str, String)]) {
    let rendered = pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", escape_value(value)))
        .collect::<Vec<_>>()
        .join(" ");
    println!("{rendered}");
}

pub fn escape_value(value: &str) -> String {
    let needs_quotes = value.chars().any(|ch| ch.is_whitespace()) || value.contains('"');
    if !needs_quotes {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
