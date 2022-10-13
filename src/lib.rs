#![warn(missing_docs)]

//! Basic implementation of a Reservoir.

use rand::Rng;

/// Builder for a reservoir. Can stream in new samples and merge
/// with other reservoirs.
#[derive(Clone, Default, Debug)]
pub struct ReservoirBuilder {
    history: u32,
    weight_sum: f32,
    selected_target_pdf: f32,
}

/// A ready to use reservoir.
#[derive(Clone, Default, Debug)]
pub struct Reservoir {
    history: u32,
    contribution_weight: f32,
}

impl Reservoir {
    /// Construct a reservoir from a single sample.
    pub fn from_sample(source_pdf: f32) -> Self {
        Self {
            history: 1,
            contribution_weight: 1.0 / source_pdf,
        }
    }

    /// Check if the reservoir has any weight. This is useful in order to
    /// early out from doing expensive computation when reconstructing the
    /// target PDF of a selected sample.
    pub fn has_weight(&self) -> bool {
        self.contribution_weight != 0.0
    }

    /// Return a copy of the reservoir with clamped history.
    pub fn with_max_history(&self, max_history: u32) -> Self {
        Self {
            history: self.history.min(max_history),
            contribution_weight: self.contribution_weight,
        }
    }

    /// Convert the reservoir back into a builder state.
    pub fn to_builder(&self, selected_target_pdf: f32) -> ReservoirBuilder {
        ReservoirBuilder {
            history: self.history,
            weight_sum: self.contribution_weight * self.history as f32 * selected_target_pdf,
            selected_target_pdf,
        }
    }

    /// Return the contribution weight of the selected sample.
    pub fn contribution_weight(&self) -> f32 {
        self.contribution_weight
    }

    /// Return the stored history.
    pub fn history(&self) -> u32 {
        self.history
    }
}

impl ReservoirBuilder {
    /// Finish building a reservoir.
    pub fn finish(self) -> Reservoir {
        let history = self.history;
        self.finish_with_history(history)
    }

    /// Finish building a reservoir, using the given history
    /// for weighting (while the stored history is unaffected).
    pub fn finish_with_history(self, unbiased_history: u32) -> Reservoir {
        let denom = unbiased_history as f32 * self.selected_target_pdf;
        Reservoir {
            history: self.history,
            contribution_weight: if denom > 0.0 {
                self.weight_sum / denom
            } else {
                0.0
            },
        }
    }

    /// Invalidate the target PDF of the selected sample.
    pub fn invalidate(&mut self) {
        self.selected_target_pdf = 0.0;
        self.weight_sum = 0.0;
    }

    /// Reweight the reservoir as if it had less samples.
    pub fn clamp_history(&mut self, history: u32) {
        assert_ne!(history, 0);
        if self.history > history {
            let avg = self.weight_sum / self.history as f32;
            self.history = history;
            self.weight_sum = avg * history as f32;
        }
    }

    /// Return the stored history.
    pub fn history(&self) -> u32 {
        self.history
    }

    /// Stream in a new sample into a reservoir.
    ///
    /// Returns true if the sample got stored into the reservoir.
    ///
    /// The `source_pdf` is a PDF of how the sample was produced.
    /// The `target_value` is how much we consider this sample to be important for the target function.
    pub fn stream<R: Rng>(&mut self, source_pdf: f32, target_value: f32, random: &mut R) -> bool {
        if true {
            // canonical fast path
            let weight = target_value / source_pdf;
            self.history += 1;
            self.weight_sum += weight;
            if random.gen::<f32>() * self.weight_sum < weight {
                self.selected_target_pdf = target_value;
                true
            } else {
                false
            }
        } else {
            // equivalent semantically, but done via another reservoir
            let other = Reservoir::from_sample(source_pdf).to_builder(target_value);
            self.merge(&other, random)
        }
    }

    /// Register a sample with zero value.
    pub fn add_empty_sample(&mut self) {
        self.history += 1;
    }

    /// Merge another reservoir into this one.
    ///
    /// Returns true if the other's sample got stored into the reservoir.
    pub fn merge<R: Rng>(&mut self, other: &Self, random: &mut R) -> bool {
        self.weight_sum += other.weight_sum;
        self.history += other.history;
        if random.gen::<f32>() * self.weight_sum < other.weight_sum {
            self.selected_target_pdf = other.selected_target_pdf;
            true
        } else {
            false
        }
    }

    /// Merge history from another reservoir that has no weight.
    pub fn merge_history(&mut self, other: &Reservoir) {
        self.history += other.history;
    }
}
