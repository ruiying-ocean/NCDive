use eframe::egui::Color32;

pub const MISSING: Color32 = Color32::from_rgb(215, 221, 225);
pub const NON_POSITIVE: Color32 = Color32::from_rgb(229, 197, 167);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Palette {
    #[default]
    Viridis,
    Plasma,
    Cividis,
    BlueRed,
}

impl Palette {
    pub const ALL: [Self; 4] = [Self::Viridis, Self::Plasma, Self::Cividis, Self::BlueRed];

    pub fn name(self) -> &'static str {
        match self {
            Self::Viridis => "Viridis",
            Self::Plasma => "Plasma",
            Self::Cividis => "Cividis",
            Self::BlueRed => "Blue–white–red",
        }
    }

    pub fn colour(self, fraction: f64) -> Color32 {
        let colours: &[u8] = match self {
            Self::Viridis => include_bytes!("../assets/viridis.rgb"),
            Self::Plasma => include_bytes!("../assets/plasma.rgb"),
            Self::Cividis => include_bytes!("../assets/cividis.rgb"),
            Self::BlueRed => include_bytes!("../assets/blue_red.rgb"),
        };
        let count = colours.len() / 3;
        let position = fraction.clamp(0.0, 1.0) * (count - 1) as f64;
        let index = (position.floor() as usize).min(count - 2);
        let mix = position - index as f64;
        let rgb: [u8; 3] = std::array::from_fn(|channel| {
            (f64::from(colours[index * 3 + channel]) * (1.0 - mix)
                + f64::from(colours[(index + 1) * 3 + channel]) * mix)
                .round() as u8
        });
        Color32::from_rgb(rgb[0], rgb[1], rgb[2])
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourScale {
    pub limits: [f64; 2],
    pub palette: Palette,
    pub logarithmic: bool,
}

impl ColourScale {
    pub fn validate(self) -> Result<(), &'static str> {
        if self.limits.iter().any(|v| !v.is_finite()) || self.limits[0] > self.limits[1] {
            return Err("Enter finite colour limits with Min no greater than Max.");
        }
        if self.logarithmic && self.limits[0] <= 0.0 {
            return Err("Log₁₀ needs positive colour limits. Use Auto range or enter Min > 0.");
        }
        Ok(())
    }

    pub fn auto_range(&mut self, values: &[f64], difference: bool) {
        let mut limits: Option<[f64; 2]> = None;
        for &value in values {
            if value.is_finite() && (!self.logarithmic || value > 0.0) {
                limits =
                    Some(limits.map_or([value, value], |[lo, hi]| [lo.min(value), hi.max(value)]));
            }
        }
        self.limits = limits.unwrap_or(if self.logarithmic {
            [1.0, 10.0]
        } else {
            [0.0, 1.0]
        });
        if difference && !self.logarithmic {
            let extent = self.limits[0].abs().max(self.limits[1].abs());
            self.limits = [-extent, extent];
        }
    }

    pub fn fraction(self, value: f64) -> f64 {
        if self.limits[0] == self.limits[1] {
            return 0.5;
        }
        let [minimum, maximum] = self.limits;
        if self.logarithmic {
            ((value.log10() - minimum.log10()) / (maximum.log10() - minimum.log10()))
                .clamp(0.0, 1.0)
        } else {
            ((value * 0.5 - minimum * 0.5) / (maximum * 0.5 - minimum * 0.5)).clamp(0.0, 1.0)
        }
    }

    pub fn value_at(self, fraction: f64) -> f64 {
        let [minimum, maximum] = self.limits;
        if self.logarithmic {
            10.0_f64.powf(minimum.log10() * (1.0 - fraction) + maximum.log10() * fraction)
        } else {
            minimum * (1.0 - fraction) + maximum * fraction
        }
    }

    pub fn colour(self, value: f64) -> Color32 {
        if !value.is_finite() {
            MISSING
        } else if self.logarithmic && value <= 0.0 {
            NON_POSITIVE
        } else {
            self.palette.colour(self.fraction(value))
        }
    }

    pub fn name(self) -> &'static str {
        if self.logarithmic {
            "Log₁₀ colour scale"
        } else {
            "Linear colour scale"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logarithmic_scale_preserves_values_and_distinguishes_exclusions() {
        let mut scale = ColourScale {
            limits: [1.0, 1000.0],
            palette: Palette::Viridis,
            logarithmic: true,
        };
        assert!((scale.fraction(10.0) - 1.0 / 3.0).abs() < 1e-12);
        assert!((scale.value_at(2.0 / 3.0) - 100.0).abs() < 1e-10);
        assert_eq!(scale.colour(f64::NAN), MISSING);
        assert_eq!(scale.colour(0.0), NON_POSITIVE);
        assert_eq!(scale.colour(-1.0), NON_POSITIVE);
        scale.auto_range(&[f64::NAN, -1.0, 0.0, 2.0, 200.0], false);
        assert_eq!(scale.limits, [2.0, 200.0]);
        scale.limits[0] = 0.0;
        assert!(scale.validate().is_err());
    }

    #[test]
    fn palettes_constant_fields_and_difference_limits_are_consistent() {
        let mut scale = ColourScale {
            limits: [7.0, 7.0],
            palette: Palette::BlueRed,
            logarithmic: false,
        };
        assert_eq!(scale.fraction(7.0), 0.5);
        scale.auto_range(&[-2.0, 5.0, f64::NAN], true);
        assert_eq!(scale.limits, [-5.0, 5.0]);
        assert_eq!(scale.fraction(0.0), 0.5);
        assert_eq!(Palette::Viridis.colour(0.0), Color32::from_rgb(68, 1, 84));
        assert_eq!(
            Palette::Viridis.colour(1.0),
            Color32::from_rgb(253, 231, 37)
        );
        for palette in Palette::ALL {
            assert_ne!(palette.colour(0.0), palette.colour(1.0));
            assert_ne!(palette.colour(0.5), MISSING);
        }
    }
}
