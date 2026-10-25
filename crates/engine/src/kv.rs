//! The engine's `key = value` text files (settings, world and player files, themes, configs, GUI
//! sidecars, animations, connected textures): one reader, so all treat comments alike. A line
//! starting with `#` is a comment; a value is taken whole, `#` included (theme colors, text
//! color tags).

/// (line number, key, value) of every `key = value` line, both trimmed.
pub fn pairs(src: &str) -> impl Iterator<Item = (usize, &str, &str)> {
    src.lines().enumerate().filter_map(|(i, l)| {
        let l = l.trim_start_matches('\u{feff}').trim();
        if l.is_empty() || l.starts_with('#') {
            return None;
        }
        let (k, v) = l.split_once('=')?;
        Some((i + 1, k.trim(), v.trim()))
    })
}

/// The value of `key`, if the text sets it.
pub fn get<'a>(src: &'a str, key: &str) -> Option<&'a str> {
    pairs(src).find(|p| p.1 == key).map(|p| p.2)
}

/// `value` as a number; a value that is not one is reported with its file and line instead of
/// quietly becoming the default.
pub fn number<T: std::str::FromStr>(file: &str, line: usize, key: &str, value: &str) -> Option<T> {
    let n = value.parse().ok();
    if n.is_none() {
        log::warn!("{file}:{line}: {key} = '{value}' is not a number");
    }
    n
}

#[cfg(test)]
mod tests {
    #[test]
    fn comments_and_values() {
        let src = "# comment\n\n a = 1 \ncolor = #ff0000\nborder = 4 # px\nnot a pair\n";
        let p: Vec<_> = super::pairs(src).collect();
        assert_eq!(p, vec![(3, "a", "1"), (4, "color", "#ff0000"), (5, "border", "4 # px")]);
        assert_eq!(super::get(src, "color"), Some("#ff0000"));
        assert_eq!(super::number::<f32>("t", 5, "border", "4 # px"), None);
    }
}
