#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8);
impl Color {
    pub fn parse_hex(_input: &str) -> Option<Self> {
        todo!()
    }
    pub fn luminance(self) -> f64 {
        todo!()
    }
    pub fn contrast(self, _other: Self) -> f64 {
        todo!()
    }
    pub fn readable_ink(self) -> Self {
        todo!()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Shared,
    App,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssError {
    pub offset: usize,
}
#[derive(Default)]
pub struct CssCascade;
impl CssCascade {
    pub fn new() -> Self {
        todo!()
    }
    pub fn load(&mut self, _origin: Origin, _css: &str) -> Result<(), CssError> {
        todo!()
    }
    pub fn resolve(&self, _selector: &str, _property: &str) -> Option<&str> {
        todo!()
    }
}
