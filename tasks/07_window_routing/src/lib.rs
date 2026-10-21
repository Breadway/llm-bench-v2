pub struct OutputRouter {
    _private: (),
}
impl OutputRouter {
    pub fn new(_outputs: &[&str], _focused: Option<&str>) -> Self {
        todo!()
    }
    pub fn set_outputs(&mut self, _outputs: &[&str]) {
        todo!()
    }
    pub fn set_focused(&mut self, _output: Option<&str>) -> bool {
        todo!()
    }
    pub fn pin(&mut self, _surface: &str, _output: &str) -> bool {
        todo!()
    }
    pub fn pin_focused_output(&mut self, _surface: &str) -> bool {
        todo!()
    }
    pub fn bind_auto(&mut self, _surface: &str) -> bool {
        todo!()
    }
    pub fn unpin(&mut self, _surface: &str) {
        todo!()
    }
    pub fn route(
        &self,
        _surface: &str,
        _parent: Option<&str>,
        _layer_surface: bool,
    ) -> Option<String> {
        todo!()
    }
}
