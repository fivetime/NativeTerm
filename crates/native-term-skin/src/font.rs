//! Weights of the interface's face. egui draws a face in one weight, so
//! each weight a design uses is a family of its own, which the program
//! fills when it installs its fonts (with the face of that weight first,
//! then whatever the others fall back to): `Proportional` is the regular
//! weight, these the heavier ones.

/// The family of the medium weight (500: buttons, badges, headers).
pub const MEDIUM: &str = "medium";
/// The family of the semibold weight (600: titles, the current tab).
pub const SEMIBOLD: &str = "semibold";

/// How heavy a text is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
}

impl Weight {
    pub fn family(self) -> egui::FontFamily {
        match self {
            Weight::Regular => egui::FontFamily::Proportional,
            Weight::Medium => egui::FontFamily::Name(MEDIUM.into()),
            Weight::Semibold => egui::FontFamily::Name(SEMIBOLD.into()),
        }
    }
}

/// The interface's face at `size` in `weight`; the regular one where the
/// program installed no family of that weight.
pub fn font(ctx: &egui::Context, size: f32, weight: Weight) -> egui::FontId {
    let family = weight.family();
    let known = matches!(family, egui::FontFamily::Proportional)
        || ctx.fonts(|fonts| fonts.definitions().families.contains_key(&family));
    egui::FontId::new(size, if known { family } else { egui::FontFamily::Proportional })
}
