use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug)]
pub struct UserName(pub String);

impl UserName {
    pub fn parse(s: String) -> Result<UserName, String> {
        let is_empty_or_whitespace = s.trim().is_empty();
        if is_empty_or_whitespace {
            return Err("User name can not be empty or whitespace".to_string());
        }
        // A grapheme is defined by the Unicode standard as a "user-perceived"
        // character: `å` is a single grapheme, but it is composed of two characters
        // (`a` and `̊`).
        //
        // `graphemes` returns an iterator over the graphemes in the input `s`.
        // `true` specifies that we want to use the extended grapheme definition set,
        // the recommended one.
        let is_too_long = s.graphemes(true).count() > 256;
        if is_too_long {
            return Err("User name is too long".to_string());
        }

        let forbidden_characters = ['/', '(', ')', '"', '<', '>', '\\', '{', '}'];
        let contains_forbidden_characters = s.chars().any(|g| forbidden_characters.contains(&g));

        if contains_forbidden_characters {
            return Err("User name contains invalid characters".to_string());
        }
        Ok(Self(s))
    }
}

impl AsRef<str> for UserName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
