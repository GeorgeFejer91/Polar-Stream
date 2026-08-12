#[derive(Default)]
pub(crate) struct ExcitationProcessor {
    heart_rate: RunningStats,
    ln_rmssd: RunningStats,
}

impl ExcitationProcessor {
    pub(crate) fn update(&mut self, heart_rate: f32, ln_rmssd: f32) -> Option<f32> {
        self.heart_rate.push(heart_rate);
        self.ln_rmssd.push(ln_rmssd);
        if self.heart_rate.count < 20 || self.ln_rmssd.count < 20 {
            return None;
        }
        let activation =
            self.heart_rate.z_score(heart_rate) * 0.65 - self.ln_rmssd.z_score(ln_rmssd) * 0.35;
        Some((1.0 / (1.0 + (-activation).exp())).clamp(0.0, 1.0))
    }
}

#[derive(Default)]
struct RunningStats {
    count: u64,
    mean: f64,
    m2: f64,
}

impl RunningStats {
    fn push(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
        self.count += 1;
        let delta = f64::from(value) - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (f64::from(value) - self.mean);
    }

    fn z_score(&self, value: f32) -> f32 {
        if self.count < 2 {
            return 0.0;
        }
        let sd = (self.m2 / (self.count - 1) as f64).sqrt();
        if sd < 1e-6 {
            0.0
        } else {
            ((f64::from(value) - self.mean) / sd) as f32
        }
    }
}
