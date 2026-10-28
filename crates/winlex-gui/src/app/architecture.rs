//! Document-facing architecture order and default selection policy.
use winlex_core::Architecture;

/// Canonical display and fallback order; a user preference is applied separately.
const ORDER: [Architecture; 3] = [Architecture::X86, Architecture::X64, Architecture::Arm64];

pub(super) fn supported(mask: i32) -> impl Iterator<Item = Architecture> {
    ORDER.into_iter().filter(move |arch| arch.supports(mask))
}

/// The preferred architecture when supported, otherwise the first canonical one.
pub(super) fn preferred(mask: i32, preferred: Option<Architecture>) -> Option<Architecture> {
    preferred
        .filter(|arch| arch.supports(mask))
        .or_else(|| supported(mask).next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_filters_in_display_order() {
        assert_eq!(
            supported(5).collect::<Vec<_>>(),
            [Architecture::X86, Architecture::Arm64]
        );
        assert_eq!(supported(0).next(), None);
    }

    #[test]
    fn preference_falls_back_to_canonical_order() {
        assert_eq!(preferred(7, None), Some(Architecture::X86));
        assert_eq!(
            preferred(7, Some(Architecture::X64)),
            Some(Architecture::X64)
        );
        assert_eq!(
            preferred(7, Some(Architecture::Arm64)),
            Some(Architecture::Arm64)
        );
        assert_eq!(
            preferred(3, Some(Architecture::Arm64)),
            Some(Architecture::X86)
        );
        assert_eq!(
            preferred(4, Some(Architecture::X64)),
            Some(Architecture::Arm64)
        );
        assert_eq!(preferred(1, None), Some(Architecture::X86));
        assert_eq!(preferred(2, None), Some(Architecture::X64));
        assert_eq!(preferred(0, None), None);
    }
}
