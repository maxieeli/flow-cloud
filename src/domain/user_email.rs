use validator::validate_email;

#[derive(Debug)]
pub struct UserEmail(pub String);

impl UserEmail {
    pub fn parse(s: String) -> Result<UserEmail, String> {
        if s.trim().is_empty() {
            return Err("Email can not be empty or whitespace".to_string());
        }
      
        if validate_email(&s) {
            Ok(Self(s))
        } else {
            Err("Invalid email".to_string())
        }
    }
}

impl AsRef<str> for UserEmail {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
