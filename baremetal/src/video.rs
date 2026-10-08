//! Graphics mode selection independent of firmware calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: u32,
}

impl DisplayMode {
    pub fn supported(self) -> bool {
        self.width >= 800
            && self.height >= 600
            && self.stride >= self.width
            && self.pixel_format < 2
    }
}

/// Prefer the desktop's default size, then a valid current mode, then the
/// smallest compatible mode to keep framebuffer traffic bounded.
pub fn choose_mode(
    modes: impl IntoIterator<Item = (u32, DisplayMode)>,
    current: u32,
) -> Option<u32> {
    modes
        .into_iter()
        .filter(|(_, mode)| mode.supported())
        .min_by_key(|(index, mode)| {
            (
                if mode.width == 1024 && mode.height == 768 {
                    0
                } else if *index == current {
                    1
                } else {
                    2
                },
                u64::from(mode.width) * u64::from(mode.height),
                *index,
            )
        })
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mode(width: u32, height: u32) -> DisplayMode {
        DisplayMode {
            width,
            height,
            stride: width,
            pixel_format: 1,
        }
    }
    #[test]
    fn low_resolution_current_mode_uses_compatible_fallback() {
        assert_eq!(
            choose_mode(
                [
                    (0, mode(640, 480)),
                    (1, mode(1920, 1080)),
                    (2, mode(800, 600))
                ],
                0
            ),
            Some(2)
        );
        assert_eq!(
            choose_mode([(0, mode(800, 600)), (1, mode(1024, 768))], 0),
            Some(1)
        );
        assert_eq!(
            choose_mode([(0, mode(1920, 1080)), (1, mode(800, 600))], 0),
            Some(0)
        );
    }
    #[test]
    fn unsupported_formats_and_invalid_stride_are_never_selected() {
        let mut format = mode(1024, 768);
        format.pixel_format = 2;
        let mut stride = mode(1024, 768);
        stride.stride = 100;
        assert_eq!(
            choose_mode([(0, format), (1, stride), (2, mode(800, 600))], 0),
            Some(2)
        );
        assert_eq!(choose_mode([(0, format), (1, mode(640, 480))], 0), None);
    }
}
