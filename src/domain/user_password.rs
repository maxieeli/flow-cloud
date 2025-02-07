use fancy_regex::Regex;
use lazy_static::lazy_static;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug)]
pub struct UserPassword(pub String);

impl UserPassword {
    pub fn parse(s: String) -> Result<UserPassword, String> {
        if s.trim().is_empty() {
            return Err("User password can not be empty or whitespace".to_owned());
        }
    
        if s.graphemes(true).count() > 100 {
            return Err("Password is too long".to_owned());
        }
    
        let forbidden_characters = ['/', '(', ')', '"', '<', '>', '\\', '{', '}'];
        let contains_forbidden_characters = s.chars().any(|g| forbidden_characters.contains(&g));
        if contains_forbidden_characters {
            return Err("Password contains invalid characters".to_string());
        }
        if !validate_password(&s) {
            return Err("Password format invalid".to_string());
        }
        Ok(Self(s))
    }
}

impl AsRef<str> for UserPassword {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

lazy_static! {
    static ref PASSWORD: Regex = Regex::new("((?=.*\\d)(?=.*[a-z])(?=.*[A-Z])(?=.*[\\W]).{6,20})").unwrap();
}

pub fn validate_password(password: &str) -> bool {
    match PASSWORD.is_match(password) {
        Ok(is_match) => is_match,
        Err(e) => {
            tracing::error!("validate_password fail: {:?}", e);
            false
        },
    }
}
