#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Backend {
    Cpu,
    Cuda,
    Migraphx,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderState {
    pub backend: Backend,
    pub available: bool,
    pub registered: bool,
    pub detail: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendChoice {
    pub selected: Backend,
    pub requested: Backend,
    pub fallback_reason: Option<String>,
}
pub fn choose_backend(_requested: Backend, _states: &[ProviderState]) -> BackendChoice {
    todo!()
}
#[derive(Default)]
pub struct ShapeCache {
    _private: (),
}
impl ShapeCache {
    pub fn remember(&mut self, _batch: usize, _sequence: usize) -> bool {
        todo!()
    }
    pub fn len(&self) -> usize {
        todo!()
    }
    pub fn is_empty(&self) -> bool {
        todo!()
    }
}
pub fn mean_pool(
    _output: &[f32],
    _actual_seq: usize,
    _hidden: usize,
    _mask: &[u8],
) -> Option<Vec<f32>> {
    todo!()
}
