

#[derive(Clone)]
pub struct CollabAccessControlImpl {
    access_control: AccessControl,
}

impl CollabAccessControlImpl {
    pub fn new(access_control: AccessControl) -> Self {
        Self { access_control }
    }
}
