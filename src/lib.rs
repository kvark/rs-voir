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

    /// Convert the reservoir back into a builder state.
    pub fn into_builder(self, selected_target_pdf: f32) -> ReservoirBuilder {
        ReservoirBuilder {
            history: self.history,
            weight_sum: self.contribution_weight * self.history as f32 * selected_target_pdf,
            selected_target_pdf,
        }
    }
}

impl ReservoirBuilder {
    /// Finish building a reservoir.
    /// Clamps history to a given value. History clamping allows reservoirs
    /// to pick up new samples in the future and not get stale.
    pub fn finish(self, max_history: u32) -> Reservoir {
        Reservoir {
            history: self.history.min(max_history),
            contribution_weight: self.weight_sum / (self.history as f32 * self.selected_target_pdf),
        }
    }

    /// Collapse all the collected samples into one.
    ///
    /// This is useful when we want to merge a reservoir with others, but we don't
    /// consider the currently stored samples to be as valuable individually as
    /// the ones stored in other reservoirs.
    pub fn collapse(&mut self) {
        assert_ne!(self.history, 0);
        self.weight_sum /= self.history as f32;
        self.history = 1;
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
            let other = Reservoir::from_sample(source_pdf).into_builder(target_value);
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
        self.history += other.history;
        self.weight_sum += other.weight_sum;
        if random.gen::<f32>() * self.weight_sum < other.weight_sum {
            self.selected_target_pdf = other.selected_target_pdf;
            true
        } else {
            false
        }
    }
}
