use std::collections::{BTreeMap, BTreeSet};

pub struct OutputRouter {
    outputs: BTreeSet<String>,
    focused: Option<String>,
    explicit: BTreeMap<String, String>,
    automatic: BTreeMap<String, String>,
}

impl OutputRouter {
    pub fn new(outputs: &[&str], focused: Option<&str>) -> Self {
        let mut this = Self {
            outputs: outputs.iter().map(|s| s.to_string()).collect(),
            focused: None,
            explicit: BTreeMap::new(),
            automatic: BTreeMap::new(),
        };
        this.set_focused(focused);
        this
    }

    pub fn set_outputs(&mut self, outputs: &[&str]) {
        self.outputs = outputs.iter().map(|s| s.to_string()).collect();
        if self
            .focused
            .as_ref()
            .is_some_and(|f| !self.outputs.contains(f))
        {
            self.focused = None;
        }
    }

    pub fn set_focused(&mut self, output: Option<&str>) -> bool {
        if let Some(name) = output {
            if !self.outputs.contains(name) {
                return false;
            }
            self.focused = Some(name.to_string());
        } else {
            self.focused = None;
        }
        true
    }

    pub fn pin(&mut self, surface: &str, output: &str) -> bool {
        if !self.outputs.contains(output) {
            return false;
        }
        self.explicit
            .insert(surface.to_string(), output.to_string());
        true
    }

    /// Pin a transient layer surface to the output that owns its focused parent.
    pub fn pin_focused_output(&mut self, surface: &str) -> bool {
        let Some(output) = self.focused.clone() else {
            return false;
        };
        self.explicit.insert(surface.to_string(), output);
        true
    }

    /// Auto binding records a fallback but never overwrites an explicit pin.
    pub fn bind_auto(&mut self, surface: &str) -> bool {
        if self.explicit.contains_key(surface) {
            return false;
        }
        let Some(output) = self.focused.clone() else {
            return false;
        };
        self.automatic.insert(surface.to_string(), output);
        true
    }

    pub fn unpin(&mut self, surface: &str) {
        self.explicit.remove(surface);
        self.automatic.remove(surface);
    }

    /// Resolve exact pin, parent pin, automatic choice, then focused output.
    /// Unpinned layer surfaces deliberately have no focused-output fallback.
    pub fn route(
        &self,
        surface: &str,
        parent: Option<&str>,
        layer_surface: bool,
    ) -> Option<String> {
        let active = |map: &BTreeMap<String, String>, id: &str| {
            map.get(id).filter(|o| self.outputs.contains(*o)).cloned()
        };
        active(&self.explicit, surface)
            .or_else(|| parent.and_then(|p| active(&self.explicit, p)))
            .or_else(|| active(&self.automatic, surface))
            .or_else(|| {
                if layer_surface {
                    None
                } else {
                    self.focused.clone().filter(|o| self.outputs.contains(o))
                }
            })
    }
}
