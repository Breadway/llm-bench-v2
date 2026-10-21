#[derive(Clone, Debug)]
pub struct Parameter {
    values: Vec<f32>,
    tracked: bool,
}
impl Parameter {
    pub fn tracked(values: Vec<f32>) -> Self {
        Self {
            values,
            tracked: true,
        }
    }
    pub fn untracked(values: Vec<f32>) -> Self {
        Self {
            values,
            tracked: false,
        }
    }
    pub fn requires_grad(&self) -> bool {
        self.tracked
    }
    pub fn values(&self) -> &[f32] {
        &self.values
    }
    pub fn load_values(&mut self, values: Vec<f32>) {
        if values.len() == self.values.len() {
            self.values = values
        }
    }
}
pub struct Output {
    value: f32,
    input: Vec<f32>,
    graph: bool,
    consumed: bool,
}
impl Output {
    pub fn value(&self) -> f32 {
        self.value
    }
}
#[derive(Default)]
pub struct Engine {
    graphs: usize,
}
impl Engine {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn retained_graphs(&self) -> usize {
        self.graphs
    }
    pub fn forward_train(&mut self, input: &[f32], p: &Parameter) -> Output {
        let graph = p.tracked;
        if graph {
            self.graphs += 1
        }
        Output {
            value: input.iter().zip(&p.values).map(|(a, b)| a * b).sum(),
            input: input.to_vec(),
            graph,
            consumed: false,
        }
    }
    pub fn forward_eval(&self, input: &[f32], p: &Parameter) -> Output {
        Output {
            value: input.iter().zip(&p.values).map(|(a, b)| a * b).sum(),
            input: input.to_vec(),
            graph: false,
            consumed: true,
        }
    }
    pub fn backward(&mut self, out: &mut Output, p: &mut Parameter, lr: f32) -> bool {
        if !out.graph || out.consumed || !p.tracked || out.input.len() != p.values.len() {
            return false;
        }
        for (w, x) in p.values.iter_mut().zip(&out.input) {
            *w -= lr * x;
        }
        out.consumed = true;
        self.graphs = self.graphs.saturating_sub(1);
        true
    }
}
